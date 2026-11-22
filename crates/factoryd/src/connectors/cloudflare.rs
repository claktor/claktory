//! Cloudflare: the person makes an API token on Cloudflare's page, with
//! what Claktory needs already chosen, and pastes it (or, with an OAuth
//! client, `FACTORY_CLOUDFLARE_CLIENT_ID`, approves Claktory's public client
//! on Cloudflare's consent screen: authorization code with PKCE, no client
//! secret). That token or grant never reaches an agent: it stays in the
//! vault, and with it Claktory mints each secret its own account-owned API
//! token, as narrow as the work needs (these permissions, on this account
//! or these zones, until then), extends it while it's used, and deletes it
//! when the secret goes.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::{
    AccessSpec, BoxFuture, Choice, Choices, Control, Derive, Derived, ExportMeta, FlowKind, Http, Inspected, Problem, Recipe, Spec, Start,
    Variable, setting as set,
};
use crate::transcript::now_ms;

/// Claktory's OAuth client at Cloudflare (public: PKCE, no secret). Empty
/// until it's registered; `FACTORY_CLOUDFLARE_CLIENT_ID` sets one meanwhile.
const CLIENT_ID: &str = "";

/// What Claktory asks of Cloudflare: to make API tokens in the person's
/// account, and to read its accounts and zones to offer them. The ids are
/// the client's scopes as registered at Cloudflare.
const SCOPES: &[(&str, &str)] = &[
    ("account-api-tokens.write", "Make and delete API tokens in your accounts"),
    ("account-settings.read", "See your account names"),
    ("zone.read", "See your zones (domains)"),
];

/// Whether Cloudflare connects by a pasted token in this version; until the
/// app takes it, only the dev Claktory offers it (`FACTORY_CLOUDFLARE_BASE`
/// or `FACTORY_CLOUDFLARE_ENABLED`).
const SHIPPED: bool = true;

/// What the token the person makes may do, as Cloudflare's page for an
/// account-owned token takes it (`permissionGroupKeys`, see
/// developers.cloudflare.com/fundamentals/api/how-to/account-owned-token-template/):
/// make API tokens in the account and read its name and zones, to offer
/// them; and the permissions agents most often get, which a token it makes
/// can then carry. The page fills these in; zones are chosen there.
const TOKEN_PERMISSIONS: &[(&str, &str, &str)] = &[
    ("account_api_tokens", "edit", "Make and delete API tokens in your account"),
    ("account_settings", "read", "See your account names"),
    ("zone", "read", "See your zones (domains)"),
    ("dns", "edit", "Edit DNS"),
    ("workers_scripts", "edit", "Edit Workers"),
    ("page", "edit", "Edit Pages"),
];

/// A secret made from it lasts this long unless asked otherwise…
const LIFETIME_MS: i64 = 30 * 24 * 3600 * 1000;
/// …and is extended this long before it ends, by a command that uses it.
const RENEW_BEFORE_MS: i64 = 7 * 24 * 3600 * 1000;
/// Tokens are refreshed this long before Cloudflare says they end.
const REFRESH_MARGIN_MS: i64 = 60 * 1000;

const DASH: (&str, &str) = ("cloudflare.dash", "https://dash.cloudflare.com");
const API: (&str, &str) = ("cloudflare.api", "https://api.cloudflare.com/client/v4");

const ACCOUNT_RESOURCE: &str = "com.cloudflare.api.account";
const ZONE_RESOURCE: &str = "com.cloudflare.api.account.zone";

pub struct Cloudflare {
    spec: Spec,
    /// The OAuth client; empty when the person pastes a token instead.
    client_id: String,
    /// Where Cloudflare's sign-in and API are, unless a test says otherwise
    /// (`Http::base`): Cloudflare's own, or a stand-in in the dev Claktory.
    dash_base: String,
    api_base: String,
}

impl Cloudflare {
    /// The recipe with OAuth client `client_id`.
    pub fn new(client_id: &str) -> Cloudflare {
        let scopes: Vec<String> = SCOPES.iter().map(|(scope, _)| (*scope).to_owned()).collect();
        Cloudflare {
            spec: Spec {
                id: "cloudflare".to_owned(),
                name: "Cloudflare".to_owned(),
                description: "Give each agent its own Cloudflare API token, with only the permissions and zones you choose."
                    .to_owned(),
                accesses: vec![AccessSpec {
                    id: "tokens".to_owned(),
                    label: "Make API tokens for agents".to_owned(),
                    why: "Claktory makes tokens with only what each agent's work needs, renews them while they're in use, and deletes each one at Cloudflare when you delete its secret or disconnect. It also reads your account and zone names so you can choose."
                        .to_owned(),
                    scopes,
                    preselected: true,
                }],
                fields: Vec::new(),
                flow: FlowKind::Browser,
                redirect_uris: vec![
                    "http://127.0.0.1:{port}/oauth/cloudflare".to_owned(),
                    "com.claktory.app:/oauth/cloudflare".to_owned(),
                    "com.claktory.app.dev:/oauth/cloudflare".to_owned(),
                ],
                variables: vec!["CLOUDFLARE_API_TOKEN".to_owned(), "CLOUDFLARE_ACCOUNT_ID".to_owned()],
                setup_note: String::new(),
                setup_url: String::new(),
                token_url: String::new(),
            },
            client_id: client_id.to_owned(),
            dash_base: DASH.1.to_owned(),
            api_base: API.1.to_owned(),
        }
    }

    /// The recipe that takes a token the person makes on Cloudflare's page
    /// and pastes: no OAuth client needed.
    pub fn pasted() -> Cloudflare {
        let mut recipe = Cloudflare::new("");
        recipe.spec.flow = FlowKind::Paste;
        recipe.spec.redirect_uris.clear();
        let access = &mut recipe.spec.accesses[0];
        access.why = "Claktory makes tokens with only what each agent's work needs, renews them while they're in use, and deletes each one at Cloudflare when you delete its secret or disconnect. It also reads your account and zone names so you can choose."
            .to_owned();
        access.scopes = TOKEN_PERMISSIONS.iter().map(|(_, _, said)| (*said).to_owned()).collect();
        recipe.spec.setup_note = "Cloudflare's page opens with the permissions Claktory needs already chosen. Under Zone Resources, choose All zones or the zones agents may work on, then Continue to summary and Create Token. Copy the token and paste it here."
            .to_owned();
        recipe.set_dash(DASH.1);
        recipe
    }

    /// Where Cloudflare's dashboard is, and so the page that makes the token.
    fn set_dash(&mut self, base: &str) {
        self.dash_base = base.to_owned();
        if self.spec.flow == FlowKind::Paste {
            self.spec.token_url = token_url(base);
        }
    }

    /// The built-in recipe, once Claktory has a client at Cloudflare. Until
    /// then `FACTORY_CLOUDFLARE_CLIENT_ID` gives one; and in the dev Claktory
    /// only, `FACTORY_CLOUDFLARE_BASE` puts a stand-in in Cloudflare's place
    /// (its sign-in at `<base>/oauth2/…`, its API at `<base>/client/v4`), to
    /// try the whole flow without Cloudflare.
    pub(crate) fn built_in() -> Option<Cloudflare> {
        let dev = std::env::args().any(|arg| arg == "--dev");
        let stand_in = set("FACTORY_CLOUDFLARE_BASE").filter(|_| dev).map(|base| base.trim_end_matches('/').to_owned());
        let client_id = set("FACTORY_CLOUDFLARE_CLIENT_ID").unwrap_or_else(|| CLIENT_ID.to_owned());
        let mut recipe = if client_id.is_empty() {
            let enabled = dev && set("FACTORY_CLOUDFLARE_ENABLED").is_some_and(|value| value != "0");
            if !(SHIPPED || enabled || stand_in.is_some()) {
                return None;
            }
            Cloudflare::pasted()
        } else {
            Cloudflare::new(&client_id)
        };
        if let Some(base) = stand_in {
            tracing::warn!(%base, "Cloudflare is played by a stand-in");
            recipe.set_dash(&base);
            recipe.api_base = format!("{base}/client/v4");
        }
        Some(recipe)
    }

    /// Where its sign-in is.
    fn dash(&self, http: &Http) -> String {
        http.base(DASH.0, &self.dash_base)
    }

    /// Where its API is.
    fn api(&self, http: &Http) -> String {
        http.base(API.0, &self.api_base)
    }
}

// ---- What's kept ----

/// The connection's grant (broker-only).
#[derive(Serialize, Deserialize)]
struct Grant {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    /// When the access token ends, in ms since 1970.
    #[serde(default)]
    expires_ms: i64,
    #[serde(default)]
    scopes: Vec<String>,
    /// A token the person pasted, rather than an OAuth grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pasted: Option<Pasted>,
}

/// Which token was pasted: its id, and the account that owns it (empty for
/// one the person owns).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Pasted {
    id: String,
    #[serde(default)]
    account: String,
}

impl Pasted {
    /// Where it is in Cloudflare's API.
    fn path(&self) -> String {
        if self.account.is_empty() { format!("/user/tokens/{}", self.id) } else { format!("/accounts/{}/tokens/{}", self.account, self.id) }
    }
}

/// What a secret made from it needs to be renewed and deleted (kept in
/// ExportMeta.recipe; no secret in it).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Minted {
    account: String,
    token_id: String,
    name: String,
    policy: Policy,
    /// How long each renewal makes it last.
    lifetime_ms: i64,
}

/// What a token may do: account-level permissions on the account, and
/// zone-level ones on some zones (none: every zone of the account).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
struct Policy {
    account_permissions: Vec<String>,
    zone_permissions: Vec<String>,
    zones: Vec<String>,
}

// ---- Cloudflare's answers ----

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct Envelope<T> {
    #[serde(default)]
    success: bool,
    #[serde(default)]
    errors: Vec<ApiError>,
    result: Option<T>,
    #[serde(default)]
    result_info: Option<ResultInfo>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    code: i64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ResultInfo {
    #[serde(default)]
    page: u32,
    #[serde(default)]
    total_pages: u32,
}

#[derive(Clone, Deserialize)]
struct Account {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Deserialize)]
struct Zone {
    id: String,
    #[serde(default)]
    name: String,
}

#[derive(Clone, Deserialize)]
struct PermissionGroup {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    scopes: Vec<String>,
}

impl PermissionGroup {
    /// Whether it applies to zones (else to the account).
    fn zone_level(&self) -> bool {
        self.scopes.iter().any(|scope| scope == ZONE_RESOURCE)
    }

    /// Whether an account-owned token can carry it.
    fn for_accounts(&self) -> bool {
        self.scopes.iter().any(|scope| scope == ZONE_RESOURCE || scope == ACCOUNT_RESOURCE)
    }
}

/// Why a call to Cloudflare didn't work.
enum Failure {
    Network(String),
    /// Its HTTP status and what it said.
    Said(u16, String),
}

impl Failure {
    /// In words for the person, for something Claktory set out `doing` ("read your accounts").
    fn problem(self, doing: &str) -> Problem {
        match self {
            Failure::Network(error) => {
                Problem::Unreachable(format!("Couldn't reach Cloudflare to {doing}: {}.", error.trim_end_matches('.')))
            }
            Failure::Said(401, _) => Problem::Reconnect("Cloudflare no longer accepts Claktory's access.".to_owned()),
            Failure::Said(403, said) => Problem::Refused(format!(
                "Cloudflare didn't let Claktory {doing}: {said}. Your role in that Cloudflare account may not allow it, or the connection may not have that access. Connect Cloudflare again and allow it."
            )),
            Failure::Said(_, said) => Problem::Refused(format!("Cloudflare refused to {doing}: {said}.")),
        }
    }
}

fn network(error: reqwest::Error) -> Failure {
    Failure::Network(error.without_url().to_string())
}

// ---- Talking to Cloudflare ----

impl Cloudflare {
    async fn token(&self, http: &Http, form: &[(&str, &str)]) -> Result<Grant, Failure> {
        let url = format!("{}/oauth2/token", self.dash(http));
        let response = http.client().post(url).form(form).send().await.map_err(network)?;
        let status = response.status();
        let text = response.text().await.map_err(network)?;
        if !status.is_success() {
            let error: Value = serde_json::from_str(&text).unwrap_or_default();
            let code = error["error"].as_str().unwrap_or_default();
            let description = error["error_description"].as_str().unwrap_or_default().trim().trim_end_matches('.');
            let said = match (code, description) {
                ("", _) => format!("HTTP {}", status.as_u16()),
                (code, "") => code.to_owned(),
                (_, description) => description.to_owned(),
            };
            // An OAuth error is a 400 whatever it is; invalid_grant means the grant is gone.
            let status = if code == "invalid_grant" { 401 } else { status.as_u16() };
            return Err(Failure::Said(status, said));
        }
        let tokens: TokenResponse = serde_json::from_str(&text).map_err(|error| Failure::Said(status.as_u16(), error.to_string()))?;
        Ok(Grant {
            access_token: tokens.access_token,
            refresh_token: tokens.refresh_token.unwrap_or_default(),
            expires_ms: now_ms() + tokens.expires_in.unwrap_or(3600).max(0) * 1000,
            scopes: tokens.scope.unwrap_or_default().split_whitespace().map(str::to_owned).collect(),
            pasted: None,
        })
    }

    /// `result` from Cloudflare's envelope, and its page info.
    async fn send<T: serde::de::DeserializeOwned>(request: reqwest::RequestBuilder) -> Result<(T, Option<ResultInfo>), Failure> {
        let response = request.send().await.map_err(network)?;
        let status = response.status();
        let text = response.text().await.map_err(network)?;
        let envelope: Option<Envelope<T>> = serde_json::from_str(&text).ok();
        let said = |envelope: Option<&Envelope<T>>| {
            let errors: Vec<String> = envelope
                .map(|envelope| {
                    envelope.errors.iter().map(|error| format!("{} ({})", error.message.trim_end_matches('.'), error.code)).collect()
                })
                .unwrap_or_default();
            if errors.is_empty() { format!("HTTP {}", status.as_u16()) } else { errors.join("; ") }
        };
        match envelope {
            Some(envelope) if status.is_success() && envelope.success => match envelope.result {
                Some(result) => Ok((result, envelope.result_info)),
                None => Err(Failure::Said(status.as_u16(), "it sent no result".to_owned())),
            },
            envelope => {
                let code = if status.is_success() { 400 } else { status.as_u16() };
                Err(Failure::Said(code, said(envelope.as_ref())))
            }
        }
    }

    /// Every page of a list.
    async fn pages<T: serde::de::DeserializeOwned>(
        &self,
        http: &Http,
        token: &str,
        path: &str,
        query: &[(&str, &str)],
    ) -> Result<Vec<T>, Failure> {
        let mut all = Vec::new();
        for page in 1..=50u32 {
            let page_text = page.to_string();
            let mut query = query.to_vec();
            query.push(("page", &page_text));
            query.push(("per_page", "50"));
            let url = format!("{}{path}", self.api(http));
            let (items, info): (Vec<T>, _) = Cloudflare::send(http.client().get(url).bearer_auth(token).query(&query)).await?;
            let more = info.is_some_and(|info: ResultInfo| info.total_pages > info.page.max(page));
            all.extend(items);
            if !more {
                break;
            }
        }
        Ok(all)
    }

    async fn accounts(&self, http: &Http, token: &str) -> Result<Vec<Account>, Problem> {
        self.pages(http, token, "/accounts", &[]).await.map_err(|failure| failure.problem("read your accounts"))
    }

    async fn zones(&self, http: &Http, token: &str, account: &str) -> Result<Vec<Zone>, Problem> {
        self.pages(http, token, "/zones", &[("account.id", account)]).await.map_err(|failure| failure.problem("read your zones"))
    }

    /// The permissions an account-owned token can carry.
    async fn permission_groups(&self, http: &Http, token: &str, account: &str) -> Result<Vec<PermissionGroup>, Problem> {
        let groups: Vec<PermissionGroup> = self
            .pages(http, token, &format!("/accounts/{account}/tokens/permission_groups"), &[])
            .await
            .map_err(|failure| failure.problem("read the permissions a token can have"))?;
        Ok(groups.into_iter().filter(PermissionGroup::for_accounts).collect())
    }

    /// Mints a token that can do only `minted.policy` until `expires_ms`.
    async fn mint(&self, http: &Http, grant: &Grant, minted: &Minted, expires_ms: i64) -> Result<(String, Zeroizing<String>), Problem> {
        let token = grant.access_token.as_str();
        #[derive(Deserialize)]
        struct Created {
            id: String,
            value: String,
        }
        let url = format!("{}/accounts/{}/tokens", self.api(http), minted.account);
        let body = token_body(minted, expires_ms);
        let (created, _): (Created, _) =
            Cloudflare::send(http.client().post(url).bearer_auth(token).json(&body)).await.map_err(|failure| match failure {
                Failure::Said(403, _) if grant.pasted.is_some() => Problem::Refused(
                    "The token you pasted can't make tokens for agents: it needs the permission Account API Tokens: Edit. Connect Cloudflare again, and make a new token on the page Claktory opens."
                        .to_owned(),
                ),
                failure => failure.problem("make an API token"),
            })?;
        Ok((created.id, Zeroizing::new(created.value)))
    }
}

impl Cloudflare {
    /// What a pasted token is: its accounts, and what Cloudflare says it may
    /// do, when it lets the token read itself.
    async fn inspect_pasted(&self, http: &Http, token: &str, pasted: &Pasted, accounts: &[Account]) -> Inspected {
        #[derive(Deserialize)]
        struct Policy {
            #[serde(default)]
            permission_groups: Vec<PermissionGroup>,
        }
        #[derive(Deserialize)]
        struct Itself {
            #[serde(default)]
            policies: Vec<Policy>,
            #[serde(default)]
            expires_on: Option<String>,
        }
        let url = format!("{}{}", self.api(http), pasted.path());
        let itself: Option<Itself> = Cloudflare::send(http.client().get(url).bearer_auth(token)).await.ok().map(|(itself, _)| itself);
        let mut access: Vec<String> = Vec::new();
        for group in itself.iter().flat_map(|itself| &itself.policies).flat_map(|policy| &policy.permission_groups) {
            if !group.name.is_empty() && !access.contains(&group.name) {
                access.push(group.name.clone());
            }
        }
        if access.is_empty() {
            access = self.spec.accesses[0].scopes.clone();
        }
        let expires_ms = itself
            .and_then(|itself| itself.expires_on)
            .and_then(|time| time.parse::<jiff::Timestamp>().ok())
            .map_or(0, |time| time.as_millisecond());
        let mut ids: Vec<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
        ids.sort_unstable();
        let names: Vec<&str> =
            accounts.iter().map(|account| if account.name.is_empty() { account.id.as_str() } else { account.name.as_str() }).collect();
        Inspected {
            subject: ids.join(","),
            account: names.join(", "),
            scopes: access.clone(),
            access,
            resources: names.iter().map(|name| format!("Cloudflare account {name}")).collect(),
            expires_ms,
        }
    }
}

/// Cloudflare's page that makes an account-owned token, with Claktory's
/// permissions and name filled in.
fn token_url(dash: &str) -> String {
    let keys: Vec<Value> = TOKEN_PERMISSIONS.iter().map(|(key, kind, _)| json!({"key": key, "type": kind})).collect();
    let mut url = reqwest::Url::parse(&format!("{}/", dash.trim_end_matches('/'))).expect("a URL");
    url.query_pairs_mut()
        .append_pair("to", "/:account/api-tokens")
        .append_pair("permissionGroupKeys", &Value::Array(keys).to_string())
        .append_pair("name", "Claktory");
    // As Cloudflare's own links write it: "/:account/…" unescaped.
    String::from(url).replace("to=%2F%3Aaccount%2Fapi-tokens", "to=/:account/api-tokens")
}

/// The token pasted, as Cloudflare checks it: (its id, its status, when it
/// ends in ms since 1970 or 0).
#[derive(Deserialize)]
struct Verified {
    id: String,
    #[serde(default)]
    status: String,
    #[serde(default)]
    expires_on: Option<String>,
}

/// An account-owned token's name, policies and expiry, as Cloudflare takes them.
fn token_body(minted: &Minted, expires_ms: i64) -> Value {
    let policy = &minted.policy;
    let account = format!("{ACCOUNT_RESOURCE}.{}", minted.account);
    let groups = |ids: &[String]| ids.iter().map(|id| json!({"id": id})).collect::<Vec<Value>>();
    let mut policies = Vec::new();
    if !policy.account_permissions.is_empty() {
        policies.push(
            json!({"effect": "allow", "permission_groups": groups(&policy.account_permissions), "resources": {account.clone(): "*"}}),
        );
    }
    if !policy.zone_permissions.is_empty() {
        let resources: serde_json::Map<String, Value> = if policy.zones.is_empty() {
            [(account, json!({format!("{ZONE_RESOURCE}.*"): "*"}))].into_iter().collect()
        } else {
            policy.zones.iter().map(|zone| (format!("{ZONE_RESOURCE}.{zone}"), json!("*"))).collect()
        };
        policies.push(json!({"effect": "allow", "permission_groups": groups(&policy.zone_permissions), "resources": resources}));
    }
    json!({"name": minted.name, "policies": policies, "expires_on": rfc3339(expires_ms)})
}

/// A scope in words: Claktory's own as it explains them, any other the way
/// Cloudflare names its permissions ("account.read": "Account Read").
fn scope_words(scope: &str) -> String {
    if let Some((_, said)) = SCOPES.iter().find(|(known, _)| *known == scope) {
        return (*said).to_owned();
    }
    let words: Vec<String> = scope
        .split(['.', '-', '_', ':'])
        .filter(|word| !word.is_empty())
        .map(|word| {
            let mut letters = word.chars();
            letters.next().map(|first| first.to_uppercase().chain(letters).collect()).unwrap_or_default()
        })
        .collect();
    if words.is_empty() { scope.to_owned() } else { words.join(" ") }
}

/// `ms` since 1970 as Cloudflare writes times: "2026-12-01T00:00:00Z".
fn rfc3339(ms: i64) -> String {
    jiff::Timestamp::from_second(ms.div_euclid(1000)).map(|time| time.to_string()).unwrap_or_default()
}

/// When a secret made to end at `expires_ms` is renewed: a week before, or
/// halfway for a shorter one.
fn renew_at(expires_ms: i64, lifetime_ms: i64) -> i64 {
    expires_ms - RENEW_BEFORE_MS.min(lifetime_ms / 2)
}

// ---- The recipe ----

impl Recipe for Cloudflare {
    fn spec(&self) -> &Spec {
        &self.spec
    }

    fn authorize_url(&self, start: &Start<'_>) -> Result<String, Problem> {
        let mut url =
            reqwest::Url::parse(&format!("{}/oauth2/auth", self.dash_base)).map_err(|error| Problem::Refused(error.to_string()))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.client_id)
            .append_pair("redirect_uri", start.redirect_uri)
            .append_pair("scope", &start.access.scopes.join(" "))
            .append_pair("state", start.state)
            .append_pair("code_challenge", start.challenge)
            .append_pair("code_challenge_method", "S256");
        Ok(url.into())
    }

    fn exchange<'a>(
        &'a self,
        http: &'a Http,
        start: &'a Start<'a>,
        code: &'a str,
        verifier: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async move {
            let form = [
                ("grant_type", "authorization_code"),
                ("client_id", self.client_id.as_str()),
                ("code", code),
                ("code_verifier", verifier),
                ("redirect_uri", start.redirect_uri),
            ];
            let grant = self.token(http, &form).await.map_err(|failure| match failure {
                Failure::Network(_) => failure.problem("sign in"),
                Failure::Said(_, said) => Problem::Refused(format!("Cloudflare didn't accept the sign-in: {said}. Start again.")),
            })?;
            Ok(Control::from_value(&grant))
        })
    }

    fn paste<'a>(
        &'a self,
        http: &'a Http,
        _start: &'a Start<'a>,
        _pending: &'a Control,
        token: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async move {
            let refused =
                || Problem::Refused("Cloudflare didn't accept that token. Check that you copied all of it, or make a new one.".to_owned());
            let api = self.api(http);
            // An account's token (what Cloudflare's page makes), checked in
            // its account…
            let mut found: Option<(Verified, String)> = None;
            match self.pages::<Account>(http, token, "/accounts", &[]).await {
                Ok(accounts) => {
                    for account in accounts {
                        let url = format!("{api}/accounts/{}/tokens/verify", account.id);
                        if let Ok((verified, _)) = Cloudflare::send::<Verified>(http.client().get(url).bearer_auth(token)).await {
                            found = Some((verified, account.id));
                            break;
                        }
                    }
                }
                Err(Failure::Network(error)) => return Err(Failure::Network(error).problem("check the token")),
                Err(Failure::Said(..)) => {}
            }
            // …or one the person owns.
            if found.is_none() {
                match Cloudflare::send::<Verified>(http.client().get(format!("{api}/user/tokens/verify")).bearer_auth(token)).await {
                    Ok((verified, _)) => found = Some((verified, String::new())),
                    Err(Failure::Network(error)) => return Err(Failure::Network(error).problem("check the token")),
                    Err(Failure::Said(..)) => {}
                }
            }
            let Some((verified, account)) = found else { return Err(refused()) };
            if !verified.status.is_empty() && verified.status != "active" {
                return Err(Problem::Refused(format!("That token is {} at Cloudflare. Make a new one.", verified.status)));
            }
            let expires_ms = verified
                .expires_on
                .as_deref()
                .and_then(|time| time.parse::<jiff::Timestamp>().ok())
                .map_or(0, |time| time.as_millisecond());
            if expires_ms != 0 && expires_ms <= now_ms() {
                return Err(Problem::Refused("That token has expired at Cloudflare. Make a new one.".to_owned()));
            }
            Ok(Control::from_value(&Grant {
                access_token: token.to_owned(),
                refresh_token: String::new(),
                expires_ms: 0,
                scopes: Vec::new(),
                pasted: Some(Pasted { id: verified.id, account }),
            }))
        })
    }

    fn inspect<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            let accounts = self.accounts(http, &grant.access_token).await?;
            if accounts.is_empty() {
                return Err(Problem::Refused("Cloudflare didn't share an account with Claktory. Connect again and choose one.".to_owned()));
            }
            if let Some(pasted) = &grant.pasted {
                return Ok(self.inspect_pasted(http, &grant.access_token, pasted, &accounts).await);
            }
            // Who signed in, if Cloudflare says; else the accounts tell it apart.
            let user: Value = async {
                let url = format!("{}/oauth2/userinfo", self.dash(http));
                let response = http.client().get(url).bearer_auth(&grant.access_token).send().await.ok()?;
                if !response.status().is_success() {
                    return None;
                }
                response.json().await.ok()
            }
            .await
            .unwrap_or_default();
            let mut ids: Vec<&str> = accounts.iter().map(|account| account.id.as_str()).collect();
            ids.sort_unstable();
            let subject = user["sub"].as_str().filter(|sub| !sub.is_empty()).map_or_else(|| ids.join(","), str::to_owned);
            let names: Vec<&str> =
                accounts.iter().map(|account| if account.name.is_empty() { account.id.as_str() } else { account.name.as_str() }).collect();
            let account = user["email"].as_str().filter(|email| !email.is_empty()).map_or_else(|| names.join(", "), str::to_owned);
            let access = grant.scopes.iter().map(|scope| scope_words(scope)).collect();
            Ok(Inspected {
                subject,
                account,
                access,
                scopes: grant.scopes.clone(),
                resources: names.iter().map(|name| format!("Cloudflare account {name}")).collect(),
                // Refreshed while it's used; Cloudflare says no end for the grant.
                expires_ms: 0,
            })
        })
    }

    fn refresh<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Option<Control>, Problem>> {
        // A pasted token isn't refreshed: it lasts until the person ends it.
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            if grant.expires_ms == 0 || now_ms() < grant.expires_ms - REFRESH_MARGIN_MS {
                return Ok(None);
            }
            if grant.refresh_token.is_empty() {
                return Err(Problem::Reconnect("Its sign-in ended and can't be renewed.".to_owned()));
            }
            let form =
                [("grant_type", "refresh_token"), ("client_id", self.client_id.as_str()), ("refresh_token", grant.refresh_token.as_str())];
            let mut fresh = self.token(http, &form).await.map_err(|failure| failure.problem("renew Claktory's access"))?;
            // Cloudflare rotates refresh tokens; keep the old one if it sent none.
            if fresh.refresh_token.is_empty() {
                fresh.refresh_token = grant.refresh_token.clone();
            }
            if fresh.scopes.is_empty() {
                fresh.scopes = grant.scopes.clone();
            }
            Ok(Some(Control::from_value(&fresh)))
        })
    }

    fn choices<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Choices, Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            let token = grant.access_token.as_str();
            let accounts = self.accounts(http, token).await?;
            let mut choices = Vec::new();
            let mut permissions: BTreeMap<String, Choice> = BTreeMap::new();
            for account in &accounts {
                choices.push(Choice {
                    kind: "account".to_owned(),
                    id: account.id.clone(),
                    label: account.name.clone(),
                    parent: String::new(),
                    why: String::new(),
                });
                for zone in self.zones(http, token, &account.id).await? {
                    choices.push(Choice {
                        kind: "zone".to_owned(),
                        id: zone.id,
                        label: zone.name,
                        parent: account.id.clone(),
                        why: String::new(),
                    });
                }
                for group in self.permission_groups(http, token, &account.id).await? {
                    let why = if group.zone_level() {
                        "Applies to the zones you choose, or all of them."
                    } else {
                        "Applies to the whole account."
                    };
                    permissions.entry(group.id.clone()).or_insert(Choice {
                        kind: "permission".to_owned(),
                        id: group.id,
                        label: group.name,
                        parent: String::new(),
                        why: why.to_owned(),
                    });
                }
            }
            choices.extend(permissions.into_values());
            Ok(Choices {
                choices,
                rules: "Choose one account and at least one permission. Zone permissions apply to the zones you choose, or to every zone in the account if you choose none. The token lasts 30 days unless you set an end date, and is extended while it's in use."
                    .to_owned(),
            })
        })
    }

    fn derive<'a>(&'a self, http: &'a Http, control: &'a Control, ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            let token = grant.access_token.as_str();
            let accounts = self.accounts(http, token).await?;
            let chosen: Vec<&Account> = accounts.iter().filter(|account| ask.resources.contains(&account.id)).collect();
            let [account] = chosen.as_slice() else {
                return Err(Problem::Refused(format!(
                    "Choose exactly one account (one of: {}), plus any of its zones to limit it to.",
                    accounts.iter().map(|account| format!("\"{}\" {}", account.id, account.name)).collect::<Vec<_>>().join(", ")
                )));
            };
            let zones = self.zones(http, token, &account.id).await?;
            let mut picked_zones = Vec::new();
            for resource in ask.resources.iter().filter(|resource| **resource != account.id) {
                match zones.iter().find(|zone| zone.id == *resource) {
                    Some(zone) => picked_zones.push(zone.clone()),
                    None => {
                        return Err(Problem::Refused(format!("\"{resource}\" isn't a zone in the {} account.", account.name)));
                    }
                }
            }
            if ask.permissions.is_empty() {
                return Err(Problem::Refused("Choose at least one permission.".to_owned()));
            }
            let groups = self.permission_groups(http, token, &account.id).await?;
            let mut picked = Vec::new();
            for permission in &ask.permissions {
                match groups.iter().find(|group| group.id == *permission) {
                    Some(group) => picked.push(group.clone()),
                    None => return Err(Problem::Refused(format!("\"{permission}\" isn't a permission a Cloudflare token can have."))),
                }
            }
            let policy = Policy {
                account_permissions: picked.iter().filter(|group| !group.zone_level()).map(|group| group.id.clone()).collect(),
                zone_permissions: picked.iter().filter(|group| group.zone_level()).map(|group| group.id.clone()).collect(),
                zones: picked_zones.iter().map(|zone| zone.id.clone()).collect(),
            };
            if !policy.zones.is_empty() && policy.zone_permissions.is_empty() {
                return Err(Problem::Refused("Zones apply only to zone permissions. Choose a zone permission, or no zones.".to_owned()));
            }
            let now = now_ms();
            if ask.expires_ms != 0 && ask.expires_ms <= now {
                return Err(Problem::Refused("That end date has passed. Choose a later one, or none for 30 days.".to_owned()));
            }
            let lifetime_ms = if ask.expires_ms > 0 { ask.expires_ms - now } else { LIFETIME_MS };
            let expires_ms = now + lifetime_ms;
            let name = if ask.name.is_empty() { "Claktory".to_owned() } else { format!("Claktory: {}", ask.name) };
            let mut minted = Minted { account: account.id.clone(), token_id: String::new(), name, policy, lifetime_ms };
            let (token_id, value) = self.mint(http, &grant, &minted, expires_ms).await?;
            minted.token_id = token_id;
            let account_name = if account.name.is_empty() { account.id.clone() } else { account.name.clone() };
            let reach = if picked_zones.is_empty() {
                format!("every zone of {account_name}")
            } else {
                picked_zones.iter().map(|zone| zone.name.as_str()).collect::<Vec<_>>().join(", ")
            };
            let access =
                picked
                    .iter()
                    .map(|group| {
                        if group.zone_level() { format!("{} on {reach}", group.name) } else { format!("{} on {account_name}", group.name) }
                    })
                    .collect();
            let mut resources = vec![format!("Cloudflare account {account_name}")];
            resources.extend(picked_zones.iter().map(|zone| zone.name.clone()));
            Ok(Derived {
                values: Some(vec![
                    Variable { name: "CLOUDFLARE_API_TOKEN".to_owned(), value },
                    Variable { name: "CLOUDFLARE_ACCOUNT_ID".to_owned(), value: Zeroizing::new(account.id.clone()) },
                ]),
                meta: ExportMeta {
                    access,
                    permissions: picked.iter().map(|group| group.name.clone()).collect(),
                    resources,
                    expires_ms,
                    per_use: false,
                    renew_at_ms: renew_at(expires_ms, lifetime_ms),
                    recipe: serde_json::to_value(&minted).unwrap_or_default(),
                    // An account id isn't secret: agents may read it.
                    public: vec!["CLOUDFLARE_ACCOUNT_ID".to_owned()],
                },
            })
        })
    }

    fn renew<'a>(&'a self, http: &'a Http, control: &'a Control, export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            let mut minted: Minted = serde_json::from_value(export.recipe.clone())
                .map_err(|_| Problem::Refused("Claktory can't tell which Cloudflare token this secret is.".to_owned()))?;
            let lifetime_ms = if minted.lifetime_ms > 0 { minted.lifetime_ms } else { LIFETIME_MS };
            let expires_ms = now_ms() + lifetime_ms;
            let mut meta = ExportMeta { expires_ms, renew_at_ms: renew_at(expires_ms, lifetime_ms), ..export.clone() };
            // The same token, lasting longer: a command using it keeps working.
            let url = format!("{}/accounts/{}/tokens/{}", self.api(http), minted.account, minted.token_id);
            let mut body = token_body(&minted, expires_ms);
            body["status"] = json!("active");
            match Cloudflare::send::<Value>(http.client().put(url).bearer_auth(&grant.access_token).json(&body)).await {
                Ok(_) => Ok(Derived { values: None, meta }),
                // Deleted at Cloudflare meanwhile: a new one, as it was.
                Err(Failure::Said(404, _)) => {
                    let (token_id, value) = self.mint(http, &grant, &minted, expires_ms).await?;
                    minted.token_id = token_id;
                    meta.recipe = serde_json::to_value(&minted).unwrap_or_default();
                    Ok(Derived {
                        values: Some(vec![
                            Variable { name: "CLOUDFLARE_API_TOKEN".to_owned(), value },
                            Variable { name: "CLOUDFLARE_ACCOUNT_ID".to_owned(), value: Zeroizing::new(minted.account.clone()) },
                        ]),
                        meta,
                    })
                }
                Err(failure) => Err(failure.problem("renew an API token")),
            }
        })
    }

    fn remove_exports<'a>(&'a self, http: &'a Http, control: &'a Control, exports: &'a [ExportMeta]) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            let mut problem = None;
            for export in exports {
                let Ok(minted) = serde_json::from_value::<Minted>(export.recipe.clone()) else { continue };
                if minted.token_id.is_empty() {
                    continue;
                }
                let url = format!("{}/accounts/{}/tokens/{}", self.api(http), minted.account, minted.token_id);
                match Cloudflare::send::<Value>(http.client().delete(url).bearer_auth(&grant.access_token)).await {
                    // Gone already is as good as deleted.
                    Ok(_) | Err(Failure::Said(404, _)) => {}
                    Err(failure) => problem = Some(failure.problem("delete an API token")),
                }
            }
            problem.map_or(Ok(()), Err)
        })
    }

    fn revoke<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async move {
            let grant: Grant = control.parse()?;
            // A pasted token is deleted at Cloudflare.
            if let Some(pasted) = &grant.pasted {
                let url = format!("{}{}", self.api(http), pasted.path());
                return match Cloudflare::send::<Value>(http.client().delete(url).bearer_auth(&grant.access_token)).await {
                    Ok(_) | Err(Failure::Said(404, _)) => Ok(()),
                    Err(failure) => Err(failure.problem("delete the token you pasted")),
                };
            }
            // Revoking the refresh token ends the whole grant.
            let token = if grant.refresh_token.is_empty() { &grant.access_token } else { &grant.refresh_token };
            let url = format!("{}/oauth2/revoke", self.dash(http));
            let form = [("client_id", self.client_id.as_str()), ("token", token.as_str())];
            let response =
                http.client().post(url).form(&form).send().await.map_err(|error| network(error).problem("cancel Claktory's access"))?;
            if response.status().is_success() {
                Ok(())
            } else {
                Err(Failure::Said(response.status().as_u16(), format!("HTTP {}", response.status().as_u16()))
                    .problem("cancel Claktory's access"))
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minted(zones: &[&str]) -> Minted {
        Minted {
            account: "acc".into(),
            token_id: String::new(),
            name: "Claktory: deploy".into(),
            policy: Policy {
                account_permissions: vec!["pages-write".into()],
                zone_permissions: vec!["dns-write".into()],
                zones: zones.iter().map(|zone| (*zone).to_owned()).collect(),
            },
            lifetime_ms: LIFETIME_MS,
        }
    }

    #[test]
    fn a_token_reaches_the_account_and_only_the_zones_chosen() {
        let body = token_body(&minted(&["z1", "z2"]), 1_796_083_200_000);
        assert_eq!(body["expires_on"], "2026-12-01T00:00:00Z");
        assert_eq!(body["policies"][0]["permission_groups"], json!([{"id": "pages-write"}]));
        assert_eq!(body["policies"][0]["resources"], json!({"com.cloudflare.api.account.acc": "*"}));
        assert_eq!(body["policies"][1]["permission_groups"], json!([{"id": "dns-write"}]));
        assert_eq!(
            body["policies"][1]["resources"],
            json!({"com.cloudflare.api.account.zone.z1": "*", "com.cloudflare.api.account.zone.z2": "*"})
        );
        // No zones: every zone of the account, not every zone of every account.
        let body = token_body(&minted(&[]), 1_796_083_200_000);
        assert_eq!(body["policies"][1]["resources"], json!({"com.cloudflare.api.account.acc": {"com.cloudflare.api.account.zone.*": "*"}}));
    }

    #[test]
    fn scopes_read_as_words() {
        assert_eq!(scope_words("zone.read"), "See your zones (domains)");
        assert_eq!(scope_words("account.read"), "Account Read");
        assert_eq!(scope_words("workers-platform.write"), "Workers Platform Write");
        assert_eq!(scope_words("offline_access"), "Offline Access");
    }

    #[test]
    fn a_secret_is_renewed_a_week_before_it_ends_or_halfway() {
        assert_eq!(renew_at(LIFETIME_MS, LIFETIME_MS), LIFETIME_MS - RENEW_BEFORE_MS);
        let day = 24 * 3600 * 1000;
        assert_eq!(renew_at(2 * day, 2 * day), day);
    }
}
