//! AWS through IAM Identity Center (docs/CONNECTORS.md). Each connection
//! registers its own public OIDC client, so no secret of Claktory's ships
//! anywhere. The person approves it in a browser with the device flow, with
//! the code filled in. A secret made from it is one role in one account:
//! STS credentials from the access portal, kept until 2 minutes before they
//! expire and then fetched again. The role's permission set is what bounds
//! an agent, and Claktory can't narrow it further.
//!
//! None of these calls is signed with SigV4: the OIDC service takes JSON,
//! and the access portal a bearer header.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::json;
use zeroize::Zeroizing;

use super::{
    AccessSpec, BoxFuture, Choice, Choices, Control, Derive, Derived, Device, ExportMeta, FieldSpec, FlowKind, Http, Inspected, Poll,
    Problem, Recipe, Spec, Start, Variable,
};
use crate::transcript::now_ms;

/// What the connection may do: get role credentials, no IAM action of its own.
const SCOPE: &str = "sso:account:access";
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";
/// Role credentials are renewed this long before they expire.
const RENEW_BEFORE_MS: i64 = 2 * 60 * 1000;
/// The access token is refreshed this long before it expires.
const REFRESH_BEFORE_MS: i64 = 5 * 60 * 1000;
/// Pages of accounts or roles read at most.
const PAGES: usize = 50;

pub struct Aws {
    /// In the dev Claktory only: a stand-in playing both the OIDC service
    /// and the access portal (`FACTORY_AWS_BASE`).
    stand_in: Option<String>,
    /// Clients registered, by OIDC service (a region's), reused by later
    /// connections while they have long left to run.
    clients: Mutex<HashMap<String, Client>>,
}

/// A public client registered at a region's OIDC service.
#[derive(Clone)]
struct Client {
    id: String,
    secret: Zeroizing<String>,
    /// When it stops working; 0 if AWS didn't say.
    expires_ms: i64,
}

/// A client is reused while it has this long left: a connection made with
/// it lasts no longer than it does.
const REUSE_LEFT_MS: i64 = 60 * 24 * 60 * 60 * 1000;

fn reusable(client: &Client, now: i64) -> bool {
    client.expires_ms > 0 && client.expires_ms - now >= REUSE_LEFT_MS
}

impl Default for Aws {
    fn default() -> Aws {
        Aws::new()
    }
}

/// AWS isn't offered until it's been tried against a real account: until
/// then only the dev Claktory has it, when asked (`FACTORY_AWS_BASE` or
/// `FACTORY_AWS_ENABLED`).
const SHIPPED: bool = false;

impl Aws {
    /// The recipe, with AWS's own services.
    pub fn new() -> Aws {
        Aws { stand_in: None, clients: Mutex::default() }
    }

    /// The built-in recipe, while AWS isn't shipped only in the dev
    /// Claktory: with `FACTORY_AWS_ENABLED=1` against AWS itself, or with
    /// `FACTORY_AWS_BASE` against a stand-in in AWS's place, to try the
    /// whole flow without AWS. The stand-in answers the OIDC service's paths
    /// (`/client/register`, `/device_authorization`, `/token`) and the
    /// portal's (`/assignment/…`, `/federation/credentials`, `/logout`).
    pub(crate) fn built_in() -> Option<Aws> {
        let set = |name: &str| std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let dev = std::env::args().any(|arg| arg == "--dev");
        let stand_in = set("FACTORY_AWS_BASE").filter(|_| dev).map(|base| base.trim_end_matches('/').to_owned());
        let enabled = dev && set("FACTORY_AWS_ENABLED").is_some_and(|value| value != "0");
        if !(SHIPPED || enabled || stand_in.is_some()) {
            return None;
        }
        if let Some(base) = &stand_in {
            tracing::warn!(%base, "AWS is played by a stand-in");
        }
        Some(Aws { stand_in, ..Aws::new() })
    }
}

static SPEC: LazyLock<Spec> = LazyLock::new(|| {
    Spec {
    id: "aws".into(),
    name: "AWS".into(),
    description: "Give an agent one role in one of your AWS accounts, with short-lived keys that renew by themselves. Uses IAM Identity Center to sign in."
        .into(),
    accesses: vec![AccessSpec {
        id: "roles".into(),
        label: "Get keys for your roles".into(),
        why: "Claktory gets short-lived keys for the roles IAM Identity Center gives you, one role for each secret, and renews them while they're in use.".into(),
        scopes: vec![SCOPE.into()],
        preselected: true,
    }],
    fields: vec![
        FieldSpec {
            id: "start_url".into(),
            label: "Access portal URL".into(),
            placeholder: "https://d-1234567890.awsapps.com/start".into(),
            hint: "In the IAM Identity Center console, under Settings summary.".into(),
            optional: false,
        },
        FieldSpec {
            id: "region".into(),
            label: "Region".into(),
            placeholder: "us-east-1".into(),
            hint: "Where IAM Identity Center is set up, also under Settings summary.".into(),
            optional: false,
        },
    ],
    flow: FlowKind::Device,
    variables: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION", "AWS_DEFAULT_REGION"]
        .map(str::to_owned)
        .to_vec(),
    setup_note: "Needs IAM Identity Center turned on, with you as a user and permission sets assigned to you. Give agents narrow ones."
        .into(),
    setup_url: "https://docs.aws.amazon.com/singlesignon/latest/userguide/get-started-enable-identity-center.html".into(),
    ..Spec::default()
}
});

/// A device flow waiting for the person.
#[derive(Serialize, Deserialize)]
struct Pending {
    #[serde(flatten)]
    client: Registration,
    device_code: String,
}

/// The portal, and the public client registered for the connection.
#[derive(Clone, Serialize, Deserialize)]
struct Registration {
    region: String,
    start_url: String,
    client_id: String,
    client_secret: String,
    /// When the registration stops working (AWS's ~90 days); 0 if unknown.
    client_expires_ms: i64,
}

/// A connection's control: the registration and the SSO session, all at
/// the top level, where the broker hides every value whose key says token
/// or secret.
#[derive(Serialize, Deserialize)]
struct Session {
    #[serde(flatten)]
    client: Registration,
    access_token: String,
    /// Empty when AWS gave none: the session then ends with the access token.
    #[serde(default)]
    refresh_token: String,
    /// When the access token expires.
    expires_ms: i64,
}

/// What a secret made from it remembers, to renew it.
#[derive(Serialize, Deserialize)]
struct Made {
    account_id: String,
    account_name: String,
    role: String,
}

fn guard<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Checks a region's name, so it can only ever be a part of an AWS host name.
fn region(text: &str) -> Result<String, Problem> {
    let region = text.trim();
    let valid = region.len() <= 32
        && region.contains('-')
        && region.chars().all(|char| char.is_ascii_lowercase() || char.is_ascii_digit() || char == '-');
    if !valid {
        return Err(Problem::Refused(format!("\"{region}\" isn't an AWS region, such as us-east-1.")));
    }
    Ok(region.to_owned())
}

/// The access portal's URL as the person gives it: a whole URL, one
/// without https://, or only the directory's id ("d-1234567890").
fn start_url(text: &str) -> Result<String, Problem> {
    let text = text.trim().trim_end_matches('/');
    let bad = || Problem::Refused(format!("\"{text}\" isn't an AWS access portal URL, such as https://d-1234567890.awsapps.com/start."));
    let is_directory = text.starts_with("d-") && text.len() > 2 && text.chars().all(|char| char.is_ascii_alphanumeric() || char == '-');
    let url = if is_directory {
        format!("https://{text}.awsapps.com/start")
    } else if text.contains("://") {
        text.to_owned()
    } else {
        format!("https://{text}")
    };
    let parsed = reqwest::Url::parse(&url).map_err(|_| bad())?;
    let host = parsed.host_str().unwrap_or_default().to_owned();
    if parsed.scheme() != "https" || !host.contains('.') || !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(bad());
    }
    // An awsapps.com portal is always at /start.
    if host.ends_with(".awsapps.com") && parsed.path().trim_matches('/').is_empty() {
        return Ok(format!("https://{host}/start"));
    }
    Ok(url)
}

/// The portal in words: its directory's id, or its host.
fn portal_name(start_url: &str) -> String {
    let host = reqwest::Url::parse(start_url).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_default();
    host.strip_suffix(".awsapps.com").map_or(host.clone(), str::to_owned)
}

impl Aws {
    fn oidc(&self, http: &Http, region: &str) -> String {
        let real = format!("https://oidc.{region}.amazonaws.com");
        http.base("aws.oidc", self.stand_in.as_deref().unwrap_or(&real))
    }

    fn portal(&self, http: &Http, region: &str) -> String {
        let real = format!("https://portal.sso.{region}.amazonaws.com");
        http.base("aws.portal", self.stand_in.as_deref().unwrap_or(&real))
    }

    /// A public client at `oidc`: one registered before while it has long
    /// left, or a new one. Asking for the refresh grant is what makes AWS
    /// give a refresh token.
    async fn client(&self, http: &Http, oidc: &str) -> Result<Client, Problem> {
        if let Some(kept) = guard(&self.clients).get(oidc).filter(|kept| reusable(kept, now_ms())) {
            return Ok(kept.clone());
        }
        let register = json!({
            "clientName": "Claktory",
            "clientType": "public",
            "grantTypes": [DEVICE_GRANT, "refresh_token"],
            "scopes": [SCOPE],
        });
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Registered {
            client_id: String,
            client_secret: String,
            #[serde(default)]
            client_secret_expires_at: i64,
        }
        let registered: Registered = match post(http, &format!("{oidc}/client/register"), &register).await? {
            Answer::Ok(body) => field(body)?,
            Answer::Error { status, code, message } => return Err(said(status, &code, &message)),
        };
        let client = Client {
            id: registered.client_id,
            secret: Zeroizing::new(registered.client_secret),
            expires_ms: registered.client_secret_expires_at.saturating_mul(1000),
        };
        guard(&self.clients).insert(oidc.to_owned(), client.clone());
        Ok(client)
    }

    fn at<'a>(&self, http: &'a Http, session: &'a Session) -> Portal<'a> {
        Portal { http, base: self.portal(http, &session.client.region), session }
    }
}

/// What AWS answered: its body, or its status and error code.
enum Answer {
    Ok(serde_json::Value),
    Error { status: u16, code: String, message: String },
}

fn unreachable(error: reqwest::Error) -> Problem {
    Problem::Unreachable(format!("Couldn't reach AWS: {}", error.without_url()))
}

async fn answer(response: reqwest::Response) -> Result<Answer, Problem> {
    let status = response.status();
    let text = response.text().await.map_err(unreachable)?;
    let body: serde_json::Value = serde_json::from_str(&text).unwrap_or_default();
    if status.is_success() {
        return Ok(Answer::Ok(body));
    }
    let field = |key: &str| body[key].as_str().unwrap_or_default().to_owned();
    // OIDC errors are {error, error_description}; the portal's {message}
    // with the type in a header or `__type`.
    let code = [field("error"), field("__type")].into_iter().find(|code| !code.is_empty()).unwrap_or_default();
    let message =
        [field("error_description"), field("message"), field("Message")].into_iter().find(|text| !text.is_empty()).unwrap_or_default();
    Ok(Answer::Error { status: status.as_u16(), code, message })
}

/// An error AWS answered, in words, for anything not handled on its own.
fn said(status: u16, code: &str, message: &str) -> Problem {
    let what = if message.is_empty() { code } else { message };
    let text = if what.is_empty() { format!("AWS answered with an error ({status}).") } else { format!("AWS said: {}", what.trim()) };
    if status >= 500 || status == 429 { Problem::Unreachable(text) } else { Problem::Refused(text) }
}

fn field<T: serde::de::DeserializeOwned>(body: serde_json::Value) -> Result<T, Problem> {
    serde_json::from_value(body).map_err(|error| Problem::Unreachable(format!("AWS sent something unexpected: {error}")))
}

async fn post(http: &Http, url: &str, body: &serde_json::Value) -> Result<Answer, Problem> {
    let response = http.client().post(url).json(body).send().await.map_err(unreachable)?;
    answer(response).await
}

/// The access portal, with a session's token.
struct Portal<'a> {
    http: &'a Http,
    base: String,
    session: &'a Session,
}

/// A GET of the access portal. A token it no longer accepts means
/// connecting again.
async fn get<T: serde::de::DeserializeOwned>(portal: &Portal<'_>, path: &str, query: &[(&str, &str)]) -> Result<T, Problem> {
    let url = format!("{}{path}", portal.base);
    let response = portal
        .http
        .client()
        .get(url)
        .query(query)
        .header("x-amz-sso_bearer_token", &portal.session.access_token)
        .send()
        .await
        .map_err(unreachable)?;
    match answer(response).await? {
        Answer::Ok(body) => field(body),
        Answer::Error { status: 401, .. } => Err(Problem::Reconnect("AWS no longer accepts its sign-in.".to_owned())),
        Answer::Error { status, code, message } => Err(said(status, &code, &message)),
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Tokens {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

#[derive(Clone)]
struct Account {
    id: String,
    name: String,
}

impl Account {
    fn label(&self) -> String {
        if self.name.is_empty() { self.id.clone() } else { format!("{} ({})", self.name, self.id) }
    }
}

async fn accounts(portal: &Portal<'_>) -> Result<Vec<Account>, Problem> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        #[serde(default)]
        account_list: Vec<Listed>,
        #[serde(default)]
        next_token: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listed {
        account_id: String,
        #[serde(default)]
        account_name: String,
    }
    let mut accounts = Vec::new();
    let mut next = String::new();
    for _ in 0..PAGES {
        let mut query = vec![("max_result", "100")];
        if !next.is_empty() {
            query.push(("next_token", &next));
        }
        let page: Page = get(portal, "/assignment/accounts", &query).await?;
        accounts.extend(page.account_list.into_iter().map(|listed| Account { id: listed.account_id, name: listed.account_name }));
        next = page.next_token.unwrap_or_default();
        if next.is_empty() {
            break;
        }
    }
    Ok(accounts)
}

async fn roles(portal: &Portal<'_>, account: &str) -> Result<Vec<String>, Problem> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Page {
        #[serde(default)]
        role_list: Vec<Listed>,
        #[serde(default)]
        next_token: Option<String>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Listed {
        role_name: String,
    }
    let mut roles = Vec::new();
    let mut next = String::new();
    for _ in 0..PAGES {
        let mut query = vec![("account_id", account), ("max_result", "100")];
        if !next.is_empty() {
            query.push(("next_token", &next));
        }
        let page: Page = get(portal, "/assignment/roles", &query).await?;
        roles.extend(page.role_list.into_iter().map(|listed| listed.role_name));
        next = page.next_token.unwrap_or_default();
        if next.is_empty() {
            break;
        }
    }
    Ok(roles)
}

/// Every account with its roles.
async fn assignments(portal: &Portal<'_>) -> Result<Vec<(Account, Vec<String>)>, Problem> {
    let mut found = Vec::new();
    for account in accounts(portal).await? {
        let roles = roles(portal, &account.id).await?;
        found.push((account, roles));
    }
    Ok(found)
}

/// Fresh credentials for the role `made`, as a secret's values and meta.
async fn credentials(portal: &Portal<'_>, made: &Made) -> Result<Derived, Problem> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Response {
        role_credentials: Credentials,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Credentials {
        access_key_id: String,
        secret_access_key: String,
        session_token: String,
        /// In ms since 1970.
        expiration: i64,
    }
    let query = [("account_id", made.account_id.as_str()), ("role_name", made.role.as_str())];
    let response: Response = get(portal, "/federation/credentials", &query).await.map_err(|problem| match problem {
        Problem::Refused(_) => Problem::Refused(format!(
            "You no longer have the role {} in AWS account {}.",
            made.role,
            Account { id: made.account_id.clone(), name: made.account_name.clone() }.label()
        )),
        other => other,
    })?;
    let credentials = response.role_credentials;
    let variable = |name: &str, value: String| Variable { name: name.to_owned(), value: Zeroizing::new(value) };
    let account = Account { id: made.account_id.clone(), name: made.account_name.clone() };
    Ok(Derived {
        values: Some(vec![
            variable("AWS_ACCESS_KEY_ID", credentials.access_key_id),
            variable("AWS_SECRET_ACCESS_KEY", credentials.secret_access_key),
            variable("AWS_SESSION_TOKEN", credentials.session_token),
            variable("AWS_REGION", portal.session.client.region.clone()),
            variable("AWS_DEFAULT_REGION", portal.session.client.region.clone()),
        ]),
        meta: ExportMeta {
            access: vec![format!("Act as role {} in AWS account {}", made.role, account.label())],
            permissions: Vec::new(),
            resources: vec![format!("{}/{}", made.account_id, made.role)],
            expires_ms: credentials.expiration,
            // Short-lived, renewed for the agent: kept until shortly before
            // they expire, then fetched again before a command gets them.
            per_use: true,
            renew_at_ms: (credentials.expiration - RENEW_BEFORE_MS).max(1),
            recipe: serde_json::to_value(made).unwrap_or_default(),
            // The region isn't secret: hidden, it would hide every ARN's.
            public: vec!["AWS_REGION".to_owned(), "AWS_DEFAULT_REGION".to_owned()],
        },
    })
}

/// The session from `tokens`; `refresh_token` kept when AWS sends no new one.
fn session(client: &Registration, tokens: Tokens, refresh_token: &str) -> Session {
    Session {
        client: client.clone(),
        access_token: tokens.access_token,
        refresh_token: tokens.refresh_token.filter(|token| !token.is_empty()).unwrap_or_else(|| refresh_token.to_owned()),
        expires_ms: now_ms() + tokens.expires_in.unwrap_or(3600) * 1000,
    }
}

impl Recipe for Aws {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    fn start_device<'a>(&'a self, http: &'a Http, start: &'a Start<'a>) -> BoxFuture<'a, Result<Device, Problem>> {
        Box::pin(async move {
            let region = region(start.fields.get("region").map(String::as_str).unwrap_or_default())?;
            let start_url = start_url(start.fields.get("start_url").map(String::as_str).unwrap_or_default())?;
            let oidc = self.oidc(http, &region);
            let client = self.client(http, &oidc).await?;
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct Authorization {
                device_code: String,
                user_code: String,
                #[serde(default)]
                verification_uri: String,
                #[serde(default)]
                verification_uri_complete: String,
                #[serde(default)]
                interval: Option<u64>,
                #[serde(default)]
                expires_in: Option<i64>,
            }
            let ask = json!({"clientId": client.id, "clientSecret": *client.secret, "startUrl": start_url});
            let answered = post(http, &format!("{oidc}/device_authorization"), &ask).await?;
            if matches!(answered, Answer::Error { .. }) {
                // Whatever went wrong, the next try registers afresh.
                guard(&self.clients).remove(&oidc);
            }
            let authorization: Authorization = match answered {
                Answer::Ok(body) => field(body)?,
                Answer::Error { status: 400, code, .. } if code == "invalid_request" || code == "InvalidRequestException" => {
                    return Err(Problem::Refused(format!(
                        "AWS has no access portal {start_url} in {region}. Check both under Settings summary in IAM Identity Center."
                    )));
                }
                Answer::Error { status, code, message } => return Err(said(status, &code, &message)),
            };
            let url = if authorization.verification_uri_complete.is_empty() {
                authorization.verification_uri
            } else {
                authorization.verification_uri_complete
            };
            // A stand-in in the dev Claktory may serve its page over http.
            if !(url.starts_with("https://") || self.stand_in.is_some() && url.starts_with("http://")) {
                return Err(Problem::Unreachable("AWS sent a page to approve it on that isn't https.".to_owned()));
            }
            let pending = Pending {
                client: Registration {
                    region,
                    start_url,
                    client_id: client.id,
                    client_secret: client.secret.to_string(),
                    client_expires_ms: client.expires_ms,
                },
                device_code: authorization.device_code,
            };
            Ok(Device {
                url,
                user_code: authorization.user_code,
                interval: Duration::from_secs(authorization.interval.unwrap_or(5).max(1)),
                expires_ms: authorization.expires_in.map_or(0, |seconds| now_ms() + seconds * 1000),
                pending: Control::from_value(&pending),
            })
        })
    }

    fn poll_device<'a>(&'a self, http: &'a Http, _start: &'a Start<'a>, pending: &'a Control) -> BoxFuture<'a, Result<Poll, Problem>> {
        Box::pin(async move {
            let pending: Pending = pending.parse()?;
            let body = json!({
                "clientId": pending.client.client_id,
                "clientSecret": pending.client.client_secret,
                "grantType": DEVICE_GRANT,
                "deviceCode": pending.device_code,
            });
            match post(http, &format!("{}/token", self.oidc(http, &pending.client.region)), &body).await? {
                Answer::Ok(body) => Ok(Poll::Done(Control::from_value(&session(&pending.client, field(body)?, "")))),
                Answer::Error { code, .. } if code == "authorization_pending" || code == "AuthorizationPendingException" => {
                    Ok(Poll::Pending)
                }
                Answer::Error { code, .. } if code == "slow_down" || code == "SlowDownException" => Ok(Poll::SlowDown),
                Answer::Error { code, .. } if code == "access_denied" || code == "AccessDeniedException" => {
                    Err(Problem::Refused("AWS didn't allow access.".to_owned()))
                }
                Answer::Error { code, .. } if code == "expired_token" || code == "ExpiredTokenException" => {
                    Err(Problem::Refused("The code expired before it was approved. Start again.".to_owned()))
                }
                Answer::Error { status, code, message } => Err(said(status, &code, &message)),
            }
        })
    }

    fn inspect<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>> {
        Box::pin(async move {
            let session: Session = control.parse()?;
            let found = assignments(&self.at(http, &session)).await?;
            let resources = found
                .iter()
                .flat_map(|(account, roles)| roles.iter().map(move |role| format!("AWS account {} · {role}", account.label())))
                .collect();
            let count = found.len();
            let accounts = if count == 1 { "1 account".to_owned() } else { format!("{count} accounts") };
            Ok(Inspected {
                // The portal and its region: the user can't be read.
                subject: format!("{} {}", session.client.start_url, session.client.region),
                account: format!("{} in {}", portal_name(&session.client.start_url), session.client.region),
                access: vec![format!("Get short-lived keys for your roles in {accounts}")],
                scopes: vec![SCOPE.to_owned()],
                resources,
                // With a refresh token it lasts as long as the registration.
                expires_ms: if session.refresh_token.is_empty() { session.expires_ms } else { session.client.client_expires_ms },
            })
        })
    }

    fn refresh<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Option<Control>, Problem>> {
        Box::pin(async move {
            let session: Session = control.parse()?;
            let now = now_ms();
            if now + REFRESH_BEFORE_MS < session.expires_ms {
                return Ok(None);
            }
            if session.refresh_token.is_empty() {
                if now < session.expires_ms {
                    return Ok(None);
                }
                return Err(Problem::Reconnect("Your AWS sign-in ended.".to_owned()));
            }
            if session.client.client_expires_ms > 0 && now >= session.client.client_expires_ms {
                return Err(Problem::Reconnect("AWS ends this sign-in after 90 days.".to_owned()));
            }
            let body = json!({
                "clientId": session.client.client_id,
                "clientSecret": session.client.client_secret,
                "grantType": "refresh_token",
                "refreshToken": session.refresh_token,
            });
            match post(http, &format!("{}/token", self.oidc(http, &session.client.region)), &body).await? {
                Answer::Ok(body) => Ok(Some(Control::from_value(&self::session(&session.client, field(body)?, &session.refresh_token)))),
                Answer::Error { status, code, message } if status >= 500 || status == 429 => Err(said(status, &code, &message)),
                // invalid_grant, invalid_client, expired_token: the grant is gone.
                Answer::Error { .. } => Err(Problem::Reconnect("AWS no longer accepts its sign-in.".to_owned())),
            }
        })
    }

    fn choices<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Choices, Problem>> {
        Box::pin(async move {
            let session: Session = control.parse()?;
            let mut choices = Vec::new();
            for (account, roles) in assignments(&self.at(http, &session)).await? {
                choices.push(Choice { kind: "account".into(), id: account.id.clone(), label: account.label(), ..Choice::default() });
                choices.extend(roles.into_iter().map(|role| Choice {
                    kind: "role".into(),
                    id: format!("{}/{role}", account.id),
                    label: role,
                    parent: account.id.clone(),
                    why: String::new(),
                }));
            }
            Ok(Choices {
                choices,
                rules: "Choose one role. The secret can do exactly what the role's permission set in IAM Identity Center allows, and nothing more.".into(),
            })
        })
    }

    fn derive<'a>(&'a self, http: &'a Http, control: &'a Control, ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            if !ask.permissions.is_empty() {
                return Err(Problem::Refused(
                    "AWS has no permissions to choose. A role's permission set is all its access, so choose one role.".to_owned(),
                ));
            }
            let asked: Vec<(&str, &str)> = ask.resources.iter().filter_map(|id| id.split_once('/')).collect();
            let ([(account_id, role)], 1) = (asked.as_slice(), ask.resources.len()) else {
                return Err(Problem::Refused("Choose exactly one role.".to_owned()));
            };
            let session: Session = control.parse()?;
            let portal = self.at(http, &session);
            let Some(account) = accounts(&portal).await?.into_iter().find(|account| account.id == *account_id) else {
                return Err(Problem::Refused(format!("{account_id} isn't one of your AWS accounts.")));
            };
            if !roles(&portal, &account.id).await?.iter().any(|name| name == role) {
                return Err(Problem::Refused(format!("You have no role {role} in AWS account {}.", account.label())));
            }
            let made = Made { account_id: account.id, account_name: account.name, role: (*role).to_owned() };
            credentials(&portal, &made).await
        })
    }

    fn renew<'a>(&'a self, http: &'a Http, control: &'a Control, export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            let made: Made = serde_json::from_value(export.recipe.clone())
                .map_err(|_| Problem::Refused("Claktory can't tell which AWS role this secret is for. Make it again.".to_owned()))?;
            let session: Session = control.parse()?;
            credentials(&self.at(http, &session), &made).await
        })
    }

    /// Signs the session out at AWS, which ends its tokens.
    fn revoke<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async move {
            let session: Session = control.parse()?;
            let url = format!("{}/logout", self.portal(http, &session.client.region));
            let response =
                http.client().post(url).header("x-amz-sso_bearer_token", &session.access_token).send().await.map_err(unreachable)?;
            match answer(response).await? {
                // Signed out, or no longer signed in.
                Answer::Ok(_) | Answer::Error { status: 401, .. } => Ok(()),
                Answer::Error { status, code, message } => Err(said(status, &code, &message)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regions_and_portals_are_read_as_the_person_gives_them() {
        assert_eq!(region(" eu-west-1 ").ok(), Some("eu-west-1".to_owned()));
        for bad in ["", "us", "EU-WEST-1", "evil.com/x-1", "us-east-1.evil.com"] {
            assert!(region(bad).is_err(), "{bad}");
        }
        for (given, read) in [
            ("https://d-1234567890.awsapps.com/start/", "https://d-1234567890.awsapps.com/start"),
            ("d-1234567890.awsapps.com/start", "https://d-1234567890.awsapps.com/start"),
            ("d-1234567890", "https://d-1234567890.awsapps.com/start"),
            ("https://d-1234567890.awsapps.com", "https://d-1234567890.awsapps.com/start"),
            ("https://ssoins-1234.portal.us-east-1.app.aws", "https://ssoins-1234.portal.us-east-1.app.aws"),
        ] {
            assert_eq!(start_url(given).ok().as_deref(), Some(read), "{given}");
        }
        for bad in ["", "http://d-1.awsapps.com/start", "https://user@evil.example/start", "https://"] {
            assert!(start_url(bad).is_err(), "{bad}");
        }
        assert_eq!(portal_name("https://d-1234567890.awsapps.com/start"), "d-1234567890");
        let day = 24 * 60 * 60 * 1000;
        let client = |expires_ms| Client { id: "c".into(), secret: Zeroizing::new("s".into()), expires_ms };
        assert!(reusable(&client(90 * day), 0), "new: reused");
        assert!(!reusable(&client(90 * day), 31 * day), "a connection made with it would end within 60 days");
        assert!(!reusable(&client(0), 0), "no expiry said: not reused");
        assert_eq!(portal_name("https://ssoins-1234.portal.us-east-1.app.aws"), "ssoins-1234.portal.us-east-1.app.aws");
    }
}
