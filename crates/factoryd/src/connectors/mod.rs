//! Connecting services (docs/CONNECTORS.md): the person connects an account
//! of a service such as Cloudflare or AWS by approving Claktory on the
//! service's own site, and the backend keeps what it gets in the vault.
//!
//! A **connector** is a recipe built in for one service (`Recipe`): its
//! endpoints, public client, access choices, and how to sign in, refresh,
//! list, mint and revoke. This module is what every recipe shares: the
//! flows (state, PKCE, the device that started them, expiry, the device
//! flow's polling), the vault's connections and the secrets made from them
//! (exports), refreshing tokens one at a time per connection, renewing
//! exports before a command gets them, and what the person and agents are
//! told.
//!
//! A connection is given to no one: its tokens never become variables.
//! Agents get exports, made by an admin agent such as Rivet or the person
//! with only the access the work needs (`derive`).

pub mod aws;
pub mod aws_stack;
pub mod cloudflare;
pub(crate) mod proposals;

use std::collections::{BTreeMap, HashMap};
use std::fmt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use factory_protocol::v1::{self, ConnectFlowState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

use crate::encoding::{hex, same as equal_in_constant_time};
use crate::factory::{Factory, RequestError};
use crate::locking::lock as guard;
use crate::transcript::now_ms;
use crate::vault::connections::{ConnectionMeta, Opened};
use crate::vault::{Value, VaultError};

pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

/// How long a flow waits for the person…
const FLOW_MS: i64 = 10 * 60 * 1000;
/// …and one that waits for a token the person makes on the service's site.
const PASTE_FLOW_MS: i64 = 30 * 60 * 1000;
/// How long an ended flow stays listed, for every device to see how it went.
const ENDED_MS: i64 = 10 * 60 * 1000;
/// How long values an export had before are still hidden from agents: a
/// command that got them may still be printing them.
const RETIRED_MS: i64 = 24 * 3600 * 1000;

// ---- What a recipe is ----

/// A connector as the app's catalog shows it.
#[derive(Clone, Debug, Default)]
pub struct Spec {
    pub id: String,
    pub name: String,
    pub description: String,
    pub accesses: Vec<AccessSpec>,
    pub fields: Vec<FieldSpec>,
    pub flow: FlowKind,
    /// BROWSER: the callbacks it accepts; "{port}" stands for any port.
    pub redirect_uris: Vec<String>,
    pub variables: Vec<String>,
    pub setup_note: String,
    pub setup_url: String,
    /// PASTE: the page that makes the token, filled in.
    pub token_url: String,
}

#[derive(Clone, Debug, Default)]
pub struct AccessSpec {
    pub id: String,
    pub label: String,
    pub why: String,
    pub scopes: Vec<String>,
    pub preselected: bool,
}

#[derive(Clone, Debug, Default)]
pub struct FieldSpec {
    pub id: String,
    pub label: String,
    pub placeholder: String,
    pub hint: String,
    pub optional: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FlowKind {
    #[default]
    Browser,
    Device,
    /// The person makes a token on the service's page and pastes it.
    Paste,
}

impl Spec {
    fn to_proto(&self) -> v1::Connector {
        v1::Connector {
            id: self.id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            accesses: self
                .accesses
                .iter()
                .map(|access| v1::ConnectorAccess {
                    id: access.id.clone(),
                    label: access.label.clone(),
                    why: access.why.clone(),
                    scopes: access.scopes.clone(),
                    preselected: access.preselected,
                })
                .collect(),
            fields: self
                .fields
                .iter()
                .map(|field| v1::ConnectorField {
                    id: field.id.clone(),
                    label: field.label.clone(),
                    placeholder: field.placeholder.clone(),
                    hint: field.hint.clone(),
                    optional: field.optional,
                })
                .collect(),
            flow: match self.flow {
                FlowKind::Browser => v1::ConnectorFlowKind::Browser,
                FlowKind::Device => v1::ConnectorFlowKind::Device,
                FlowKind::Paste => v1::ConnectorFlowKind::Paste,
            }
            .into(),
            redirect_uris: self.redirect_uris.clone(),
            variables: self.variables.clone(),
            setup_note: self.setup_note.clone(),
            setup_url: self.setup_url.clone(),
            token_url: self.token_url.clone(),
        }
    }

    /// The access asked for: `id`, or the preselected one.
    fn access(&self, id: &str) -> Option<&AccessSpec> {
        if id.is_empty() {
            self.accesses.iter().find(|access| access.preselected).or_else(|| self.accesses.first())
        } else {
            self.accesses.iter().find(|access| access.id == id)
        }
    }
}

/// A connection's tokens: the recipe's own JSON, wiped when dropped and
/// never shown by Debug. Only the broker and its recipe read it.
pub struct Control(Zeroizing<String>);

impl Control {
    pub fn new(json: String) -> Control {
        Control(Zeroizing::new(json))
    }

    pub fn from_value<T: Serialize>(value: &T) -> Control {
        Control::new(serde_json::to_string(value).unwrap_or_default())
    }

    pub fn parse<T: serde::de::DeserializeOwned>(&self) -> Result<T, Problem> {
        serde_json::from_str(&self.0).map_err(|_| Problem::Reconnect("Its sign-in can't be read any more.".to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Control {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Control([hidden])")
    }
}

/// What a recipe is given to start a flow.
pub struct Start<'a> {
    pub access: &'a AccessSpec,
    /// The connector's fields, trimmed, required ones present.
    pub fields: &'a BTreeMap<String, String>,
    /// BROWSER: where the browser comes back to, checked against the spec.
    pub redirect_uri: &'a str,
    /// BROWSER: the state to send, and PKCE's S256 challenge. The state is
    /// 43 random URL-safe characters, unique to the flow.
    pub state: &'a str,
    pub challenge: &'a str,
    /// The flow's id ("f" and 12 hex digits).
    pub flow: &'a str,
    /// The agent's suggestion it answers, if it answers one.
    pub proposal: Option<&'a v1::ConnectProposal>,
}

/// A device flow started (DEVICE).
pub struct Device {
    /// The verification page, with the code filled in where the service can.
    pub url: String,
    pub user_code: String,
    pub interval: Duration,
    /// When the service gives up on it, in ms since 1970; 0 for the flow's own limit.
    pub expires_ms: i64,
    /// What polling needs (the device code, a client registration).
    pub pending: Control,
}

pub enum Poll {
    Pending,
    SlowDown,
    Done(Control),
}

/// Who and what a connection is, as the service says.
#[derive(Clone, Debug, Default)]
pub struct Inspected {
    /// The account's stable id, to tell a reconnect to another account.
    pub subject: String,
    /// Whose, in words: "matt@example.com".
    pub account: String,
    pub access: Vec<String>,
    pub scopes: Vec<String>,
    pub resources: Vec<String>,
    pub expires_ms: i64,
}

#[derive(Clone, Debug, Default)]
pub struct Choice {
    pub kind: String,
    pub id: String,
    pub label: String,
    pub parent: String,
    pub why: String,
}

#[derive(Clone, Debug, Default)]
pub struct Choices {
    pub choices: Vec<Choice>,
    /// How to pick, for the person and admin agents.
    pub rules: String,
}

/// What to make a secret of.
#[derive(Clone, Debug, Default)]
pub struct Derive {
    pub name: String,
    pub permissions: Vec<String>,
    pub resources: Vec<String>,
    /// 0 for the recipe's default.
    pub expires_ms: i64,
}

pub struct Variable {
    pub name: String,
    pub value: Zeroizing<String>,
}

/// A secret made or renewed: its values (None: the same as before) and
/// what it reaches.
pub struct Derived {
    pub values: Option<Vec<Variable>>,
    pub meta: ExportMeta,
}

/// What a secret made from a connection reaches, kept beside it (never a
/// token).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct ExportMeta {
    #[serde(default)]
    pub access: Vec<String>,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    /// When it stops working; 0 if never.
    #[serde(default)]
    pub expires_ms: i64,
    /// Short-lived, renewed for the person (shown as such); renewal itself
    /// goes by `renew_at_ms`.
    #[serde(default)]
    pub per_use: bool,
    /// From when a command's use renews it first; 0 never.
    #[serde(default)]
    pub renew_at_ms: i64,
    /// The recipe's own: what it needs to renew and remove it (a minted
    /// token's id, the policy as given). Not secret.
    #[serde(default)]
    pub recipe: serde_json::Value,
    /// Its variables that aren't secret (a region): given to commands like
    /// the rest, but not hidden in what agents read.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub public: Vec<String>,
}

/// Why something a recipe did didn't work, in words for the person.
#[derive(Clone, Debug)]
pub enum Problem {
    /// The service no longer accepts the connection: it needs connecting again.
    Reconnect(String),
    /// The service couldn't be reached, or answered with an error.
    Unreachable(String),
    /// Anything else: what was asked can't be done.
    Refused(String),
}

impl Problem {
    pub fn text(&self) -> &str {
        match self {
            Problem::Reconnect(text) | Problem::Unreachable(text) | Problem::Refused(text) => text,
        }
    }

    fn unsupported() -> Problem {
        Problem::Refused("This service can't do that.".to_owned())
    }
}

impl From<Problem> for RequestError {
    fn from(problem: Problem) -> RequestError {
        match problem {
            Problem::Unreachable(text) => RequestError::Unreachable(text),
            Problem::Reconnect(text) | Problem::Refused(text) => RequestError::Connect(text),
        }
    }
}

/// The HTTP client recipes use: it never follows a redirect, and each
/// service's address can be replaced, for tests only.
pub struct Http {
    client: reqwest::Client,
    bases: Mutex<HashMap<String, String>>,
}

impl Http {
    fn new() -> Http {
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(30))
            .user_agent(concat!("Claktory/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Http { client, bases: Mutex::new(HashMap::new()) }
    }

    pub fn client(&self) -> &reqwest::Client {
        &self.client
    }

    /// The address to use for `key` ("cloudflare", "aws.oidc"): `default`,
    /// unless a test replaced it.
    pub fn base(&self, key: &str, default: &str) -> String {
        self.bases.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(key).cloned().unwrap_or_else(|| default.to_owned())
    }
}

/// A service's recipe. Only `spec`, `inspect` and the methods of its flow
/// kind are needed; the rest say the service can't.
pub trait Recipe: Send + Sync {
    fn spec(&self) -> &Spec;

    /// BROWSER: the page that asks the person, for `start`.
    fn authorize_url(&self, _start: &Start<'_>) -> Result<String, Problem> {
        Err(Problem::unsupported())
    }

    /// BROWSER: the code the browser brought back, and PKCE's verifier, for
    /// the connection's tokens.
    fn exchange<'a>(
        &'a self,
        _http: &'a Http,
        _start: &'a Start<'a>,
        _code: &'a str,
        _verifier: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// DEVICE: starts the flow.
    fn start_device<'a>(&'a self, _http: &'a Http, _start: &'a Start<'a>) -> BoxFuture<'a, Result<Device, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// DEVICE: whether the person approved yet.
    fn poll_device<'a>(&'a self, _http: &'a Http, _start: &'a Start<'a>, _pending: &'a Control) -> BoxFuture<'a, Result<Poll, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// PASTE: starts the flow: the page to open (by default the spec's
    /// `token_url`), and what checking the token will need, kept with the
    /// flow (by default nothing; a key made for this flow, say).
    fn start_paste<'a>(&'a self, _http: &'a Http, _start: &'a Start<'a>) -> BoxFuture<'a, Result<(String, Control), Problem>> {
        let url = self.spec().token_url.clone();
        Box::pin(async move { Ok((url, Control::new(String::new()))) })
    }

    /// PASTE: the token the person pasted, checked with the service (with
    /// `pending`, what `start_paste` kept), as the connection's control. A
    /// token it doesn't accept is a Problem saying why, and the person can
    /// paste another.
    fn paste<'a>(
        &'a self,
        _http: &'a Http,
        _start: &'a Start<'a>,
        _pending: &'a Control,
        _token: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// Who the connection is and what it reaches.
    fn inspect<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>>;

    /// Fresh tokens if they need refreshing; None while they're good.
    /// Problem::Reconnect when the service no longer accepts them.
    fn refresh<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<Option<Control>, Problem>> {
        Box::pin(async { Ok(None) })
    }

    /// What a secret can be made of.
    fn choices<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<Choices, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// Makes a secret with the access asked (values required).
    fn derive<'a>(&'a self, _http: &'a Http, _control: &'a Control, _ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// Renews a secret made before, now due (ExportMeta.renew_at_ms).
    fn renew<'a>(&'a self, _http: &'a Http, _control: &'a Control, _export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async { Err(Problem::unsupported()) })
    }

    /// Removes secrets made before at the service (minted tokens), where
    /// there's anything to remove.
    fn remove_exports<'a>(
        &'a self,
        _http: &'a Http,
        _control: &'a Control,
        _exports: &'a [ExportMeta],
    ) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async { Ok(()) })
    }

    /// Revokes the connection at the service.
    fn revoke<'a>(&'a self, _http: &'a Http, _control: &'a Control) -> BoxFuture<'a, Result<(), Problem>> {
        let name = self.spec().name.clone();
        Box::pin(async move {
            Err(Problem::Refused(format!("{name} can't be cancelled from here. Remove Claktory's access on {name}'s site.")))
        })
    }

    /// The values in its tokens to hide from agents: by default every
    /// top-level string whose key says token or secret.
    fn hidden(&self, control: &Control) -> Vec<Zeroizing<String>> {
        let Ok(serde_json::Value::Object(map)) = serde_json::from_str::<serde_json::Value>(control.as_str()) else { return Vec::new() };
        map.iter()
            .filter(|(key, _)| key.contains("token") || key.contains("secret"))
            .filter_map(|(_, value)| value.as_str())
            .map(|value| Zeroizing::new(value.to_owned()))
            .collect()
    }
}

/// A trimmed environment setting; absent, non-Unicode and empty values are unset.
fn setting(name: &str) -> Option<String> {
    std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty())
}

/// The recipes built in.
fn built_in() -> Vec<Arc<dyn Recipe>> {
    let mut recipes: Vec<Arc<dyn Recipe>> = Vec::new();
    recipes.extend(aws_stack::AwsStack::built_in().map(|recipe| Arc::new(recipe) as Arc<dyn Recipe>));
    recipes.extend(aws::Aws::built_in().map(|recipe| Arc::new(recipe) as Arc<dyn Recipe>));
    recipes.extend(cloudflare::Cloudflare::built_in().map(|recipe| Arc::new(recipe) as Arc<dyn Recipe>));
    recipes
}

/// Whether this Claktory connects any service: until one is built in (or a
/// test adds one), nothing about connecting shows, to agents or the person.
static OFFERED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn offered() -> bool {
    OFFERED.load(std::sync::atomic::Ordering::Relaxed)
}

// ---- The broker ----

struct Flow {
    view: v1::ConnectFlow,
    recipe: Arc<dyn Recipe>,
    /// The device it belongs to: only it can finish it.
    binding: String,
    access: AccessSpec,
    fields: BTreeMap<String, String>,
    redirect_uri: String,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
    challenge: String,
    /// A connection to connect again in place.
    reconnect: String,
    /// The suggestion it answers, and the person's comment for its agent.
    proposal: Option<v1::ConnectProposal>,
    comment: String,
    /// PASTE: what `start_paste` kept for checking the token.
    pending: Control,
    /// The browser's answer was taken (codes are used once).
    finished: bool,
    cancel: CancellationToken,
    ended_ms: i64,
}

impl Flow {
    fn active(&self) -> bool {
        self.ended_ms == 0
    }

    fn start(&self) -> Start<'_> {
        Start {
            access: &self.access,
            fields: &self.fields,
            redirect_uri: &self.redirect_uri,
            state: &self.state,
            challenge: &self.challenge,
            flow: &self.view.id,
            proposal: self.proposal.as_ref(),
        }
    }
}

/// The connections being made, and what connections share.
pub(crate) struct Broker {
    recipes: Mutex<Vec<Arc<dyn Recipe>>>,
    http: Http,
    flows: Mutex<Vec<Flow>>,
    /// One refresh or renewal at a time per connection.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Values exports and connections had before, still hidden for a while.
    retired: Mutex<Vec<(i64, factory_agent::SecretVariable)>>,
}

impl Default for Broker {
    fn default() -> Broker {
        let recipes = built_in();
        if !recipes.is_empty() {
            OFFERED.store(true, std::sync::atomic::Ordering::Relaxed);
        }
        Broker {
            recipes: Mutex::new(recipes),
            http: Http::new(),
            flows: Mutex::new(Vec::new()),
            locks: Mutex::new(HashMap::new()),
            retired: Mutex::new(Vec::new()),
        }
    }
}

fn random_text<const N: usize>() -> Zeroizing<String> {
    let mut bytes = Zeroizing::new([0u8; N]);
    getrandom::fill(bytes.as_mut_slice()).expect("the system's random source");
    Zeroizing::new(URL_SAFE_NO_PAD.encode(bytes.as_slice()))
}

/// PKCE's S256 challenge for `verifier`.
fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// Whether `uri` is one of the shapes `pattern` allows ("{port}": 1-65535).
fn redirect_matches(pattern: &str, uri: &str) -> bool {
    match pattern.split_once("{port}") {
        None => pattern == uri,
        Some((before, after)) => uri.strip_prefix(before).and_then(|rest| rest.strip_suffix(after)).is_some_and(|port| {
            !port.is_empty()
                && port.len() <= 5
                && port.bytes().all(|b| b.is_ascii_digit())
                && port.parse::<u32>().is_ok_and(|port| (1..=65535).contains(&port))
        }),
    }
}

impl Broker {
    fn recipe(&self, id: &str) -> Option<Arc<dyn Recipe>> {
        guard(&self.recipes).iter().find(|recipe| recipe.spec().id == id).cloned()
    }

    /// The services this Claktory can connect.
    pub(crate) fn catalog(&self) -> Vec<v1::Connector> {
        guard(&self.recipes).iter().map(|recipe| recipe.spec().to_proto()).collect()
    }

    /// The flows going on or ended lately, for every device.
    pub(crate) fn flows(&self) -> Vec<v1::ConnectFlow> {
        let now = now_ms();
        let mut flows = guard(&self.flows);
        flows.retain(|flow| flow.active() || now - flow.ended_ms < ENDED_MS);
        flows.iter().map(|flow| flow.view.clone()).collect()
    }

    fn lock_for(&self, connection: &str) -> Arc<tokio::sync::Mutex<()>> {
        guard(&self.locks).entry(connection.to_owned()).or_default().clone()
    }

    fn retire(&self, values: Vec<factory_agent::SecretVariable>) {
        let now = now_ms();
        let mut retired = guard(&self.retired);
        retired.retain(|(when, _)| now - when < RETIRED_MS);
        retired.extend(values.into_iter().map(|value| (now, value)));
    }

    /// A connection's tokens as values to hide, named after it.
    fn tokens(recipe: &dyn Recipe, name: &str, control: &Control) -> Vec<factory_agent::SecretVariable> {
        recipe
            .hidden(control)
            .into_iter()
            .map(|value| factory_agent::SecretVariable {
                secret: name.to_owned(),
                variable: "token".to_owned(),
                value,
                file: false,
                public: false,
                binary: false,
            })
            .collect()
    }

    /// What to hide from agents besides the vault's variables: the
    /// connections' tokens, and values exports had before.
    pub(crate) fn hidden(&self, vault: &crate::vault::Vault) -> Vec<factory_agent::SecretVariable> {
        let mut hidden = Vec::new();
        for (name, connector, control) in vault.controls() {
            let Some(recipe) = self.recipe(&connector) else { continue };
            hidden.extend(Broker::tokens(recipe.as_ref(), &name, &control));
        }
        let now = now_ms();
        hidden.extend(guard(&self.retired).iter().filter(|(when, _)| now - when < RETIRED_MS).map(|(_, value)| value.clone()));
        hidden
    }

    /// Changes a flow, if it's still there; true if it was.
    fn update(&self, id: &str, change: impl FnOnce(&mut Flow)) -> bool {
        let mut flows = guard(&self.flows);
        match flows.iter_mut().find(|flow| flow.view.id == id) {
            Some(flow) => {
                change(flow);
                true
            }
            None => false,
        }
    }

    fn end(flow: &mut Flow, state: ConnectFlowState) {
        if flow.active() {
            flow.view.state = state.into();
            flow.ended_ms = now_ms();
            flow.cancel.cancel();
        }
    }
}

// ---- What the app and admin agents ask ----

impl Factory {
    /// Lets a test connect a recipe of its own, its services' addresses
    /// replaced by `bases` (key → URL).
    #[doc(hidden)]
    pub fn add_connector(&self, recipe: Arc<dyn Recipe>, bases: &[(&str, &str)]) {
        OFFERED.store(true, std::sync::atomic::Ordering::Relaxed);
        let id = recipe.spec().id.clone();
        let mut recipes = guard(&self.broker.recipes);
        recipes.retain(|known| known.spec().id != id);
        recipes.push(recipe);
        let mut known = guard(&self.broker.http.bases);
        for (key, url) in bases {
            known.insert((*key).to_owned(), (*url).to_owned());
        }
    }

    /// Replaces a built-in recipe's service address, for tests.
    #[doc(hidden)]
    pub fn set_connector_base(&self, key: &str, url: &str) {
        guard(&self.broker.http.bases).insert(key.to_owned(), url.to_owned());
    }

    fn flows_changed(&self) {
        self.broadcast_workspace(&self.lock());
    }

    /// Starts connecting a service for the device `binding` (named
    /// `device_name`): what to open.
    pub(crate) async fn start_connect(
        self: &Arc<Self>,
        request: &v1::StartConnect,
        binding: &str,
        device_name: &str,
    ) -> Result<v1::ConnectStarted, RequestError> {
        if !self.vault.exists() {
            return Err(VaultError::NoVault.into());
        }
        if !self.vault.is_open() {
            return Err(VaultError::Locked.into());
        }
        let Some(recipe) = self.broker.recipe(&request.connector_id) else {
            return Err(RequestError::Connect(format!("Claktory can't connect \"{}\" yet.", request.connector_id)));
        };
        let spec = recipe.spec().clone();
        let Some(access) = spec.access(&request.access_id).cloned() else {
            return Err(RequestError::Connect(format!("{} doesn't offer the access \"{}\".", spec.name, request.access_id)));
        };
        let mut fields = BTreeMap::new();
        for field in &spec.fields {
            let value = request.fields.get(&field.id).map(|value| value.trim()).unwrap_or_default();
            if value.is_empty() {
                if !field.optional {
                    return Err(RequestError::Connect(format!("Enter {} to connect {}.", field.label, spec.name)));
                }
                continue;
            }
            fields.insert(field.id.clone(), value.to_owned());
        }
        let comment = crate::decisions::comment(&request.comment)?.to_owned();
        let mut proposal = None;
        if !request.proposal_id.is_empty() {
            let found = self.vault.connect_proposal(&request.proposal_id)?;
            if found.state() != v1::ConnectProposalState::Pending {
                return Err(RequestError::Connect("This suggestion was already answered.".to_owned()));
            }
            if found.connector_id != spec.id {
                return Err(RequestError::Connect("This suggestion is for a different service.".to_owned()));
            }
            proposal = Some(found);
        }
        if !request.secret_id.is_empty() {
            let opened = self.vault.connection(&request.secret_id)?;
            if opened.connector != spec.id {
                return Err(RequestError::Connect(format!("{} isn't a {} connection.", opened.name, spec.name)));
            }
        }
        let redirect_uri = request.redirect_uri.trim().to_owned();
        if spec.flow == FlowKind::Browser && !spec.redirect_uris.iter().any(|pattern| redirect_matches(pattern, &redirect_uri)) {
            return Err(RequestError::Connect(format!("{} can't return to {redirect_uri}.", spec.name)));
        }
        let id = format!("f{}", hex(&random_bytes::<6>()));
        let now = now_ms();
        let mut flow = Flow {
            view: v1::ConnectFlow {
                id: id.clone(),
                connector_id: spec.id.clone(),
                device_name: device_name.to_owned(),
                state: ConnectFlowState::Waiting.into(),
                proposal_id: request.proposal_id.clone(),
                started_ms: now,
                expires_ms: now + if spec.flow == FlowKind::Paste { PASTE_FLOW_MS } else { FLOW_MS },
                ..Default::default()
            },
            recipe: recipe.clone(),
            binding: binding.to_owned(),
            access,
            fields,
            redirect_uri,
            state: random_text::<32>(),
            verifier: Zeroizing::new(String::new()),
            challenge: String::new(),
            reconnect: request.secret_id.clone(),
            proposal,
            comment,
            pending: Control::new(String::new()),
            finished: false,
            cancel: CancellationToken::new(),
            ended_ms: 0,
        };
        flow.verifier = random_text::<48>();
        flow.challenge = challenge(&flow.verifier);
        let mut device = None;
        let started = match spec.flow {
            FlowKind::Browser => {
                let url = recipe.authorize_url(&flow.start())?;
                v1::ConnectStarted { flow_id: id.clone(), url, user_code: String::new(), expires_ms: flow.view.expires_ms }
            }
            FlowKind::Device => {
                let started_device = recipe.start_device(&self.broker.http, &flow.start()).await?;
                if started_device.expires_ms > 0 {
                    flow.view.expires_ms = flow.view.expires_ms.min(started_device.expires_ms);
                }
                let started = v1::ConnectStarted {
                    flow_id: id.clone(),
                    url: started_device.url.clone(),
                    user_code: started_device.user_code.clone(),
                    expires_ms: flow.view.expires_ms,
                };
                device = Some(started_device);
                started
            }
            FlowKind::Paste => {
                let (url, pending) = recipe.start_paste(&self.broker.http, &flow.start()).await?;
                flow.pending = pending;
                v1::ConnectStarted { flow_id: id.clone(), url, user_code: String::new(), expires_ms: flow.view.expires_ms }
            }
        };
        let (cancel, expires_ms) = (flow.cancel.clone(), flow.view.expires_ms);
        guard(&self.broker.flows).push(flow);
        if let Some(device) = device {
            tokio::spawn(self.clone().poll_device(id.clone(), device));
        }
        // It ends when it expires, unless the browser's answer came first.
        tokio::spawn({
            let factory = self.clone();
            let id = id.clone();
            async move {
                let wait = Duration::from_millis(u64::try_from(expires_ms - now_ms()).unwrap_or(0));
                tokio::select! {
                    () = tokio::time::sleep(wait) => {}
                    () = cancel.cancelled() => return,
                }
                let expired = factory.broker.update(&id, |flow| {
                    if !flow.finished {
                        Broker::end(flow, ConnectFlowState::Expired);
                    }
                });
                if expired {
                    factory.flows_changed();
                }
            }
        });
        tracing::info!(flow = %id, connector = %spec.id, "connecting a service");
        self.flows_changed();
        Ok(started)
    }

    /// Where the browser came back to, from the device that started the
    /// flow: signs in with it, in the background.
    pub(crate) fn finish_connect(self: &Arc<Self>, request: &v1::FinishConnect, binding: &str) -> Result<(), RequestError> {
        let refused = |text: &str| Err(RequestError::Connect(text.to_owned()));
        let (recipe, code) = {
            let mut flows = guard(&self.broker.flows);
            let Some(flow) = flows.iter_mut().find(|flow| flow.view.id == request.flow_id) else {
                return refused("This connection was stopped. Start again.");
            };
            if flow.binding != binding {
                return refused("This connection was started on another device. Finish it there, or start again here.");
            }
            if !flow.active() || flow.finished {
                return refused("This connection is already done.");
            }
            match flow.recipe.spec().flow {
                FlowKind::Browser => {}
                FlowKind::Device => return refused("This connection finishes by itself once you approve it."),
                FlowKind::Paste => {
                    let token = Zeroizing::new(request.token.trim().to_owned());
                    if token.is_empty() {
                        return refused("Paste the token to connect.");
                    }
                    flow.finished = true;
                    flow.view.error.clear();
                    flow.view.state = ConnectFlowState::Exchanging.into();
                    let recipe = flow.recipe.clone();
                    drop(flows);
                    self.flows_changed();
                    tokio::spawn(self.clone().pasted(request.flow_id.clone(), recipe, token));
                    return Ok(());
                }
            }
            let Ok(url) = reqwest::Url::parse(request.callback_url.trim()) else {
                return refused("That address doesn't belong to this connection.");
            };
            let mut base = url.clone();
            base.set_query(None);
            base.set_fragment(None);
            let expected = reqwest::Url::parse(&flow.redirect_uri).map(|expected| expected.to_string()).unwrap_or_default();
            if base.as_str() != expected {
                return refused("That address doesn't belong to this connection.");
            }
            let query: HashMap<String, String> = url.query_pairs().into_owned().collect();
            let name = flow.recipe.spec().name.clone();
            if let Some(error) = query.get("error") {
                let text = match error.as_str() {
                    "access_denied" => format!("{name} didn't allow access."),
                    _ => match query.get("error_description").filter(|text| !text.trim().is_empty()) {
                        Some(description) => format!("{name} said: {}", description.trim()),
                        None => format!("{name} refused ({error})."),
                    },
                };
                flow.view.error = text;
                Broker::end(flow, ConnectFlowState::Failed);
                drop(flows);
                self.flows_changed();
                return Ok(());
            }
            if !query.get("state").is_some_and(|state| equal_in_constant_time(state, &flow.state)) {
                return refused("That address doesn't belong to this connection. Start again.");
            }
            let Some(code) = query.get("code").filter(|code| !code.is_empty()) else {
                return refused("That address has no sign-in code. Start again.");
            };
            flow.finished = true;
            flow.view.state = ConnectFlowState::Exchanging.into();
            (flow.recipe.clone(), Zeroizing::new(code.clone()))
        };
        self.flows_changed();
        let factory = self.clone();
        let id = request.flow_id.clone();
        tokio::spawn(async move {
            let Some((owned, cancel)) = factory.flow_start(&id) else { return };
            let start = owned.start();
            let exchanged = tokio::select! {
                exchanged = recipe.exchange(&factory.broker.http, &start, &code, &owned.verifier) => exchanged,
                () = cancel.cancelled() => return,
            };
            match exchanged {
                Ok(control) => factory.connected(&id, control).await,
                Err(problem) => factory.flow_failed(&id, problem.text()),
            }
        });
        Ok(())
    }

    /// PASTE: checks the token pasted with the service, and keeps it; one it
    /// doesn't accept leaves the flow waiting for another, saying why.
    async fn pasted(self: Arc<Self>, id: String, recipe: Arc<dyn Recipe>, token: Zeroizing<String>) {
        let Some((owned, cancel)) = self.flow_start(&id) else { return };
        let start = owned.start();
        let checked = tokio::select! {
            checked = recipe.paste(&self.broker.http, &start, &owned.pending, &token) => checked,
            () = cancel.cancelled() => return,
        };
        match checked {
            Ok(control) => self.connected(&id, control).await,
            Err(problem) => {
                tracing::info!(flow = %id, error = problem.text(), "a pasted token didn't work");
                if self.broker.update(&id, |flow| {
                    if flow.active() {
                        flow.finished = false;
                        flow.view.error = problem.text().to_owned();
                        flow.view.state = ConnectFlowState::Waiting.into();
                    }
                }) {
                    self.flows_changed();
                }
            }
        }
    }

    /// A copy of what a flow needs to go on, outside the lock.
    fn flow_start(&self, id: &str) -> Option<(OwnedStart, CancellationToken)> {
        let flows = guard(&self.broker.flows);
        let flow = flows.iter().find(|flow| flow.view.id == id)?;
        Some((
            OwnedStart {
                access: flow.access.clone(),
                fields: flow.fields.clone(),
                redirect_uri: flow.redirect_uri.clone(),
                state: flow.state.clone(),
                verifier: flow.verifier.clone(),
                challenge: flow.challenge.clone(),
                flow: flow.view.id.clone(),
                proposal: flow.proposal.clone(),
                pending: Control(flow.pending.0.clone()),
            },
            flow.cancel.clone(),
        ))
    }

    async fn poll_device(self: Arc<Self>, id: String, device: Device) {
        let Some((owned, cancel)) = self.flow_start(&id) else { return };
        let start = owned.start();
        let recipe = match guard(&self.broker.flows).iter().find(|flow| flow.view.id == id) {
            Some(flow) => flow.recipe.clone(),
            None => return,
        };
        let mut interval = device.interval.max(Duration::from_secs(1));
        loop {
            tokio::select! {
                () = tokio::time::sleep(interval) => {}
                () = cancel.cancelled() => return,
            }
            let polled = tokio::select! {
                polled = recipe.poll_device(&self.broker.http, &start, &device.pending) => polled,
                () = cancel.cancelled() => return,
            };
            match polled {
                Ok(Poll::Pending) => {}
                Ok(Poll::SlowDown) => interval += Duration::from_secs(5),
                Ok(Poll::Done(control)) => {
                    self.broker.update(&id, |flow| {
                        flow.finished = true;
                        flow.view.state = ConnectFlowState::Exchanging.into();
                    });
                    self.connected(&id, control).await;
                    return;
                }
                Err(problem) => {
                    self.flow_failed(&id, problem.text());
                    return;
                }
            }
        }
    }

    fn flow_failed(&self, id: &str, text: &str) {
        tracing::info!(flow = %id, error = text, "connecting a service failed");
        if self.broker.update(id, |flow| {
            flow.view.error = text.to_owned();
            Broker::end(flow, ConnectFlowState::Failed);
        }) {
            self.flows_changed();
        }
    }

    /// The service gave a flow its tokens: checks what they reach, keeps
    /// them in the vault, and tells whoever proposed it, and Rivet.
    async fn connected(self: &Arc<Self>, id: &str, control: Control) {
        let found = {
            let flows = guard(&self.broker.flows);
            flows.iter().find(|flow| flow.view.id == id).map(|flow| {
                let proposal = (flow.view.proposal_id.clone(), flow.comment.clone());
                (flow.recipe.clone(), flow.reconnect.clone(), proposal, flow.access.id.clone(), flow.cancel.clone())
            })
        };
        let Some((recipe, reconnect, (proposal_id, comment), access_id, cancel)) = found else { return };
        self.broker.update(id, |flow| flow.view.state = ConnectFlowState::Checking.into());
        self.flows_changed();
        let inspected = tokio::select! {
            inspected = recipe.inspect(&self.broker.http, &control) => inspected,
            () = cancel.cancelled() => return,
        };
        let inspected = match inspected {
            Ok(inspected) => inspected,
            Err(problem) => return self.flow_failed(id, problem.text()),
        };
        let spec = recipe.spec();
        let mut meta = ConnectionMeta {
            subject: inspected.subject.clone(),
            account: inspected.account.clone(),
            access: inspected.access.clone(),
            scopes: inspected.scopes.clone(),
            resources: inspected.resources.clone(),
            expires_ms: inspected.expires_ms,
            checked_ms: now_ms(),
            problem: String::new(),
            proposal_id: proposal_id.clone(),
            access_id,
        };
        let saved = if reconnect.is_empty() {
            let name = if inspected.account.is_empty() { spec.name.clone() } else { format!("{} ({})", spec.name, inspected.account) };
            self.vault.add_connection(&name, &describe(&spec.name, &meta.access), &spec.id, &meta, &control)
        } else {
            match self.vault.connection(&reconnect) {
                Ok(before) if !before.meta.subject.is_empty() && before.meta.subject != meta.subject => Err(VaultError::Invalid(format!(
                    "That's a different {} account from {}'s. Sign in with the same account, or connect it as a new one.",
                    spec.name, before.name
                ))),
                Ok(before) => {
                    if meta.proposal_id.is_empty() {
                        meta.proposal_id = before.meta.proposal_id.clone();
                    }
                    self.vault.set_connection(&before.id, &meta, Some(&control)).map(|()| (before.id, before.name))
                }
                Err(error) => Err(error),
            }
        };
        let (secret_id, secret_name) = match saved {
            Ok(saved) => saved,
            Err(error) => {
                // Nothing kept: don't leave a grant at the service no one can use.
                if reconnect.is_empty()
                    && let Err(problem) = recipe.revoke(&self.broker.http, &control).await
                {
                    tracing::warn!(error = problem.text(), "could not revoke a connection that couldn't be kept");
                }
                let text = match error {
                    VaultError::Locked => {
                        "Secrets were locked before the connection could be saved. Unlock them and connect again.".to_owned()
                    }
                    other => other.to_string(),
                };
                return self.flow_failed(id, &text);
            }
        };
        tracing::info!(flow = %id, connection = %secret_id, "service connected");
        self.broker.update(id, |flow| {
            flow.view.secret_id = secret_id.clone();
            Broker::end(flow, ConnectFlowState::Connected);
        });
        if !proposal_id.is_empty() {
            proposals::connected(self, &proposal_id, (&secret_id, &secret_name), spec, &meta, &comment);
        }
        self.vault_changed();
    }

    /// Stops a flow, or takes an ended one off the list.
    pub(crate) fn cancel_connect(&self, id: &str) {
        let mut flows = guard(&self.broker.flows);
        match flows.iter_mut().find(|flow| flow.view.id == id) {
            Some(flow) if flow.active() => Broker::end(flow, ConnectFlowState::Cancelled),
            Some(_) => flows.retain(|flow| flow.view.id != id),
            None => return,
        }
        drop(flows);
        self.flows_changed();
    }

    /// Ends the flows a device started: it was unpaired.
    pub(crate) fn cancel_device_flows(&self, device: &str) {
        let mut changed = false;
        for flow in guard(&self.broker.flows).iter_mut().filter(|flow| flow.binding == device && flow.active()) {
            Broker::end(flow, ConnectFlowState::Cancelled);
            changed = true;
        }
        if changed {
            self.flows_changed();
        }
    }

    /// A connection opened, its tokens refreshed if they needed it, with
    /// its recipe; the lock held keeps any other refresh or renewal of it
    /// waiting.
    async fn fresh(&self, id: &str) -> Result<(Opened, Arc<dyn Recipe>, tokio::sync::OwnedMutexGuard<()>), RequestError> {
        let id = self.vault.connection(id)?.id;
        let held = self.broker.lock_for(&id).lock_owned().await;
        let mut opened = self.vault.connection(&id)?;
        let Some(recipe) = self.broker.recipe(&opened.connector) else {
            return Err(RequestError::Connect(format!("This Claktory can no longer use {}.", opened.name)));
        };
        match recipe.refresh(&self.broker.http, &opened.control).await {
            Ok(None) => {}
            Ok(Some(control)) => {
                // Kept before it's used: a refresh token may be good once only.
                self.vault.set_control(&id, &control)?;
                opened.meta.checked_ms = now_ms();
                opened.meta.problem.clear();
                self.vault.set_connection(&id, &opened.meta, None)?;
                self.broker.retire(Broker::tokens(recipe.as_ref(), &opened.name, &opened.control));
                opened.control = control;
                self.vault_changed();
            }
            Err(problem) => return Err(self.recipe_failed(&opened, problem)),
        }
        Ok((opened, recipe, held))
    }

    /// What a recipe's problem with a connection means for the asker; one
    /// the service no longer accepts is marked as needing connecting again.
    fn recipe_failed(&self, opened: &Opened, problem: Problem) -> RequestError {
        let Problem::Reconnect(why) = problem else { return problem.into() };
        let mut meta = opened.meta.clone();
        meta.problem = why.clone();
        match self.vault.set_connection(&opened.id, &meta, None) {
            Ok(()) => self.vault_changed(),
            Err(error) => tracing::warn!(%error, connection = %opened.id, "could not mark a connection as needing connecting again"),
        }
        RequestError::Connect(format!("{} needs to be connected again. {why}", opened.name))
    }

    /// What a secret can be made of from a connection.
    pub(crate) async fn connect_choices(&self, secret_id: &str) -> Result<v1::ConnectChoices, RequestError> {
        let (opened, recipe, _held) = self.fresh(secret_id).await?;
        let choices = recipe.choices(&self.broker.http, &opened.control).await.map_err(|problem| self.recipe_failed(&opened, problem))?;
        Ok(v1::ConnectChoices {
            secret_id: opened.id,
            choices: choices
                .choices
                .into_iter()
                .map(|choice| v1::ConnectChoice {
                    kind: choice.kind,
                    id: choice.id,
                    label: choice.label,
                    parent: choice.parent,
                    why: choice.why,
                })
                .collect(),
            rules: choices.rules,
        })
    }

    /// Makes a secret from a connection, given to no one; returns its id
    /// and name. `by` is the admin agent that asked, '' for the person.
    pub(crate) async fn derive_secret(&self, request: &v1::DeriveSecret, by: &str) -> Result<(String, String), RequestError> {
        let (opened, recipe, _held) = self.fresh(&request.secret_id).await?;
        let ask = Derive {
            name: request.name.trim().to_owned(),
            permissions: request.permissions.clone(),
            resources: request.resources.clone(),
            expires_ms: request.expires_ms,
        };
        let derived =
            recipe.derive(&self.broker.http, &opened.control, &ask).await.map_err(|problem| self.recipe_failed(&opened, problem))?;
        let Some(values) = derived.values.filter(|values| !values.is_empty()) else {
            return Err(RequestError::Connect(format!("{} didn't allow anything a secret could be made from.", recipe.spec().name)));
        };
        let values: Vec<Value> =
            values.into_iter().map(|variable| Value { variable: variable.name, value: variable.value, file: false, binary: false }).collect();
        let name = if ask.name.is_empty() { format!("{} access", recipe.spec().name) } else { ask.name.clone() };
        // Unless said otherwise, it says what it grants, as checked.
        let description = match request.description.trim() {
            "" => describe(&recipe.spec().name, &derived.meta.access),
            given => given.to_owned(),
        };
        match self.vault.add_export(&opened.id, &name, &description, &values, &derived.meta) {
            Ok(made) => {
                tracing::info!(connection = %opened.id, secret = %made.0, by, "made a secret from a connection");
                self.vault_changed();
                Ok(made)
            }
            Err(error) => {
                // Not kept: remove what the service made for it.
                if let Err(problem) = recipe.remove_exports(&self.broker.http, &opened.control, std::slice::from_ref(&derived.meta)).await {
                    tracing::warn!(error = problem.text(), "could not remove a token that couldn't be kept");
                }
                Err(error.into())
            }
        }
    }

    /// Renews what `agent`'s command asks for among secrets made from a
    /// connection, when it's due, before the command gets it. Why not, in
    /// words for the agent, only when what it has no longer works.
    pub(crate) async fn renew_for(&self, agent: &str, names: &[String]) -> Result<(), String> {
        let mut renewed = false;
        for export in self.vault.granted_exports(agent, names) {
            let due = |meta: &ExportMeta| meta.renew_at_ms > 0 && now_ms() >= meta.renew_at_ms;
            if !due(&export.meta) {
                continue;
            }
            let outcome = async {
                let (opened, recipe, _held) = self.fresh(&export.connection).await?;
                // Another command may have renewed it meanwhile.
                let Some(current) = self.vault.export(&export.id)?.filter(|current| due(&current.meta)) else { return Ok(false) };
                let derived = recipe
                    .renew(&self.broker.http, &opened.control, &current.meta)
                    .await
                    .map_err(|problem| self.recipe_failed(&opened, problem))?;
                let values: Option<Vec<Value>> = derived.values.map(|values| {
                    values.into_iter().map(|variable| Value { variable: variable.name, value: variable.value, file: false, binary: false }).collect()
                });
                if values.is_some() {
                    let before = self.vault.values(&current.id)?;
                    self.broker.retire(
                        before
                            .into_iter()
                            .filter(|value| !current.meta.public.contains(&value.variable))
                            .map(|value| factory_agent::SecretVariable {
                                secret: current.name.clone(),
                                variable: value.variable,
                                value: value.value,
                                file: value.file,
                                public: false,
                                binary: value.binary,
                            })
                            .collect(),
                    );
                }
                self.vault.set_export(&current.id, values.as_deref(), &derived.meta)?;
                // A new token in its place: the old one goes at the service.
                if values.is_some()
                    && derived.meta.recipe != current.meta.recipe
                    && let Err(problem) =
                        recipe.remove_exports(&self.broker.http, &opened.control, std::slice::from_ref(&current.meta)).await
                {
                    tracing::warn!(error = problem.text(), secret = %current.name, "could not remove a renewed secret's old token");
                }
                Ok::<bool, RequestError>(true)
            }
            .await;
            match outcome {
                Ok(changed) => renewed |= changed,
                Err(error) => {
                    let still_good = export.meta.expires_ms > 0 && now_ms() < export.meta.expires_ms;
                    tracing::warn!(secret = %export.name, %error, still_good, "could not renew a secret made from a connection");
                    if !still_good {
                        if renewed {
                            self.vault_changed();
                        }
                        return Err(format!(
                            "{} couldn't be renewed: {error} Ask the person to check its connection in Settings → Secrets.",
                            export.name
                        ));
                    }
                }
            }
        }
        if renewed {
            self.vault_changed();
        }
        Ok(())
    }

    /// Removes a connection and the secrets made from it, removing their
    /// tokens at the service where it can, and with `revoke`, the
    /// connection too. A revoke that fails keeps everything as it was.
    pub(crate) async fn disconnect_service(&self, secret_id: &str, revoke: bool) -> Result<(), RequestError> {
        let opened = self.vault.connection(secret_id)?;
        let exports = self.vault.exports_of(&opened.id)?;
        match self.fresh(&opened.id).await {
            Ok((opened, recipe, _held)) => {
                let metas: Vec<ExportMeta> = exports.iter().map(|export| export.meta.clone()).collect();
                if let Err(problem) = recipe.remove_exports(&self.broker.http, &opened.control, &metas).await {
                    tracing::warn!(error = problem.text(), "could not remove a connection's tokens at its service");
                }
                if revoke {
                    recipe.revoke(&self.broker.http, &opened.control).await.map_err(|problem| {
                        RequestError::Connect(format!(
                            "{} couldn't be cancelled at {}: {} It's still here. Try again, or disconnect it without cancelling.",
                            opened.name,
                            recipe.spec().name,
                            problem.text()
                        ))
                    })?;
                }
            }
            Err(error) if revoke => return Err(error),
            Err(error) => tracing::warn!(%error, "disconnecting a service that can't be reached"),
        }
        let (names, agents) = self.vault.remove_connection(&opened.id)?;
        tracing::info!(connection = %opened.id, removed = ?names, had = ?agents, revoke, "service disconnected");
        self.vault_changed();
        Ok(())
    }

    /// Deletes a secret made from a connection, removing its token at the
    /// service where there is one.
    pub(crate) async fn remove_export(&self, export: &crate::vault::connections::Export) -> Result<(), RequestError> {
        match self.fresh(&export.connection).await {
            Ok((opened, recipe, _held)) => {
                if let Err(problem) = recipe.remove_exports(&self.broker.http, &opened.control, std::slice::from_ref(&export.meta)).await {
                    tracing::warn!(error = problem.text(), secret = %export.name, "could not remove a secret's token at its service");
                }
            }
            Err(error) => tracing::warn!(%error, secret = %export.name, "deleting a secret whose connection can't be reached"),
        }
        self.vault.delete_secret(&export.id)?;
        self.vault_changed();
        Ok(())
    }
}

/// What a flow's background work holds, apart from the flow.
struct OwnedStart {
    access: AccessSpec,
    fields: BTreeMap<String, String>,
    redirect_uri: String,
    state: Zeroizing<String>,
    verifier: Zeroizing<String>,
    challenge: String,
    flow: String,
    proposal: Option<v1::ConnectProposal>,
    /// PASTE: what `start_paste` kept.
    pending: Control,
}

impl OwnedStart {
    fn start(&self) -> Start<'_> {
        Start {
            access: &self.access,
            fields: &self.fields,
            redirect_uri: &self.redirect_uri,
            state: &self.state,
            challenge: &self.challenge,
            flow: &self.flow,
            proposal: self.proposal.as_ref(),
        }
    }
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).expect("the system's random source");
    bytes
}

/// What a secret made from (or a connection to) `service` grants, in a
/// line, from what was checked: "Cloudflare: DNS Edit on claktory.com".
pub(crate) fn describe(service: &str, access: &[String]) -> String {
    const LONGEST: usize = 300;
    if access.is_empty() {
        return String::new();
    }
    let text = format!("{service}: {}", access.join("; "));
    if text.chars().count() <= LONGEST {
        return text;
    }
    format!("{}…", text.chars().take(LONGEST - 1).collect::<String>())
}

/// A random id, 16 hex digits.
pub(crate) fn random_id() -> String {
    hex(&random_bytes::<8>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_match_their_shapes_and_nothing_else() {
        let loopback = "http://127.0.0.1:{port}/oauth/cloudflare";
        assert!(redirect_matches(loopback, "http://127.0.0.1:53682/oauth/cloudflare"));
        for wrong in [
            "http://127.0.0.1:/oauth/cloudflare",
            "http://127.0.0.1:0/oauth/cloudflare",
            "http://127.0.0.1:99999/oauth/cloudflare",
            "http://127.0.0.1:80@evil.com/oauth/cloudflare",
            "http://127.0.0.1:8080/oauth/cloudflare/x",
            "http://localhost:8080/oauth/cloudflare",
        ] {
            assert!(!redirect_matches(loopback, wrong), "{wrong}");
        }
        assert!(redirect_matches("com.claktory.app:/oauth/cloudflare", "com.claktory.app:/oauth/cloudflare"));
        assert!(!redirect_matches("com.claktory.app:/oauth/cloudflare", "com.claktory.app.dev:/oauth/cloudflare"));
    }

    /// Nothing about connecting shows until a service is built in: not the
    /// Secrets extension's option, nor (through it) the tool.
    #[test]
    fn connecting_shows_only_with_a_service() {
        let broker = Broker::default();
        assert_eq!(offered(), !broker.catalog().is_empty());
        let manifest = crate::extensions::Extension::manifest(&crate::extensions::secrets::Secrets);
        assert_eq!(manifest.options.iter().any(|option| option.id == proposals::OPTION), offered());
    }

    #[test]
    fn pkce_follows_rfc_7636() {
        // RFC 7636, appendix B.
        assert_eq!(challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        let verifier = random_text::<48>();
        assert_eq!(verifier.len(), 64);
        assert!(equal_in_constant_time("abc", "abc") && !equal_in_constant_time("abc", "abd") && !equal_in_constant_time("abc", "ab"));
    }
}
