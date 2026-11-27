//! Fills from the vault (#435 W6, agent-browser.md §5.3, Fugo 40552): an
//! errand's agent names a field and a secret, and Claktory puts the
//! secret's value into that field, never into the agent's hands.
//!
//! - **Where:** only into a field of a page whose origin, as Chrome says it
//!   (the frame's securityOrigin, never the page's or the agent's word), is
//!   one of the secret's sites, exact (`vault::sites`).
//! - **What:** only into an `input` that's a password field (type
//!   password, or autocomplete current-password or new-password), a
//!   one-time code's, or a username's (type email, or autocomplete
//!   username or email), and a username's only the secret's username or
//!   email: never a password into a field that could publish it (a bio, a
//!   support message).
//! - **How:** by node, not by focus. The reader checks the field is still
//!   the one read, visible, enabled, on screen and not covered, and hands it
//!   over by its object in the reader's isolated world, which the page
//!   can't see; the frame's id and loader are checked again just before the
//!   value is set through the native setter, and `input` and `change` are
//!   sent, so nothing a page script does with focus can send it elsewhere.
//! - **What's told:** the agent gets "filled", the number of characters and
//!   the field's kind; the vault's `uses` and the errand's log get where,
//!   never the value.
//! - **Identity providers** (Google, Apple, Microsoft): on their sign-in
//!   origins only Claktory fills, and the agent may only press Next,
//!   Continue or Sign in (`on_identity_provider`); no one-time code from the
//!   vault there; and one account per provider in the browser. The first
//!   sign-in there puts a card in the agent's conversation, with Sign out
//!   (`super::code::signed_in`); while a sign-out waits for the errand to
//!   end, nothing is filled or pressed on that provider's sites.

use std::sync::Arc;

use serde_json::{Value, json};

use super::reader::Page;
use super::{log, said};
use crate::factory::Factory;
use crate::vault::sites::Fillable;

/// The tool.
pub(crate) const TOOL: &str = "browser_fill_secret";

/// An identity provider: its sign-in origins (Fugo 40552), and the domains
/// whose cookies hold a sign-in there, which signing out deletes (Fugo
/// 41236, Giorno 41238).
pub(crate) struct IdentityProvider {
    pub name: &'static str,
    pub origins: &'static [&'static str],
    /// Each with all its subdomains; Google's also any domain whose
    /// registrable part starts "google." (google.co.uk, google.de), in
    /// `owner`.
    pub cookie_domains: &'static [&'static str],
}

pub(crate) const IDENTITY_PROVIDERS: &[IdentityProvider] = &[
    IdentityProvider { name: "Google", origins: &["https://accounts.google.com"], cookie_domains: &["google.com", "youtube.com"] },
    IdentityProvider {
        name: "Apple",
        origins: &["https://appleid.apple.com", "https://idmsa.apple.com"],
        cookie_domains: &["apple.com", "icloud.com"],
    },
    IdentityProvider {
        name: "Microsoft",
        origins: &["https://login.microsoftonline.com", "https://login.live.com"],
        cookie_domains: &["microsoftonline.com", "live.com", "microsoft.com", "office.com"],
    },
];

#[cfg(all(feature = "qa-idp-fill", not(debug_assertions)))]
compile_error!("The qa-idp-fill feature is for QA builds only: a release build never carries it.");

/// Whether the vault fills on an identity provider's sign-in. Its ship
/// gate (Fugo 41161): not until the person is told of the browser's first
/// sign-in there, with Sign out. Until then only a QA build with the
/// `qa-idp-fill` feature does, for the early Google sign-in test.
pub(crate) fn identity_provider_fills() -> bool {
    cfg!(feature = "qa-idp-fill")
}

/// Whose sign-in `origin` is, if it's an identity provider's.
pub(crate) fn provider(origin: &str) -> Option<&'static str> {
    IDENTITY_PROVIDERS.iter().find(|known| known.origins.contains(&origin)).map(|known| known.name)
}

/// Whose sign-in cookies `host`'s are (a cookie's domain, with or without
/// its leading dot, or a page's host), if an identity provider's: what
/// signing out of it deletes, and where nothing is filled or pressed while
/// a sign-out waits.
pub(crate) fn owner(host: &str) -> Option<&'static str> {
    let host = host.trim_start_matches('.').trim_end_matches('.').to_ascii_lowercase();
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    if let Some(known) = IDENTITY_PROVIDERS.iter().find(|known| known.cookie_domains.iter().any(|domain| under(domain))) {
        return Some(known.name);
    }
    // google.co.uk, www.google.de: Google's own in each country.
    psl::domain_str(&host).is_some_and(|registrable| registrable.split('.').next() == Some("google")).then_some("Google")
}

/// Refused on `origin` (a page's or a field's) while the browser waits to
/// sign out of its identity provider (`signing_out`, the host's word):
/// nothing is filled or pressed there until it has (Fugo 41236).
pub(crate) fn while_signing_out(origin: &str, signing_out: impl Fn(&str) -> bool) -> Option<(String, String)> {
    let host = url::Url::parse(origin).ok()?.host_str()?.to_owned();
    let provider = owner(&host).filter(|provider| signing_out(provider))?;
    Some(("sensitive_action".to_owned(), format!("signing out of {provider} when this errand ends: nothing is filled or pressed on its sites until then")))
}

/// What a control the agent may press on an identity provider's page says:
/// moving the sign-in on, in the languages the browser's pages may be in.
const ADVANCES: &[&str] = &[
    "next",
    "continue",
    "sign in",
    "log in",
    "login",
    "weiter",
    "anmelden",
    "suivant",
    "continuer",
    "se connecter",
    "siguiente",
    "continuar",
    "iniciar sesión",
    "avanti",
    "accedi",
    "volgende",
    "doorgaan",
    "inloggen",
    "próximo",
    "entrar",
];

/// Whether a control's words move a sign-in on and nothing else.
fn advances(words: &str) -> bool {
    let words = words.trim().trim_end_matches(['.', '…', '>', '›', '→']).trim().to_lowercase();
    ADVANCES.contains(&words.as_str())
}

/// On an identity provider's sign-in page, what the agent may do there
/// itself (Fugo 40552): press Next, Continue or Sign in. A fill is
/// Claktory's, from the vault; anything else (consent, another way to
/// sign in, recovery, security settings, passkeys) is refused, for the
/// agent to report. `kind` is "click" or "fill"; `action` the reader's
/// description of the target; `main_frame`, whether the target is in the
/// page's main frame. None when it's allowed, or `origin` isn't an identity
/// provider's.
pub(crate) fn on_identity_provider(origin: &str, main_frame: bool, kind: &str, action: &Value) -> Option<(String, String)> {
    let provider = provider(origin)?;
    // Only the page's own sign-in, never one framed in another (Fugo 40552).
    if !main_frame {
        return Some(("sensitive_action".to_owned(), format!("{provider}'s sign-in is used only as the page itself, never in a frame of another page")));
    }
    if kind == "fill" {
        return Some((
            "sensitive_action".to_owned(),
            format!("on {provider}'s sign-in only Claktory fills a field, from the vault: use {TOOL}"),
        ));
    }
    let name = action["name"].as_str().unwrap_or_default();
    let shows = action["shows"].as_str().unwrap_or_default();
    if advances(name) && (shows.is_empty() || advances(shows)) {
        return None;
    }
    Some((
        "sensitive_action".to_owned(),
        format!(
            "on {provider}'s sign-in you may only press Next, Continue or Sign in: anything else there is the person's, so stop and report what the page asks"
        ),
    ))
}

/// What kind of field the reader handed over, asked of the node itself in
/// the reader's world: "password", "one-time-code", "username", or why it
/// takes no secret.
const FIELD: &str = r#"function () {
  if (!(this instanceof HTMLInputElement)) return 'other';
  if (!this.isConnected || this.ownerDocument !== document || this.disabled || this.readOnly) return 'gone';
  const type = (this.type || 'text').toLowerCase();
  const tokens = (this.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
  if (type === 'password' || tokens.includes('current-password') || tokens.includes('new-password')) return 'password';
  if (tokens.includes('one-time-code')) return 'one-time-code';
  if (type === 'email' || tokens.includes('username') || tokens.includes('email')) return 'username';
  return 'other';
}"#;

/// Puts `value` into the node through the native setter, as typing would
/// leave it, and tells the page's listeners.
pub(crate) const SET: &str = r#"function (value) {
  if (!this.isConnected || this.ownerDocument !== document) return false;
  this.focus();
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(this, value);
  this.dispatchEvent(new Event('input', {bubbles: true}));
  this.dispatchEvent(new Event('change', {bubbles: true}));
  return true;
}"#;

/// Whether the field still holds `value` once the page had a moment to
/// react: true or false, never the value.
pub(crate) const KEPT: &str = r#"function (value) { return this.value === value; }"#;

/// A field's kind, from FIELD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Field {
    Password,
    OneTimeCode,
    Username,
}

impl Field {
    fn of(word: &str) -> Option<Field> {
        Some(match word {
            "password" => Field::Password,
            "one-time-code" => Field::OneTimeCode,
            "username" => Field::Username,
            _ => return None,
        })
    }

    fn word(self) -> &'static str {
        match self {
            Field::Password => "password",
            Field::OneTimeCode => "one-time code",
            Field::Username => "username",
        }
    }
}

/// Whether a variable holds a username or an email address, by its name:
/// USERNAME, USER, LOGIN, EMAIL, or one ending in _USERNAME and so on.
pub(crate) fn names_a_user(variable: &str) -> bool {
    let variable = variable.to_ascii_uppercase();
    ["USERNAME", "USER", "LOGIN", "EMAIL", "USER_NAME", "E_MAIL"]
        .iter()
        .any(|word| variable == *word || variable.ends_with(&format!("_{word}")))
}

/// Which of `variables` goes into a `field`: the one asked for, if the
/// field takes it; else the only one that fits. Or why not, for the agent.
pub(crate) fn choose<'a>(secret: &str, variables: &'a [String], asked: Option<&str>, field: Field) -> Result<&'a str, String> {
    let fits = |variable: &str| field != Field::Username || names_a_user(variable);
    let listed = variables.join(", ");
    if let Some(asked) = asked.map(str::trim).filter(|asked| !asked.is_empty()) {
        let Some(found) = variables.iter().find(|variable| variable.eq_ignore_ascii_case(asked)) else {
            return Err(format!("{secret} has no variable {asked}: it has {listed}."));
        };
        if !fits(found) {
            return Err(format!("{found} isn't a username or email, and a username field takes only that: name {secret}'s username variable."));
        }
        return Ok(found);
    }
    let candidates: Vec<&String> = match field {
        Field::Username => variables.iter().filter(|variable| names_a_user(variable)).collect(),
        Field::Password | Field::OneTimeCode if variables.len() == 1 => variables.iter().collect(),
        Field::Password | Field::OneTimeCode => variables.iter().filter(|variable| !names_a_user(variable)).collect(),
    };
    match candidates.as_slice() {
        [one] => Ok(one.as_str()),
        [] if field == Field::Username => Err(format!("{secret} has no username or email variable ({listed}), so it fills no username field.")),
        _ => Err(format!("{secret} has {listed}: say which goes into this {} field with variable.", field.word())),
    }
}

/// A fill that was made.
#[derive(Debug)]
pub(crate) struct Filled {
    /// Chrome's origin of the field's document.
    pub origin: String,
    pub field: Field,
    /// How many characters went in.
    pub count: usize,
    /// Whether the field still held them a moment later.
    pub kept: bool,
}

/// Fills `target` of `document` on `page` with `fillable`'s `variable` (or
/// the one that fits the field), checking the field's origin (Chrome's) is
/// one of its sites and its kind takes a secret; on an identity provider's
/// origin, never a code, and never a secret other than `account` (the
/// browser's account there, if it has one); and nowhere on a provider's
/// sites while `signing_out` of it. Or why not, with the origin where it
/// was refused (empty before it's known).
#[allow(clippy::too_many_arguments)]
pub(crate) async fn fill_into(
    page: &Arc<Page>,
    document: &str,
    target: &str,
    fillable: &Fillable,
    variable: Option<&str>,
    account: impl Fn(&str) -> Option<String>,
    signing_out: impl Fn(&str) -> bool,
    identity_providers: bool,
) -> Result<Filled, (String, (String, String))> {
    let (object, frame) = page.element(document, target).await.map_err(|refused| (String::new(), refused))?;
    fill_node(page, &object, &frame, fillable, variable, account, signing_out, identity_providers).await
}

/// Chrome's origin of a frame as the vault's sites spell them: an https
/// origin only, else empty.
fn https_origin(origin: &str) -> String {
    url::Url::parse(origin).ok().filter(|url| url.scheme() == "https").map(|url| url.origin().ascii_serialization()).unwrap_or_default()
}

/// Whether a field whose frame is at `origin` (`https_origin`'s) may take
/// a secret bound to `sites`: exactly one of them, never http, a subdomain
/// or another port (Fugo 50018).
fn fills_on(origin: &str, sites: &[String]) -> bool {
    !origin.is_empty() && sites.iter().any(|site| site == origin)
}

/// `fill_into`, for the node `object` (in the reader's world) of `frame`.
/// On an identity provider's origin only when `identity_providers` (its
/// ship gate, `identity_provider_fills`) and only in the page's main frame.
#[allow(clippy::too_many_arguments)]
async fn fill_node(
    page: &Arc<Page>,
    object: &str,
    frame: &super::reader::Frame,
    fillable: &Fillable,
    variable: Option<&str>,
    account: impl Fn(&str) -> Option<String>,
    signing_out: impl Fn(&str) -> bool,
    identity_providers: bool,
) -> Result<Filled, (String, (String, String))> {
    // Chrome's word for whose the field's document is: an https origin,
    // exactly one of the secret's sites.
    let origin = https_origin(&frame.origin);
    let refuse = |error: &str, why: String| (origin.clone(), (error.to_owned(), why));
    if !fills_on(&origin, &fillable.sites) {
        let shown = if origin.is_empty() { "this page".to_owned() } else { origin.clone() };
        return Err(refuse("wrong_site", format!("{} is only filled on {}, and this field is on {shown}", fillable.name, fillable.sites.join(", "))));
    }
    if let Some(refused) = while_signing_out(&origin, signing_out) {
        return Err((origin.clone(), refused));
    }
    let call = |declaration: &'static str, arguments: Vec<Value>| {
        let page = page.clone();
        let object = object.to_owned();
        async move {
            page.call(
                "Runtime.callFunctionOn",
                json!({"objectId": object, "functionDeclaration": declaration, "arguments": arguments, "returnByValue": true}),
            )
            .await
        }
    };
    let gone = || refuse("stale_target", "the field went: read the page again".to_owned());
    let kind = call(FIELD, Vec::new()).await.map_err(|_| gone())?;
    let field = match kind["result"]["value"].as_str().unwrap_or_default() {
        "gone" => return Err(gone()),
        word => Field::of(word)
            .ok_or_else(|| refuse("unsupported_target", "only a password, one-time code, username or email field takes a secret".to_owned()))?,
    };
    if let Some(provider) = provider(&origin) {
        if !identity_providers {
            return Err(refuse(
                "sensitive_action",
                format!("Claktory doesn't sign in to {provider} from the vault yet: stop and report that the errand needs a {provider} sign-in"),
            ));
        }
        // Only the page's own sign-in, never one framed in it (Fugo 40552).
        let main = frame.parent.is_empty() && page.frame().await.is_ok_and(|top| top.id == frame.id && top.parent.is_empty());
        if !main {
            return Err(refuse("sensitive_action", format!("{provider}'s sign-in is filled only as the page itself, never in a frame of another page")));
        }
        if field == Field::OneTimeCode {
            return Err(refuse("sensitive_action", format!("{provider}'s codes are never filled from the vault: stop and report what the page asks")));
        }
        if account(provider).is_some_and(|other| other != fillable.id) {
            return Err(refuse(
                "sensitive_action",
                format!("Claktory's browser is signed in to {provider} with another account, and it holds one per provider: report it, and the person signs that one out first"),
            ));
        }
    }
    let names: Vec<String> = fillable.values.iter().map(|value| value.variable.clone()).collect();
    let chosen = choose(&fillable.name, &names, variable, field).map_err(|why| refuse("unsupported_target", why))?;
    let Some(value) = fillable.values.iter().find(|value| value.variable == chosen) else { return Err(gone()) };
    // The same document as checked, just before: never another page's.
    match page.frame().await {
        Ok(now) if now.id == frame.id && now.loader == frame.loader => {}
        _ => return Err(refuse("stale_document", "the page changed: read it again".to_owned())),
    }
    let argument = || vec![json!({"value": value.value.as_str()})];
    let set = call(SET, argument()).await;
    if !matches!(&set, Ok(set) if set["result"]["value"] == true) {
        return Err(gone());
    }
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let kept = call(KEPT, argument()).await.is_ok_and(|kept| kept["result"]["value"] == true);
    Ok(Filled { origin, field, count: value.value.chars().count(), kept })
}

impl Factory {
    /// browser_fill_secret: fills `target` of `document` (as the agent's
    /// last errand_read named them) with secret `secret`'s `variable` (or
    /// the one that fits the field), and logs it. What the agent is told,
    /// and the details of the card its first sign-in at an identity
    /// provider posts (null without one); or why not.
    pub(crate) async fn fill_secret(
        self: &Arc<Self>,
        agent: &str,
        document: &str,
        target: &str,
        secret: &str,
        variable: Option<&str>,
    ) -> Result<(String, Value), String> {
        let errand = self.driving(agent).map_err(said)?;
        let fillable = self.vault.fillable(agent, secret).map_err(|why| {
            let yours = self.vault.fillable_lines(agent);
            if yours.is_empty() {
                format!("{why} You have no secret to fill: your lead asks the person for one, bound to the site.")
            } else {
                format!("{why} You can fill: {}.", yours.join("; "))
            }
        })?;
        let entry = |action: String, origin: &str, outcome: String| log::Entry {
            errand: errand.id.clone(),
            agent: agent.to_owned(),
            origin: origin.to_owned(),
            action,
            outcome,
            ..Default::default()
        };
        let action = format!("Filled {} from the vault", fillable.name);
        let refuse = |origin: &str, (error, why): (String, String)| -> String {
            log::write(&self.store, &entry(action.clone(), origin, format!("{error}: {why}")));
            said((error, why))
        };
        // One step at a time: nothing else drives the page meanwhile.
        let mut live = self.errands.live.lock().await;
        let page = self.page(&mut live, &errand).await.map_err(said)?;
        match page.elsewhere(&errand.sites).await {
            Ok(None) => {}
            Ok(Some(origin)) => {
                let why = format!("this tab is on {origin}, not one of the errand's sites: go back to one with errand_navigate");
                return Err(refuse(&origin, ("off_site".to_owned(), why)));
            }
            Err(error) => return Err(said(("interrupted".to_owned(), crate::browser::schema::one_line(&error.to_string(), 200)))),
        }
        let account = |provider: &str| self.vault.browser_account(provider);
        let browser = self.errands.browser();
        let signing_out = |provider: &str| browser.as_ref().is_some_and(|browser| browser.signing_out(provider));
        let filled = fill_into(&page, document, target, &fillable, variable, account, signing_out, identity_provider_fills())
            .await
            .map_err(|(origin, refused)| refuse(&origin, refused))?;
        drop(live);
        let Filled { origin, field, count, kept } = filled;
        let path = page.frame().await.ok().and_then(|now| url::Url::parse(&now.url).ok()).map(|url| super::fence::path_of(&url));
        let mut logged = entry(action.clone(), &origin, if kept { String::new() } else { "the page didn't keep it".to_owned() });
        logged.target = format!("{} field", field.word());
        logged.path = path.unwrap_or_default();
        log::write(&self.store, &logged);
        let detail = format!("filled into a {} field on {origin}", field.word());
        if let Err(error) = self.vault.record_fill(&fillable.id, agent, &errand.id, &detail) {
            tracing::error!(%error, "a fill from the vault wasn't logged in its uses");
        }
        let mut card = Value::Null;
        if let Some(provider) = provider(&origin) {
            match self.vault.set_browser_account(provider, &fillable.id) {
                Ok(true) => {
                    let said = format!("Signing in to {provider} with {}: the browser's account there from now on", fillable.name);
                    log::write(&self.store, &entry(said, &origin, String::new()));
                    // The person is told, with Sign out (Fugo 41236).
                    card = super::code::signed_in(self, agent, &errand, provider, &fillable).unwrap_or_else(|why| {
                        tracing::error!(%why, "the card about the browser's first sign-in wasn't posted");
                        Value::Null
                    });
                }
                Ok(false) => {}
                Err(error) => tracing::error!(%error, "the browser's account at an identity provider wasn't kept"),
            }
        }
        let characters = if count == 1 { "character" } else { "characters" };
        let into = format!("Filled {} ({count} {characters}) into the {} field", fillable.name, field.word());
        let mut said = if kept { format!("{into}.") } else { format!("{into}, but the page didn't keep it: read the page to see why.") };
        if !card.is_null() {
            said.push_str(&format!(
                " This is the browser's first sign-in to {}, so Claktory put a request in your conversation telling the person, with Sign out: it isn't yours to answer, and the errand goes on.",
                provider(&origin).unwrap_or_default()
            ));
        }
        Ok((said, card))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::schema::Answer;
    use crate::vault::Value as Secret;
    use base64::Engine as _;

    /// A local Chrome on `pages` (address → HTML), served by Claktory itself
    /// (Fetch), so each is on its own https origin as Chrome sees it, and
    /// open at the first. None where there's no Chrome.
    async fn chrome_on(pages: &[(&str, &str)]) -> Option<(super::super::cdp::Local, Arc<Page>, std::path::PathBuf)> {
        let (local, page, profile) = super::super::reader::tests::page_with("").await?;
        let mut events = page.cdp().events();
        page.call("Fetch.enable", json!({"patterns": [{"urlPattern": "*", "requestStage": "Request"}]})).await.unwrap();
        let served: Vec<(String, String)> = pages.iter().map(|(url, html)| ((*url).to_owned(), (*html).to_owned())).collect();
        let serving = page.clone();
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event.method != "Fetch.requestPaused" {
                    continue;
                }
                let request = event.params["requestId"].as_str().unwrap_or_default().to_owned();
                let url = event.params["request"]["url"].as_str().unwrap_or_default();
                let body = served.iter().find(|(at, _)| at == url).map_or("", |(_, html)| html.as_str());
                let body = base64::engine::general_purpose::STANDARD.encode(body);
                let headers = json!([{"name": "Content-Type", "value": "text/html; charset=utf-8"}]);
                let status = if body.is_empty() { 404 } else { 200 };
                let fulfilled = json!({"requestId": request, "responseCode": status, "responseHeaders": headers, "body": body});
                let _ = serving.call("Fetch.fulfillRequest", fulfilled).await;
            }
        });
        page.call("Page.enable", json!({})).await.unwrap();
        go(&page, pages[0].0).await;
        Some((local, page, profile))
    }

    /// Goes to `url` and waits until it's shown.
    async fn go(page: &Page, url: &str) {
        page.call("Page.navigate", json!({"url": url})).await.unwrap();
        for _ in 0..100 {
            if page.frame().await.is_ok_and(|frame| frame.url == url) {
                let ready = page.call("Runtime.evaluate", json!({"expression": "document.readyState", "returnByValue": true})).await;
                if ready.is_ok_and(|ready| ready["result"]["value"] == "complete") {
                    return;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        panic!("{url} didn't load");
    }

    /// What the page's own script sees: `expression` in its main world.
    async fn page_says(page: &Page, expression: &str) -> Value {
        page.call("Runtime.evaluate", json!({"expression": expression, "returnByValue": true})).await.unwrap()["result"]["value"].clone()
    }

    /// The page read, its document, and the ref of the element named `name`.
    async fn read(page: &Page, name: &str) -> (String, String) {
        let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
        let element = read.elements.iter().find(|element| element.name.contains(name)).unwrap_or_else(|| panic!("{name} in {:?}", read.elements));
        (read.document.clone(), element.reference.clone())
    }

    /// Fugo 50018 (H2): a secret bound to https://shop.example is filled
    /// only in a frame at exactly that origin.
    #[test]
    fn a_secret_fills_only_at_exactly_its_sites_origin() {
        let sites = vec!["https://shop.example".to_owned()];
        let fills = |frame: &str| fills_on(&https_origin(frame), &sites);
        assert!(fills("https://shop.example"));
        assert!(fills("https://shop.example:443"));
        for frame in [
            "http://shop.example",
            "https://sub.shop.example",
            "https://www.shop.example",
            "https://shop.example:8443",
            "https://shop.example.evil.test",
            "https://evil-shop.example",
            "wss://shop.example",
            "null",
            "",
        ] {
            assert!(!fills(frame), "{frame}");
        }
    }

    fn shop_secret(sites: &[&str]) -> Fillable {
        let value = |variable: &str, value: &str| Secret { variable: variable.to_owned(), value: zeroize::Zeroizing::new(value.to_owned()), file: false, binary: false };
        Fillable {
            id: "s1".to_owned(),
            name: "Shop".to_owned(),
            sites: sites.iter().map(|site| (*site).to_owned()).collect(),
            values: vec![value("USERNAME", "matt@example.com"), value("PASSWORD", "correct horse battery")],
        }
    }

    const LOGIN: &str = r#"<!doctype html><title>Sign in</title>
        <label>Email <input type=email id=email autocomplete=username></label>
        <label>Password <input type=password id=pw oninput="document.title = 'typed ' + this.value.length"></label>
        <label>About you <input id=bio></label>
        <button>Sign in</button>"#;

    #[tokio::test]
    async fn a_secret_goes_into_its_sites_password_and_username_fields_only() {
        let Some((_local, page, profile)) = chrome_on(&[("https://shop.example/login", LOGIN)]).await else { return };
        let none = |_: &str| None;
        let (document, password) = read(&page, "Password").await;
        let filled = fill_into(&page, &document, &password, &shop_secret(&["https://shop.example"]), None, none, |_| false, true).await.expect("filled");
        assert_eq!((filled.origin.as_str(), filled.field, filled.count, filled.kept), ("https://shop.example", Field::Password, 21, true));
        assert_eq!(page_says(&page, "document.getElementById('pw').value").await, "correct horse battery");
        assert_eq!(page_says(&page, "document.title").await, "typed 21", "the page's own listener heard it");

        let (document, email) = read(&page, "Email").await;
        let filled = fill_into(&page, &document, &email, &shop_secret(&["https://shop.example"]), None, none, |_| false, true).await.expect("filled");
        assert_eq!(filled.field, Field::Username);
        assert_eq!(page_says(&page, "document.getElementById('email').value").await, "matt@example.com");
        // Never the password into a field that could show it.
        let refused = fill_into(&page, &document, &email, &shop_secret(&["https://shop.example"]), Some("PASSWORD"), none, |_| false, true).await.unwrap_err();
        assert_eq!(refused.1.0, "unsupported_target");
        let (document, bio) = read(&page, "About you").await;
        let refused = fill_into(&page, &document, &bio, &shop_secret(&["https://shop.example"]), None, none, |_| false, true).await.unwrap_err();
        assert_eq!(refused.1.0, "unsupported_target", "{refused:?}");
        assert_eq!(page_says(&page, "document.getElementById('bio').value").await, "");

        // Bound elsewhere: never filled here.
        let (document, password) = read(&page, "Password").await;
        let refused = fill_into(&page, &document, &password, &shop_secret(&["https://shop.example.net"]), None, none, |_| false, true).await.unwrap_err();
        assert_eq!((refused.0.as_str(), refused.1.0.as_str()), ("https://shop.example", "wrong_site"));
        // A page read before isn't the page now.
        go(&page, "https://shop.example/login").await;
        let refused = fill_into(&page, &document, &password, &shop_secret(&["https://shop.example"]), None, none, |_| false, true).await.unwrap_err();
        assert_eq!(refused.1.0, "stale_document");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn the_field_is_filled_by_node_whatever_the_page_does_with_focus() {
        // The password field hands focus to a field another site's frame
        // could read: the value still goes only into the field named.
        let moving = r#"<!doctype html><title>Sign in</title>
            <label>Note <input id=note></label>
            <label>Password <input type=password id=pw onfocus="document.getElementById('note').focus()"></label>"#;
        let Some((_local, page, profile)) = chrome_on(&[("https://shop.example/login", moving)]).await else { return };
        let (document, password) = read(&page, "Password").await;
        fill_into(&page, &document, &password, &shop_secret(&["https://shop.example"]), None, |_| None, |_| false, true).await.expect("filled");
        assert_eq!(page_says(&page, "document.getElementById('pw').value").await, "correct horse battery");
        assert_eq!(page_says(&page, "document.getElementById('note').value").await, "");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn an_identity_provider_takes_one_account_and_no_code_from_the_vault() {
        let google = r#"<!doctype html><title>Sign in</title>
            <label>Email or phone <input type=email id=email autocomplete=username></label>
            <label>Enter code <input id=code autocomplete=one-time-code></label>"#;
        let Some((_local, page, profile)) = chrome_on(&[("https://accounts.google.com/signin", google)]).await else { return };
        let fillable = shop_secret(&["https://accounts.google.com"]);
        let (document, code) = read(&page, "Enter code").await;
        let refused = fill_into(&page, &document, &code, &fillable, Some("PASSWORD"), |_| None, |_| false, true).await.unwrap_err();
        assert_eq!(refused.1.0, "sensitive_action", "{refused:?}");
        let (document, email) = read(&page, "Email").await;
        let refused = fill_into(&page, &document, &email, &fillable, None, |_| Some("s2".to_owned()), |_| false, true).await.unwrap_err();
        assert!(refused.1.1.contains("another account"), "{refused:?}");
        assert_eq!(page_says(&page, "document.getElementById('email').value").await, "");
        let filled = fill_into(&page, &document, &email, &fillable, None, |_| Some("s1".to_owned()), |_| false, true).await.expect("its own account");
        assert_eq!(filled.field, Field::Username);
        // Until its ship gate opens, never on an identity provider.
        let (document, email) = read(&page, "Email").await;
        let refused = fill_into(&page, &document, &email, &fillable, None, |_| None, |_| false, false).await.unwrap_err();
        assert!(refused.1.1.contains("doesn't sign in to Google"), "{refused:?}");
        // While a sign-out of Google waits, nothing is filled there.
        let (document, email) = read(&page, "Email").await;
        let refused = fill_into(&page, &document, &email, &fillable, None, |_| None, |provider| provider == "Google", true).await.unwrap_err();
        assert!(refused.1.1.starts_with("signing out of Google when this errand ends"), "{refused:?}");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[test]
    fn a_providers_cookies_are_on_its_domains_and_googles_in_every_country() {
        for (host, provider) in [
            ("accounts.google.com", Some("Google")),
            (".google.com", Some("Google")),
            ("www.google.co.uk", Some("Google")),
            ("google.de", Some("Google")),
            (".youtube.com", Some("Google")),
            ("idmsa.apple.com", Some("Apple")),
            ("www.icloud.com", Some("Apple")),
            ("login.microsoftonline.com", Some("Microsoft")),
            (".live.com", Some("Microsoft")),
            ("outlook.office.com", Some("Microsoft")),
            ("shop.example", None),
            ("notgoogle.com", None),
            ("google.com.evil.example", None),
            ("googleusercontent.com", None),
            ("mygoogle.co.uk", None),
            ("ilive.com", None),
        ] {
            assert_eq!(owner(host), provider, "{host}");
        }
        assert_eq!(provider("https://idmsa.apple.com"), Some("Apple"));
        assert_eq!(provider("https://www.google.com"), None);
        let signing_out = |provider: &str| provider == "Google";
        assert!(while_signing_out("https://mail.google.com", signing_out).is_some());
        assert!(while_signing_out("https://www.youtube.com", signing_out).is_some());
        assert!(while_signing_out("https://appleid.apple.com", signing_out).is_none());
        assert!(while_signing_out("https://shop.example", signing_out).is_none());
    }

    #[tokio::test]
    async fn an_identity_providers_sign_in_framed_in_another_page_is_never_filled() {
        let shop = r#"<!doctype html><title>Shop</title><iframe src="https://accounts.google.com/frame"></iframe>"#;
        let framed = r#"<!doctype html><label>Email <input type=email id=email autocomplete=username></label>"#;
        let pages = [("https://shop.example/", shop), ("https://accounts.google.com/frame", framed)];
        let Some((_local, page, profile)) = chrome_on(&pages).await else { return };
        // The framed sign-in runs in a process of its own (site isolation):
        // its session, and its field as a reader there would hand it over.
        let mut framed_session = None;
        for _ in 0..100 {
            let targets = page.cdp().call("Target.getTargets", json!({}), None).await.unwrap();
            let found = targets["targetInfos"].as_array().unwrap().iter().find(|target| target["type"] == "iframe").cloned();
            if let Some(target) = found {
                let attached = page.cdp().call("Target.attachToTarget", json!({"targetId": target["targetId"], "flatten": true}), None).await.unwrap();
                framed_session = attached["sessionId"].as_str().map(str::to_owned);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let framed = Arc::new(Page::new(page.cdp().clone(), framed_session.expect("the framed sign-in's own process")));
        let mut frame = framed.frame().await.unwrap();
        for _ in 0..100 {
            if frame.origin == "https://accounts.google.com" {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            frame = framed.frame().await.unwrap();
        }
        assert_eq!(frame.origin, "https://accounts.google.com");
        assert!(!frame.parent.is_empty(), "{:?}", frame.id);
        let world = framed.call("Page.createIsolatedWorld", json!({"frameId": frame.id, "worldName": "test"})).await.unwrap();
        let context = world["executionContextId"].as_i64().unwrap();
        let field = json!({"expression": "document.getElementById('email')", "contextId": context});
        let object = framed.call("Runtime.evaluate", field).await.unwrap()["result"]["objectId"].as_str().unwrap().to_owned();
        let page = framed;
        let fillable = shop_secret(&["https://accounts.google.com"]);
        let refused = fill_node(&page, &object, &frame, &fillable, None, |_| None, |_| false, true).await.unwrap_err();
        assert!(refused.1.1.contains("never in a frame"), "{refused:?}");
        let typed = json!({"expression": "document.getElementById('email').value", "contextId": context, "returnByValue": true});
        assert_eq!(page.call("Runtime.evaluate", typed).await.unwrap()["result"]["value"], "");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[test]
    fn a_username_field_takes_only_a_username() {
        let both = vec!["USERNAME".to_owned(), "PASSWORD".to_owned()];
        assert_eq!(choose("Shop", &both, None, Field::Username).unwrap(), "USERNAME");
        assert_eq!(choose("Shop", &both, None, Field::Password).unwrap(), "PASSWORD");
        assert!(choose("Shop", &both, Some("PASSWORD"), Field::Username).is_err(), "never a password into a public field");
        assert_eq!(choose("Shop", &both, Some("username"), Field::Password).unwrap(), "USERNAME");
        let one = vec!["TOKEN".to_owned()];
        assert_eq!(choose("Key", &one, None, Field::Password).unwrap(), "TOKEN");
        assert!(choose("Key", &one, None, Field::Username).is_err());
        let many = vec!["SHOP_EMAIL".to_owned(), "PIN".to_owned(), "PASSWORD".to_owned()];
        assert_eq!(choose("Shop", &many, None, Field::Username).unwrap(), "SHOP_EMAIL");
        assert!(choose("Shop", &many, None, Field::Password).is_err(), "two could go: the agent names one");
        assert!(choose("Shop", &many, Some("NOPE"), Field::Password).is_err());
    }

    #[test]
    fn users_are_known_by_their_variables_names() {
        for user in ["USERNAME", "user", "LOGIN", "EMAIL", "SHOP_EMAIL", "GOOGLE_USERNAME", "E_MAIL"] {
            assert!(names_a_user(user), "{user}");
        }
        for other in ["PASSWORD", "USERNAME_PASSWORD", "EMAILS", "TOKEN", "SUPERUSERPASS"] {
            assert!(!names_a_user(other), "{other}");
        }
    }

    #[test]
    fn on_an_identity_provider_the_agent_only_moves_the_sign_in_on() {
        let google = "https://accounts.google.com";
        let button = |name: &str| json!({"role": "button", "name": name});
        assert!(on_identity_provider(google, true, "click", &button("Next")).is_none());
        assert!(on_identity_provider(google, true, "click", &button(" Weiter ")).is_none());
        assert!(on_identity_provider(google, true, "click", &button("Sign in")).is_none());
        for refused in ["Try another way", "Forgot password?", "Allow", "Create account", "Use a passkey", "Next step: recovery"] {
            assert!(on_identity_provider(google, true, "click", &button(refused)).is_some(), "{refused}");
        }
        // A name that says Next on a control that shows something else.
        assert!(on_identity_provider(google, true, "click", &json!({"name": "Next", "shows": "Delete account"})).is_some());
        assert!(on_identity_provider(google, true, "fill", &json!({})).is_some(), "only Claktory fills there");
        assert!(on_identity_provider(google, false, "click", &button("Next")).is_some(), "never framed in another page");
        assert!(on_identity_provider("https://shop.example", true, "click", &button("Delete everything")).is_none(), "elsewhere, the reader's rules");
        assert_eq!(provider("https://login.live.com"), Some("Microsoft"));
        assert_eq!(provider("https://accounts.google.com.evil.example"), None);
        assert_eq!(provider("http://accounts.google.com"), None);
    }
}
