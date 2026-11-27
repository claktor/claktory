//! Agents' requests for secrets (`secrets_request`). An agent doesn't know
//! which secrets the vault holds: it describes what it needs (a name, what
//! it's for, the variables it expects, never values) and why, and is
//! answered the same whether or not the vault has such a secret. Each
//! request shows in the agent's conversation after the call, where the
//! person gives one of the vault's secrets, adds a new one from the
//! description, or dismisses it (`requests::decide_request`), and the agent
//! is told only what it got. One left waiting expires
//! (`requests::expire_requests`). Requests from before named an existing
//! secret (`secret`).

use factory_protocol::v1::{self, SecretRequestState};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Vault, VaultError, check_name, crypto, valid_variable};
use crate::transcript::now_ms;

/// The longest reason an agent may give.
const MAX_REASON: usize = 2000;
/// The most variables a new secret asked for may have.
const MAX_VARIABLES: usize = 20;

/// A new secret an agent asks for: what the person fills in.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct Wanted {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub variables: Vec<WantedVariable>,
    /// A key the vault generates (#252), by type; empty for a secret
    /// someone has.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub generate: String,
    /// A key to generate for no one, as the agent asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub nobody: bool,
    /// The generated key's public half, once made.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub public_key: String,
    /// A password to generate: what it's made of, as checked (with the
    /// length the person chose, once made).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<super::generate::Password>,
    /// Files to import, as read when asked (`import`): set only by Claktory
    /// from what it read, never from what an agent sends.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub imports: Vec<super::import::Asked>,
    /// The agent had read a pipe, a direct message or a page when it asked.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_pipes: bool,
    /// Where the person makes it (#360, K3): an https link to a public
    /// site, its host in punycode (`check_link`), and that host.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub link: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub link_host: String,
    /// At most a line of how, in the agent's words (`clean_steps`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub steps: String,
    /// Where a generated key goes (K6): an owner/repo or a host
    /// (`check_place`); empty when none was given.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub place: String,
    /// When the person said the generated key is in place (K6).
    #[serde(default, skip_serializing_if = "is_zero")]
    pub done_ms: i64,
    /// The person answered with a new secret, added from the card.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub added: bool,
    /// A site's login asked for from the agent's browser (browser_sign_in):
    /// set only by Claktory, never from what an
    /// agent sends (`ask` clears them).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub login: Option<Login>,
}

/// What makes a request a login's (SecretRequest.sites, fill_only, login,
/// login_site, login_outcome).
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub(crate) struct Login {
    /// The exact https origin Chrome reported for the sign-in field's frame.
    pub origin: String,
    /// Its registrable domain, in ASCII.
    pub site: String,
    /// SecretLoginOutcome, as its number.
    #[serde(default)]
    pub outcome: i32,
}

fn is_zero(value: &i64) -> bool {
    *value == 0
}

/// The longest link an agent may give.
const MAX_LINK: usize = 2000;
/// The longest line of steps.
const MAX_STEPS: usize = 200;

/// An agent's link for making the secret, checked (Fugo's K3 rules): https,
/// a public site's name (no IP address, localhost, single label, .local or
/// .internal), no user name, password or port, at most 2,000 characters.
/// Returns the link with its host in punycode, and that host; both empty
/// for no link.
pub(crate) fn check_link(raw: &str) -> Result<(String, String), VaultError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok((String::new(), String::new()));
    }
    let refuse = |why: &str| {
        VaultError::Invalid(format!("link: {why}. Give the https:// address of the service's own page for making it, or leave link out."))
    };
    if raw.len() > MAX_LINK {
        return Err(refuse("it's longer than 2,000 characters"));
    }
    let url = url::Url::parse(raw).map_err(|_| refuse("it isn't a web address"))?;
    if url.scheme() != "https" {
        return Err(refuse("only https:// links"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(refuse("no user name or password in it"));
    }
    if url.port().is_some() {
        return Err(refuse("no port in it"));
    }
    let host = match url.host() {
        Some(url::Host::Domain(host)) => host.trim_end_matches('.').to_ascii_lowercase(),
        Some(_) => return Err(refuse("a site's name, not an IP address")),
        None => return Err(refuse("it names no site")),
    };
    let local =
        ["localhost", ".localhost", ".local", ".internal"].iter().any(|end| host == end.trim_start_matches('.') || host.ends_with(end));
    if local || !host.contains('.') {
        return Err(refuse("a public site, not this computer or a local network's"));
    }
    let link = url.to_string();
    if link.len() > MAX_LINK {
        return Err(refuse("it's longer than 2,000 characters"));
    }
    Ok((link, host))
}

/// Characters that never show in an agent's line of steps: controls, and
/// those that hide or reorder text (`text::hidden`), but for a variation
/// selector right after a visible character, an emoji's own (❤️).
fn hidden(c: char, after_visible: bool) -> bool {
    let selector = ('\u{FE00}'..='\u{FE0F}').contains(&c);
    !(selector && after_visible) && (c.is_control() || factory_agent::text::hidden(c))
}

/// An agent's steps as one plain line of at most 200 characters.
pub(crate) fn clean_steps(raw: &str) -> String {
    let mut after_visible = false;
    let spaced: String = raw
        .chars()
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .filter(|&c| {
            let hidden = hidden(c, after_visible);
            // A selector takes the character before it: a second one is stray.
            after_visible = !hidden && !c.is_whitespace() && !('\u{FE00}'..='\u{FE0F}').contains(&c);
            !hidden
        })
        .collect();
    spaced.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_STEPS).collect()
}

/// Where a generated key goes, as Claktory's own sentence names it: an
/// owner/repo or a host, letters, digits and . _ - (up to 50 each side,
/// never only dots); empty when none is given, refused otherwise.
pub(crate) fn check_place(raw: &str) -> Result<String, VaultError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(String::new());
    }
    let part = |part: &str| {
        (1..=50).contains(&part.len())
            && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
            && !part.bytes().all(|byte| byte == b'.')
    };
    let fits = match raw.split_once('/') {
        Some((owner, repo)) => part(owner) && part(repo),
        None => part(raw),
    };
    if fits {
        Ok(raw.to_owned())
    } else {
        Err(VaultError::Invalid(
            "place: an owner/repo (claktor/claktory) or a server's name (deploy.example.com), letters, digits and . _ - only, or leave place out.".to_owned(),
        ))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct WantedVariable {
    pub name: String,
    /// Given to commands as a file's path.
    #[serde(default)]
    pub file: bool,
}

fn state_of(state: &str) -> SecretRequestState {
    match state {
        "granted" => SecretRequestState::Granted,
        "dismissed" => SecretRequestState::Dismissed,
        "expired" => SecretRequestState::Expired,
        "withdrawn" => SecretRequestState::Withdrawn,
        "approved" => SecretRequestState::Approved,
        "generated" => SecretRequestState::Generated,
        _ => SecretRequestState::Pending,
    }
}

const COLUMNS: &str =
    "id, agent, secret, secret_name, new_secret, reason, state, decided, comment, given, given_name, updated, approved, admin, collecting, replied";

fn row(row: &rusqlite::Row<'_>) -> rusqlite::Result<v1::SecretRequest> {
    let new_secret: String = row.get(4)?;
    let wanted = serde_json::from_str::<Wanted>(&new_secret).ok();
    let (generate, grant_to_nobody, public_key) =
        wanted.as_ref().map(|wanted| (wanted.generate.clone(), wanted.nobody, wanted.public_key.clone())).unwrap_or_default();
    let password = wanted.as_ref().and_then(|wanted| wanted.password.as_ref()).and_then(spec);
    let state: String = row.get(6)?;
    let pending = state == "pending";
    let imports = wanted.as_ref().map(|wanted| imports(&wanted.imports, pending)).unwrap_or_default();
    let asker_read_pipes = wanted.as_ref().is_some_and(|wanted| wanted.read_pipes);
    let (link, link_host, steps, place, done_ms, added) = wanted
        .as_ref()
        .map(|wanted| {
            (wanted.link.clone(), wanted.link_host.clone(), wanted.steps.clone(), wanted.place.clone(), wanted.done_ms, wanted.added)
        })
        .unwrap_or_default();
    let login = wanted.as_ref().and_then(|wanted| wanted.login.clone());
    let new_secret = wanted.map(|wanted| v1::NewSecret {
        name: wanted.name,
        description: wanted.description,
        variables: wanted
            .variables
            .into_iter()
            .map(|variable| v1::SecretVariable { name: variable.name, hint: String::new(), file: variable.file })
            .collect(),
    });
    Ok(v1::SecretRequest {
        generate,
        grant_to_nobody,
        public_key,
        password,
        imports,
        asker_read_pipes,
        link,
        link_host,
        steps,
        place,
        done_ms,
        added,
        id: row.get(0)?,
        agent_id: row.get(1)?,
        secret_id: row.get(2)?,
        secret_name: row.get(3)?,
        new_secret,
        reason: row.get(5)?,
        state: state_of(&state).into(),
        decided_ms: row.get(7)?,
        comment: row.get(8)?,
        given_secret_id: row.get(9)?,
        given_secret_name: row.get(10)?,
        updated_ms: row.get(11)?,
        approved_ms: row.get(12)?,
        admin_agent_id: row.get(13)?,
        collecting: row.get(14)?,
        replied: row.get(15)?,
        ..login_fields(login.as_ref())
    })
}

/// A request's login fields (sites, fill_only, login, login_site,
/// login_outcome), the rest default.
fn login_fields(login: Option<&Login>) -> v1::SecretRequest {
    let Some(login) = login else { return v1::SecretRequest::default() };
    v1::SecretRequest {
        sites: vec![login.origin.clone()],
        fill_only: true,
        login: true,
        login_site: login.site.clone(),
        login_outcome: login.outcome,
        ..v1::SecretRequest::default()
    }
}

/// The login a request is for, from what the card shows.
pub(crate) fn login_of(request: &v1::SecretRequest) -> Option<Login> {
    let origin = request.sites.first().filter(|_| request.login)?;
    Some(Login { origin: origin.clone(), site: request.login_site.clone(), outcome: request.login_outcome })
}

/// Reads one request without changing its state or applying an ownership check.
fn request(db: &Connection, id: &str) -> rusqlite::Result<v1::SecretRequest> {
    db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1"), [id], row)
}

/// Reads one request only when it belongs to `agent`.
fn agent_request(db: &Connection, id: &str, agent: &str) -> rusqlite::Result<v1::SecretRequest> {
    db.query_row(&format!("SELECT {COLUMNS} FROM requests WHERE id = ?1 AND agent = ?2"), [id, agent], row)
}

/// The files a request imports, as its card shows them; with what's wrong
/// with each now while it waits.
fn imports(asked: &[super::import::Asked], pending: bool) -> Vec<v1::SecretImport> {
    asked
        .iter()
        .map(|file| v1::SecretImport {
            variable: file.variable.clone(),
            path: file.path.clone(),
            size: file.size as i64,
            modified_ms: file.modified_ms,
            fingerprint: super::import::shown(&file.keyed),
            sensitive: super::import::sensitive(std::path::Path::new(&file.path)).to_owned(),
            problem: if pending { super::import::problem(file) } else { String::new() },
        })
        .collect()
}

/// A kept password ask as its spec (None if it no longer passes, which
/// leaves nothing to generate).
fn spec(password: &super::generate::Password) -> Option<v1::PasswordSpec> {
    super::generate::password_spec(password, 0).ok()
}

/// The tool agents ask with.
pub(crate) const TOOL: &str = "secrets_request";
/// The tool an admin agent asks the person to add a secret with.
pub(crate) const COLLECT_TOOL: &str = "secrets_collect";

/// What a secrets_request result's details hold: the request as made.
pub(crate) fn details(request: &v1::SecretRequest) -> serde_json::Value {
    let new_secret = request.new_secret.as_ref().map(|wanted| {
        let variables: Vec<serde_json::Value> =
            wanted.variables.iter().map(|variable| serde_json::json!({"name": variable.name, "file": variable.file})).collect();
        serde_json::json!({"name": wanted.name, "description": wanted.description, "variables": variables})
    });
    serde_json::json!({
        "request": request.id,
        "agent": request.agent_id,
        "secret_id": request.secret_id,
        "secret_name": request.secret_name,
        "new_secret": new_secret,
        "reason": request.reason,
        "admin": request.admin_agent_id,
        "generate": request.generate,
        "nobody": request.grant_to_nobody,
        "public_key": request.public_key,
        "password": request.password.as_ref().map(super::generate::password_ask),
        "imports": request.imports.iter().map(|file| serde_json::json!({
            "variable": file.variable,
            "path": file.path,
            "size": file.size,
            "modified_ms": file.modified_ms,
            "fingerprint": file.fingerprint,
            "sensitive": file.sensitive,
        })).collect::<Vec<_>>(),
        "read_pipes": request.asker_read_pipes,
        "link": request.link,
        "link_host": request.link_host,
        "steps": request.steps,
        "place": request.place,
        "login": login_of(request),
    })
}

/// What a secrets_collect result's details hold: the request, marked so
/// the admin agent's conversation shows the add-secret card for it.
pub(crate) fn collect_details(request: &v1::SecretRequest) -> serde_json::Value {
    let mut details = details(request);
    details["collect"] = true.into();
    details["approved_ms"] = request.approved_ms.into();
    details["comment"] = request.comment.clone().into();
    details["collecting"] = request.collecting.into();
    details
}

/// The card a secrets_collect result opens in the admin agent's
/// conversation (called directly, or through extension_call): the approved
/// request, prefilled; None for any other result.
pub(crate) fn collected(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::SecretRequest> {
    if !matches!(tool, COLLECT_TOOL | "extension_call") || is_error {
        return None;
    }
    if details.get("collect").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    // Only the call that opened the card shows one; a cancel updates it.
    if details.get("collecting").and_then(serde_json::Value::as_bool) != Some(true) {
        return None;
    }
    let mut request = parsed(details)?;
    request.set_state(SecretRequestState::Approved);
    request.approved_ms = details.get("approved_ms").and_then(serde_json::Value::as_i64).unwrap_or_default();
    request.comment = details.get("comment").and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    request.collecting = true;
    Some(request)
}

/// The entry a secrets_request or browser_sign_in result shows (called
/// directly, or through extension_call), pending; None for any other
/// result, or one that asked for nothing. A secrets_collect result (its
/// details say `collect`) isn't a new request: `collected` reads those.
pub(crate) fn requested(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::SecretRequest> {
    if !matches!(tool, TOOL | crate::errands::fill::SIGN_IN_TOOL | "extension_call") || is_error || details.get("collect").is_some() {
        return None;
    }
    parsed(details)
}

/// A SecretRequest from a result's details, as `details` wrote them.
fn parsed(details: &serde_json::Value) -> Option<v1::SecretRequest> {
    let text = crate::transcript::text_fields(details);
    let id = details.get("request")?.as_str()?.to_owned();
    let new_secret =
        details.get("new_secret").and_then(|wanted| serde_json::from_value::<Wanted>(wanted.clone()).ok()).map(|wanted| v1::NewSecret {
            name: wanted.name,
            description: wanted.description,
            variables: wanted
                .variables
                .into_iter()
                .map(|variable| v1::SecretVariable { name: variable.name, hint: String::new(), file: variable.file })
                .collect(),
        });
    Some(v1::SecretRequest {
        id,
        agent_id: text("agent"),
        secret_id: text("secret_id"),
        secret_name: text("secret_name"),
        new_secret,
        reason: text("reason"),
        state: SecretRequestState::Pending.into(),
        admin_agent_id: text("admin"),
        generate: text("generate"),
        grant_to_nobody: details.get("nobody").and_then(serde_json::Value::as_bool).unwrap_or_default(),
        public_key: text("public_key"),
        password: details
            .get("password")
            .and_then(|password| serde_json::from_value::<super::generate::Password>(password.clone()).ok())
            .as_ref()
            .and_then(spec),
        imports: details
            .get("imports")
            .and_then(serde_json::Value::as_array)
            .map(|files| {
                files
                    .iter()
                    .map(|file| {
                        let text = crate::transcript::text_fields(file);
                        v1::SecretImport {
                            variable: text("variable"),
                            path: text("path"),
                            size: file.get("size").and_then(serde_json::Value::as_i64).unwrap_or_default(),
                            modified_ms: file.get("modified_ms").and_then(serde_json::Value::as_i64).unwrap_or_default(),
                            fingerprint: text("fingerprint"),
                            sensitive: text("sensitive"),
                            problem: String::new(),
                        }
                    })
                    .collect()
            })
            .unwrap_or_default(),
        asker_read_pipes: details.get("read_pipes").and_then(serde_json::Value::as_bool).unwrap_or_default(),
        link: text("link"),
        link_host: text("link_host"),
        steps: text("steps"),
        place: text("place"),
        ..login_fields(details.get("login").and_then(|login| serde_json::from_value::<Login>(login.clone()).ok()).as_ref())
    })
}

/// The requests waiting for the person, oldest first, each with when it
/// was made or the agent last changed it (ms since 1970).
fn pending(db: &Connection) -> Result<Vec<(v1::SecretRequest, i64)>, VaultError> {
    Ok(crate::store::query(
        db,
        &format!("SELECT {COLUMNS}, MAX(created, updated) AS since FROM requests WHERE state = 'pending' ORDER BY created, id"),
        [],
        // By name: COLUMNS grows (`replied`, #370), and an index here once
        // read a new column as `since`, expiring every request in a minute.
        |found| Ok((row(found)?, found.get("since")?)),
    )?)
}

/// A reason as an agent gives it, checked.
fn check_reason(reason: &str) -> Result<String, VaultError> {
    let reason = reason.trim();
    if reason.is_empty() {
        return Err(VaultError::Invalid("Say why you need it, in reason: the person reads it.".to_owned()));
    }
    if reason.chars().count() > MAX_REASON {
        return Err(VaultError::Invalid(format!("Keep reason under {MAX_REASON} characters.")));
    }
    Ok(reason.to_owned())
}

fn check_description(description: &str) -> Result<String, VaultError> {
    let description = description.trim();
    if description.chars().count() > MAX_REASON {
        return Err(VaultError::Invalid(format!("Keep description under {MAX_REASON} characters.")));
    }
    Ok(description.to_owned())
}

fn check_variables(given: &[WantedVariable]) -> Result<Vec<WantedVariable>, VaultError> {
    if given.len() > MAX_VARIABLES {
        return Err(VaultError::Invalid(format!("Name {MAX_VARIABLES} variables at most.")));
    }
    let mut variables = Vec::new();
    for variable in given {
        let variable_name = variable.name.trim();
        if !valid_variable(variable_name) {
            return Err(VaultError::Invalid(format!(
                "\"{variable_name}\" isn't a variable name: letters, digits and _, not starting with a digit."
            )));
        }
        if variables.iter().any(|seen: &WantedVariable| seen.name == variable_name) {
            return Err(VaultError::Invalid(format!("{variable_name} is named twice.")));
        }
        variables.push(WantedVariable { name: variable_name.to_owned(), file: variable.file });
    }
    Ok(variables)
}

/// A generate request's variables (#252): the one its key's file goes in,
/// a file, `SSH_KEY` when none is named; the one a password goes in,
/// `PASSWORD` when none is named (a file only if asked); any other
/// request's as given.
fn check_generated(generate: &str, variables: Vec<WantedVariable>) -> Result<Vec<WantedVariable>, VaultError> {
    let generate = generate.trim();
    if generate.is_empty() {
        return Ok(variables);
    }
    if generate == super::generate::PASSWORD {
        return match variables.as_slice() {
            [] => Ok(vec![WantedVariable { name: super::generate::PASSWORD_VARIABLE.to_owned(), file: false }]),
            [one] => Ok(vec![one.clone()]),
            _ => Err(VaultError::Invalid("A generated password goes in one variable: name one, or none for PASSWORD.".to_owned())),
        };
    }
    if !super::generate::KINDS.contains(&generate) {
        return Err(VaultError::Invalid(format!(
            "The vault can't generate \"{generate}\"; it generates {}.",
            super::generate::KINDS.join(", ")
        )));
    }
    match variables.as_slice() {
        [] => Ok(vec![WantedVariable { name: super::generate::VARIABLE.to_owned(), file: true }]),
        [one] => Ok(vec![WantedVariable { name: one.name.clone(), file: true }]),
        _ => Err(VaultError::Invalid(
            "A generated key goes in one variable, its private key's file: name one, or none for SSH_KEY.".to_owned(),
        )),
    }
}

/// What an agent changes in a request of its own; None keeps it as it was.
#[derive(Default)]
pub(crate) struct Change {
    pub description: Option<String>,
    pub reason: Option<String>,
    pub variables: Option<Vec<WantedVariable>>,
}

impl Vault {
    /// Reads the changed request before invalidating the cached vault view.
    fn changed_request(inner: &mut super::Inner, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let request = request(Self::db(inner)?, id)?;
        inner.state = None;
        Ok(request)
    }

    /// Records `agent`'s request for what `wanted` describes. Whether or
    /// not the vault has such a secret, or has given it one, makes no
    /// difference to the answer: nothing of the vault shows through. Errors
    /// say what to fix.
    pub(crate) fn ask(&self, agent: &str, wanted: Wanted, reason: &str, admin: &str) -> Result<v1::SecretRequest, VaultError> {
        let reason = check_reason(reason)?;
        let name = check_name(&wanted.name).map_err(|_| {
            VaultError::Invalid(
                "Give what you need a short name, such as \"Cloudflare token\": letters, digits, spaces and - _ . @ ( ), up to 64."
                    .to_owned(),
            )
        })?;
        let description = check_description(&wanted.description)?;
        let imports = wanted.imports;
        let variables = if imports.is_empty() {
            check_generated(&wanted.generate, check_variables(&wanted.variables)?)?
        } else {
            if !wanted.generate.trim().is_empty() || wanted.password.is_some() {
                return Err(VaultError::Invalid("Ask to import files or to generate a secret, not both.".to_owned()));
            }
            // Each file's variable, holding its path.
            imports.iter().map(|file| WantedVariable { name: file.variable.clone(), file: true }).collect()
        };
        let generate = wanted.generate.trim().to_owned();
        let (link, link_host) = check_link(&wanted.link)?;
        let steps = clean_steps(&wanted.steps);
        let place = check_place(&wanted.place)?;
        // A password is checked against the bounds now, and kept as checked
        // (its length filled in): anything weaker is refused here, not at
        // approval.
        let password = match (generate.as_str(), wanted.password) {
            (super::generate::PASSWORD, password) => {
                let spec = super::generate::password_spec(&password.unwrap_or_default(), 0)?;
                Some(super::generate::password_ask(&spec))
            }
            (_, Some(_)) => {
                return Err(VaultError::Invalid("password goes with generate: \"password\"; leave it out otherwise.".to_owned()));
            }
            (_, None) => None,
        };
        let wanted = Wanted {
            name: name.clone(),
            description,
            variables,
            generate,
            nobody: wanted.nobody,
            public_key: String::new(),
            password,
            imports,
            read_pipes: wanted.read_pipes,
            link,
            link_host,
            steps,
            place,
            done_ms: 0,
            added: false,
            // Only Claktory asks for a login (`ask_login`), from Chrome's
            // origin: never an agent's words.
            login: None,
        };
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let waiting: Option<String> = db
            .query_row(
                "SELECT id FROM requests WHERE agent = ?1 AND secret_name = ?2 COLLATE NOCASE AND state = 'pending'",
                [agent, &name],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(waiting) = waiting {
            return Err(VaultError::Invalid(format!(
                "You asked for {name} already ({waiting}); it's waiting for the person. Change it with secrets_request_update, or withdraw it with secrets_request_cancel."
            )));
        }
        let id = format!("r{}", crypto::hex(&crypto::random::<4>()));
        db.execute(
            "INSERT INTO requests (id, agent, secret, secret_name, new_secret, reason, state, created, admin) VALUES (?1, ?2, '', ?3, ?4, ?5, 'pending', ?6, ?7)",
            params![id, agent, name, json, reason, now_ms(), admin],
        )?;
        Self::changed_request(&mut inner, &id)
    }

    /// Records `agent`'s request for its login to `origin` (an exact https
    /// origin Chrome reported, as `browser::origin::canonical` spells it):
    /// USERNAME and PASSWORD, named for the site ("X login"), to be bound
    /// to that origin and granted fill-only. One already waiting for the
    /// same origin is answered instead of a second: (it, false). `given` is
    /// the login it signed in with and the site refused, whose values Sign
    /// in replaces (`outcome` REFUSED then).
    pub(crate) fn ask_login(
        &self,
        agent: &str,
        origin: &str,
        reason: &str,
        given: Option<(&str, &str)>,
        outcome: v1::SecretLoginOutcome,
    ) -> Result<(v1::SecretRequest, bool), VaultError> {
        let site = super::logins::site_of(origin);
        if site.is_empty() {
            return Err(VaultError::Invalid(format!("Claktory signs in only on https sites, not {origin}.")));
        }
        // The agent's line is optional: the card says the site anyway.
        let reason = if reason.trim().is_empty() { format!("To sign in to {site}.") } else { check_reason(reason)? };
        if let Some(waiting) = self.login_request(agent, origin)?.filter(|found| found.state() == SecretRequestState::Pending) {
            return Ok((waiting, false));
        }
        let name = given.map_or_else(|| super::logins::login_name(&site), |(_, name)| name.to_owned());
        let wanted = Wanted {
            name: name.clone(),
            description: format!("Your login for {site}. Claktory types it on {origin} only."),
            variables: [super::logins::USERNAME, super::logins::PASSWORD]
                .into_iter()
                .map(|name| WantedVariable { name: name.to_owned(), file: false })
                .collect(),
            login: Some(Login { origin: origin.to_owned(), site, outcome: outcome.into() }),
            ..Wanted::default()
        };
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let (given, given_name) = given.unwrap_or_default();
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let id = format!("r{}", crypto::hex(&crypto::random::<4>()));
        db.execute(
            "INSERT INTO requests (id, agent, secret, secret_name, new_secret, reason, state, created, admin, given, given_name)
             VALUES (?1, ?2, '', ?3, ?4, ?5, 'pending', ?6, '', ?7, ?8)",
            params![id, agent, name, json, reason, now_ms(), given, given_name],
        )?;
        Self::changed_request(&mut inner, &id).map(|request| (request, true))
    }

    /// `agent`'s latest request for its login to exactly `origin`, however
    /// it stands; None when it never asked.
    pub(crate) fn login_request(&self, agent: &str, origin: &str) -> Result<Option<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(None) };
        let asked: Vec<v1::SecretRequest> = crate::store::query(
            db,
            &format!("SELECT {COLUMNS} FROM requests WHERE agent = ?1 AND new_secret LIKE '%\"login\":%' ORDER BY created DESC, rowid DESC"),
            [agent],
            row,
        )?;
        Ok(asked.into_iter().find(|request| request.login && request.sites.first().is_some_and(|site| site == origin)))
    }

    /// What filling login request `id`'s login did (SecretLoginOutcome);
    /// REFUSED opens it again, waiting for the person, the login it was
    /// given kept so Sign in replaces its values. Returns it as it is now.
    pub(crate) fn set_login_outcome(&self, id: &str, outcome: v1::SecretLoginOutcome) -> Result<v1::SecretRequest, VaultError> {
        let changed = self.change_wanted(id, |wanted| {
            if let Some(login) = wanted.login.as_mut() {
                login.outcome = outcome.into();
            }
        })?;
        if outcome != v1::SecretLoginOutcome::Refused || changed.state() == SecretRequestState::Pending {
            return Ok(changed);
        }
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        db.execute(
            "UPDATE requests SET state = 'pending', decided = 0, comment = '', updated = ?2 WHERE id = ?1 AND state = 'granted'",
            params![id, now_ms()],
        )?;
        Self::changed_request(&mut inner, id)
    }

    /// `agent`'s request `id`, if it's still waiting.
    pub(crate) fn pending_request(&self, agent: &str, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let inner = self.lock();
        let found = agent_request(Self::db(&inner)?, id, agent).optional()?;
        match found {
            None => Err(VaultError::NoRequest(id.to_owned())),
            Some(request) if request.state() == SecretRequestState::Expired => Err(VaultError::Invalid("This request expired.".to_owned())),
            Some(request) if request.state() == SecretRequestState::Withdrawn => {
                Err(VaultError::Invalid("The agent withdrew this request.".to_owned()))
            }
            Some(request) if !matches!(request.state(), SecretRequestState::Pending | SecretRequestState::Approved) => {
                Err(VaultError::Invalid("This request was decided already.".to_owned()))
            }
            Some(request) => Ok(request),
        }
    }

    /// Marks `agent`'s request `id` approved: the person approved the need
    /// and `admin` (an admin agent) finds or collects the secret. The
    /// person's comment, if any, shows with it. Returns it as it is now.
    pub(crate) fn approve_request(&self, agent: &str, id: &str, admin: &str, comment: &str) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET state = 'approved', approved = ?3, admin = ?4, comment = ?5
             WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, now_ms(), admin, comment.trim()],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This request was decided already.".to_owned()));
        }
        Self::changed_request(&mut inner, id)
    }

    /// Marks whether `admin`'s add-secret card for the request is open
    /// (secrets_collect), taking the request over for `admin`; only an
    /// approved request can be collected. Returns it as it is now.
    pub(crate) fn set_collecting(&self, agent: &str, id: &str, admin: &str, collecting: bool) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET collecting = ?3, admin = ?4 WHERE id = ?1 AND agent = ?2 AND state = 'approved'",
            params![id, agent, collecting, admin],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("Only a request the person approved can be collected.".to_owned()));
        }
        Self::changed_request(&mut inner, id)
    }

    /// The request `id`, whoever asked, if the person approved it and it's
    /// with the admin agent; or why it can't be worked on.
    pub(crate) fn approved_request(&self, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let inner = self.lock();
        let found = request(Self::db(&inner)?, id.trim()).optional()?;
        match found {
            None => Err(VaultError::Invalid(format!("There's no request {}.", id.trim()))),
            Some(request) if request.state() == SecretRequestState::Pending => {
                Err(VaultError::Invalid(format!("{} still waits for the person to approve the need.", request.id)))
            }
            Some(request) if request.state() != SecretRequestState::Approved => {
                Err(VaultError::Invalid(format!("{} isn't open any more.", request.id)))
            }
            Some(request) => Ok(request),
        }
    }

    /// The request `id`, whoever asked and however it stands; None if
    /// there's no such request. An error while the vault can't be read.
    pub(crate) fn find_request(&self, id: &str) -> Result<Option<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        Ok(request(db, id.trim()).optional()?)
    }

    /// Requests for secrets made after `after` (a row's rowid), oldest
    /// first, at most `limit`: (rowid, the agent that asked, when), for
    /// History (#458). Nothing of what was asked.
    pub(crate) fn requests_after(&self, after: i64, limit: usize) -> Vec<(i64, String, i64)> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Vec::new() };
        let limit = i64::try_from(limit).unwrap_or(i64::MAX);
        crate::store::query(db, "SELECT rowid, agent, created FROM requests WHERE rowid > ?1 ORDER BY rowid LIMIT ?2", [after, limit], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .unwrap_or_else(|error| {
            tracing::warn!(%error, "could not read the requests for secrets");
            Vec::new()
        })
    }

    /// Hands back the open requests `admin` held (it's being deleted):
    /// they lose their admin and stop collecting, so the asker's card is
    /// the person's to answer directly again. The changed requests, as
    /// they are now.
    pub(crate) fn forget_admin(&self, admin: &str) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let mut inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        let ids: Vec<String> =
            crate::store::query(db, "SELECT id FROM requests WHERE admin = ?1 AND state IN ('pending', 'approved')", [admin], |row| {
                row.get(0)
            })?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        db.execute("UPDATE requests SET admin = '', collecting = 0 WHERE admin = ?1 AND state IN ('pending', 'approved')", [admin])?;
        let mut requests = Vec::new();
        for id in &ids {
            requests.push(request(db, id)?);
        }
        inner.state = None;
        Ok(requests)
    }

    /// The requests `admin` was handed (approved, or decided with it set),
    /// oldest first: to refresh its conversation's cards on load.
    pub(crate) fn requests_with_admin(&self, admin: &str) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        Ok(crate::store::query(db, &format!("SELECT {COLUMNS} FROM requests WHERE admin = ?1 ORDER BY created, id"), [admin], row)?)
    }

    /// Marks the request given (the secret `given`: its id and name) or
    /// dismissed (None), `replied` when dismissed by the person's reply
    /// (#370); returns it as it is now.
    pub(crate) fn decide_request(
        &self,
        agent: &str,
        id: &str,
        given: Option<(&str, &str)>,
        comment: &str,
        replied: bool,
    ) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let state = if given.is_some() { "granted" } else { "dismissed" };
        let (given, given_name) = given.unwrap_or_default();
        let changed = db.execute(
            "UPDATE requests SET state = ?3, decided = ?4, comment = ?5, given = ?6, given_name = ?7, collecting = 0, replied = ?8
             WHERE id = ?1 AND agent = ?2 AND state IN ('pending', 'approved')",
            params![id, agent, state, now_ms(), comment.trim(), given, given_name, replied && given.is_empty()],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This request was decided already.".to_owned()));
        }
        Self::changed_request(&mut inner, id)
    }

    /// Marks a generate request decided (#252): its key or password
    /// `given` (id, and the name the asker has it under) made (`made`: a
    /// key's public half, or the password's spec as made), and given to
    /// the asker or, `nobody`, to no one. Returns it as it is now.
    pub(crate) fn decide_generated(
        &self,
        request: &v1::SecretRequest,
        given: (&str, &str),
        made: (&str, Option<&v1::PasswordSpec>),
        nobody: bool,
        comment: &str,
    ) -> Result<v1::SecretRequest, VaultError> {
        let (public_key, password) = made;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let json: String = db.query_row("SELECT new_secret FROM requests WHERE id = ?1", [&request.id], |row| row.get(0))?;
        let mut wanted: Wanted = serde_json::from_str(&json).unwrap_or_default();
        wanted.public_key = public_key.to_owned();
        if let Some(password) = password {
            wanted.password = Some(super::generate::password_ask(password));
        }
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let state = if nobody { "generated" } else { "granted" };
        let changed = db.execute(
            "UPDATE requests SET state = ?3, decided = ?4, comment = ?5, given = ?6, given_name = ?7, new_secret = ?8, collecting = 0
             WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![request.id, request.agent_id, state, now_ms(), comment.trim(), given.0, given.1, json],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid("This request was decided already.".to_owned()));
        }
        Self::changed_request(&mut inner, &request.id)
    }

    /// The requests waiting for the person, oldest first, each with when it
    /// was made or last changed; none while there's no vault.
    pub(crate) fn waiting_requests(&self) -> Result<Vec<(v1::SecretRequest, i64)>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        pending(db)
    }

    /// Marks `agent`'s request `id` expired, if it's still waiting; returns
    /// it as it is now, or None if it was decided meanwhile.
    pub(crate) fn expire_request(&self, agent: &str, id: &str) -> Result<Option<v1::SecretRequest>, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET state = 'expired', decided = ?3 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, now_ms()],
        )?;
        if changed == 0 {
            return Ok(None);
        }
        Self::changed_request(&mut inner, id).map(Some)
    }

    /// Changes `agent`'s waiting request `id`; returns it as it is now.
    pub(crate) fn update_request(&self, agent: &str, id: &str, change: Change) -> Result<v1::SecretRequest, VaultError> {
        let id = id.trim();
        let current = self.own_waiting(agent, id)?;
        if current.login {
            return Err(VaultError::Invalid(format!(
                "{} is a sign-in Claktory asked for from your browser, so it can't be changed: withdraw it with secrets_request_cancel if you no longer need it.",
                current.id
            )));
        }
        let kept = self.wanted(&current.id)?;
        let mut wanted: Wanted = Wanted {
            name: current.secret_name.clone(),
            generate: current.generate.clone(),
            nobody: current.grant_to_nobody,
            password: current.password.as_ref().map(super::generate::password_ask),
            // What was read of the files stays as it was.
            imports: kept.imports,
            read_pipes: current.asker_read_pipes,
            link: kept.link,
            link_host: kept.link_host,
            steps: kept.steps,
            place: kept.place,
            ..current.new_secret.as_ref().map_or_else(Wanted::default, |wanted| Wanted {
                name: wanted.name.clone(),
                description: wanted.description.clone(),
                variables: wanted
                    .variables
                    .iter()
                    .map(|variable| WantedVariable { name: variable.name.clone(), file: variable.file })
                    .collect(),
                ..Wanted::default()
            })
        };
        if let Some(description) = &change.description {
            wanted.description = check_description(description)?;
        }
        if let Some(variables) = &change.variables {
            if !wanted.imports.is_empty() {
                return Err(VaultError::Invalid(
                    "An import's variables come with its files: withdraw it with secrets_request_cancel and ask again.".to_owned(),
                ));
            }
            wanted.variables = check_generated(&wanted.generate, check_variables(variables)?)?;
        }
        let reason = match &change.reason {
            Some(reason) => check_reason(reason)?,
            None => current.reason.clone(),
        };
        if change.description.is_none() && change.variables.is_none() && change.reason.is_none() {
            return Err(VaultError::Invalid("Say what to change: description, reason or variables.".to_owned()));
        }
        if current.state() == SecretRequestState::Approved {
            return Err(VaultError::Invalid(format!(
                "The person already approved {}; it can't be changed any more. Withdraw it with secrets_request_cancel if you no longer need it.",
                current.id
            )));
        }
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET new_secret = ?3, reason = ?4, updated = ?5 WHERE id = ?1 AND agent = ?2 AND state = 'pending'",
            params![id, agent, json, reason, now_ms()],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid(format!("Your request {id} isn't waiting any more.")));
        }
        Self::changed_request(&mut inner, id)
    }

    /// Withdraws `agent`'s waiting request `id`, with its reason (may be
    /// empty); returns it as it is now.
    pub(crate) fn withdraw_request(&self, agent: &str, id: &str, reason: &str) -> Result<v1::SecretRequest, VaultError> {
        let id = id.trim();
        self.own_waiting(agent, id)?;
        let reason = check_description(reason)?;
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let changed = db.execute(
            "UPDATE requests SET state = 'withdrawn', decided = ?3, comment = ?4, collecting = 0
             WHERE id = ?1 AND agent = ?2 AND state IN ('pending', 'approved')",
            params![id, agent, now_ms(), reason],
        )?;
        if changed == 0 {
            return Err(VaultError::Invalid(format!("Your request {id} isn't waiting any more.")));
        }
        Self::changed_request(&mut inner, id)
    }

    /// The request `id` as kept, with what the card doesn't show (an
    /// import's files as read when asked).
    pub(crate) fn wanted(&self, id: &str) -> Result<Wanted, VaultError> {
        let inner = self.lock();
        let json: String = Self::db(&inner)?.query_row("SELECT new_secret FROM requests WHERE id = ?1", [id], |row| row.get(0))?;
        Ok(serde_json::from_str(&json).unwrap_or_default())
    }

    /// Changes what's kept of request `id`'s description (`Wanted`), in place;
    /// returns it as it is now.
    fn change_wanted(&self, id: &str, change: impl FnOnce(&mut Wanted)) -> Result<v1::SecretRequest, VaultError> {
        let mut inner = self.lock();
        let db = Self::db(&inner)?;
        let json: String = db.query_row("SELECT new_secret FROM requests WHERE id = ?1", [id], |row| row.get(0))?;
        let mut wanted: Wanted = serde_json::from_str(&json).unwrap_or_default();
        change(&mut wanted);
        let json = serde_json::to_string(&wanted).map_err(|error| VaultError::Invalid(error.to_string()))?;
        db.execute("UPDATE requests SET new_secret = ?2 WHERE id = ?1", params![id, json])?;
        Self::changed_request(&mut inner, id)
    }

    /// Request `id` was answered with a secret the person added from the
    /// card (SecretRequest.added); returns it as it is now.
    pub(crate) fn mark_added(&self, id: &str) -> Result<v1::SecretRequest, VaultError> {
        self.change_wanted(id, |wanted| wanted.added = true)
    }

    /// The person put `agent`'s generated key in place (K6,
    /// SecretRequestDone): None when they had already said so.
    pub(crate) fn mark_done(&self, agent: &str, id: &str) -> Result<Option<v1::SecretRequest>, VaultError> {
        let request = {
            let inner = self.lock();
            agent_request(Self::db(&inner)?, id.trim(), agent).optional()?
        };
        let Some(request) = request else { return Err(VaultError::NoRequest(id.to_owned())) };
        let made = matches!(request.state(), SecretRequestState::Generated | SecretRequestState::Granted);
        if request.generate != super::generate::SSH_ED25519 || !made {
            return Err(VaultError::Invalid("Only a generated key can be put in place.".to_owned()));
        }
        if request.done_ms != 0 {
            return Ok(None);
        }
        self.change_wanted(&request.id, |wanted| wanted.done_ms = now_ms()).map(Some)
    }

    /// `agent`'s own request `id`, if it's still open (waiting, or approved
    /// and with the admin agent); or why not, for the agent.
    fn own_waiting(&self, agent: &str, id: &str) -> Result<v1::SecretRequest, VaultError> {
        let inner = self.lock();
        let found = agent_request(Self::db(&inner)?, id.trim(), agent).optional()?;
        match found {
            None => Err(VaultError::Invalid(format!("You have no request {}. secrets_requests lists yours.", id.trim()))),
            Some(request) if !matches!(request.state(), SecretRequestState::Pending | SecretRequestState::Approved) => {
                Err(VaultError::Invalid(format!("Your request {} isn't waiting any more.", request.id)))
            }
            Some(request) => Ok(request),
        }
    }

    /// `agent`'s waiting requests and its latest `decided` others, newest
    /// first.
    pub(crate) fn recent_requests(&self, agent: &str, decided: u32) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        Ok(crate::store::query(
            db,
            &format!(
                "SELECT {COLUMNS} FROM requests WHERE agent = ?1 AND state = 'pending'
             UNION ALL SELECT * FROM (SELECT {COLUMNS} FROM requests WHERE agent = ?1 AND state != 'pending' ORDER BY decided DESC LIMIT ?2)"
            ),
            params![agent, decided],
            row,
        )?)
    }

    /// Every request of `agent`'s, to show its conversation as it is.
    pub(crate) fn requests_of(&self, agent: &str) -> Result<Vec<v1::SecretRequest>, VaultError> {
        let inner = self.lock();
        let Some(db) = &inner.db else { return Ok(Vec::new()) };
        Ok(crate::store::query(db, &format!("SELECT {COLUMNS} FROM requests WHERE agent = ?1 ORDER BY created"), [agent], row)?)
    }
}

#[cfg(test)]
mod tests {
    use super::clean_steps;

    #[test]
    fn steps_lose_every_hidden_character() {
        assert_eq!(
            clean_steps("Open\u{E0065} the\u{61C} Tokens\u{3164} page,\u{202E}\u{200B}\n then\tCreate."),
            "Open the Tokens page, then Create."
        );
        // An emoji keeps its own selector; a stray or second one goes.
        assert_eq!(
            clean_steps("\u{FE0F}Click \u{2764}\u{FE0F}\u{FE0F}, 1\u{FE0F}\u{20E3} or \u{200B}\u{FE0F}x \u{FE0E}."),
            "Click \u{2764}\u{FE0F}, 1\u{FE0F}\u{20E3} or x ."
        );
    }
}
