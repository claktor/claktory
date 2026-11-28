//! Website logins typed into an agent's browser (`browser_sign_in`): the
//! login an agent holds for exactly the page's origin, as Chrome reports it,
//! typed into the page's own fields with the browser's keys at a person's
//! pace, never handed to the agent; and the identity providers (Google,
//! Apple, Microsoft), whose sign-in pages the agent may only move on.

use std::sync::Arc;

use serde_json::{Value, json};

use super::reader::Page;
use super::{log, said};
use crate::factory::Factory;
use crate::vault::sites::Fillable;

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
            format!("on {provider}'s sign-in only Claktory types a field, from the vault: use {SIGN_IN_TOOL}"),
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

/// A sign-in field's kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Password,
    Username,
}

impl Field {
    fn word(self) -> &'static str {
        match self {
            Field::Password => "password",
            Field::Username => "username",
        }
    }
}

/// Whether a variable holds a username or an email address, by its name:
/// USERNAME, USER, LOGIN, EMAIL, or one ending in _USERNAME and so on.
fn names_a_user(variable: &str) -> bool {
    let variable = variable.to_ascii_uppercase();
    ["USERNAME", "USER", "LOGIN", "EMAIL", "USER_NAME", "E_MAIL"]
        .iter()
        .any(|word| variable == *word || variable.ends_with(&format!("_{word}")))
}

/// Chrome's origin of a frame as the vault's sites spell them: an https
/// origin only, else empty.
fn https_origin(origin: &str) -> String {
    url::Url::parse(origin).ok().filter(|url| url.scheme() == "https").map(|url| url.origin().ascii_serialization()).unwrap_or_default()
}

/// The tool that signs an agent's browser in to a site (the Browser
/// extension's, `sign_in`).
pub(crate) const SIGN_IN_TOOL: &str = "browser_sign_in";

/// What the agent is told when the keyboard leaves the field mid-sign-in:
/// never which character, nor how many went in.
const MOVED: &str = "the page moved focus during sign-in; stopped";

/// What a field the agent named for a sign-in is, asked of the node in the
/// reader's world: "password" (type password, or autocomplete
/// current-password or new-password), "username" (a text, email or tel
/// field, or autocomplete username or email), else "other" or "gone".
const LOGIN_FIELD: &str = r#"function () {
  if (!(this instanceof HTMLInputElement)) return 'other';
  if (!this.isConnected || this.ownerDocument !== document || this.disabled || this.readOnly) return 'gone';
  const type = (this.type || 'text').toLowerCase();
  const tokens = (this.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
  if (type === 'password' || tokens.includes('current-password') || tokens.includes('new-password')) return 'password';
  if (tokens.includes('one-time-code')) return 'other';
  if (['text', 'email', 'tel'].includes(type) || tokens.includes('username') || tokens.includes('email')) return 'username';
  return 'other';
}"#;

/// Before and after every key: the field is still the page's
/// own (its top document's), connected, the one with the keyboard (the top
/// document's focused element, followed down through shadow roots, never
/// a frame: a frame that took the focus is the top document's focused
/// element), in a document still at `origin`. Not whether the window has
/// the system's focus, which a browser no one looks at never has. How long its value
/// is then (as the page counts it), else -1. Asked in the reader's world,
/// which the page's scripts can't reach; the length goes nowhere else.
const HELD: &str = r#"function (origin) {
  if (!this.isConnected || this.ownerDocument !== document || window !== window.top) return -1;
  if (location.origin !== origin) return -1;
  let active = document.activeElement;
  while (active && active.shadowRoot && active.shadowRoot.activeElement) active = active.shadowRoot.activeElement;
  if (active !== this) return -1;
  return this.value.length;
}"#;

/// Where the field is in the viewport, and the viewport's size.
const PLACE: &str = r#"function () {
  const r = this.getBoundingClientRect();
  return [r.left, r.top, r.width, r.height, innerWidth, innerHeight];
}"#;

/// Whether the point (x, y) of the viewport is on the field, through
/// shadow roots: what a press there would press.
const HITS: &str = r#"function (x, y) {
  let at = document.elementFromPoint(x, y);
  while (at && at.shadowRoot) {
    const inner = at.shadowRoot.elementFromPoint(x, y);
    if (!inner || inner === at) break;
    at = inner;
  }
  return !!at && (at === this || this.contains(at));
}"#;

/// Whether the field's form says it sends its values in the next page's
/// address (method="get", said on the form or its button). A form that says
/// nothing is sent by the page's own script on most sign-in pages, so it
/// isn't refused; the address the agent is told never has a query anyway.
const SENDS_IN_ADDRESS: &str = r#"function () {
  const form = this.form;
  if (!form) return false;
  const said = (element) => (element && element.getAttribute(element === form ? 'method' : 'formmethod') || '').trim().toLowerCase() === 'get';
  return said(form) || Array.from(form.querySelectorAll('button[formmethod], input[formmethod]')).some(said);
}"#;

/// Empties the field: after a sign-in stopped partway, nothing typed stays.
const CLEAR: &str = r#"function () {
  if (!this.isConnected) return false;
  Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set.call(this, '');
  this.dispatchEvent(new Event('input', {bubbles: true}));
  return true;
}"#;

/// Shift, as the Input domain's modifiers say it.
const SHIFT: u32 = 8;

/// Why a sign-in stopped, as an errand's tools say it.
type Refused = (String, String);

fn refused(error: &str, why: impl Into<String>) -> Refused {
    (error.to_owned(), why.into())
}

/// The browser didn't answer.
fn lost(error: super::cdp::Error) -> Refused {
    refused("interrupted", crate::browser::schema::one_line(&error.to_string(), 200))
}

/// One field of a sign-in, as the agent named it and the reader handed it
/// over: its node in the reader's world.
struct LoginField {
    object: String,
    field: Field,
}

/// How characters are typed: by the keyboard's keys (text for one no key
/// types), or all as text.
#[derive(Clone, Copy)]
enum Keys {
    Keys,
    Text,
}

/// A page being typed into: its top frame as it was when the sign-in began
/// (same frame, same document, same origin to the end).
struct Typing<'a> {
    page: &'a Page,
    top: &'a super::reader::Frame,
    origin: &'a str,
}

impl Typing<'_> {
    /// `declaration` called on `object`, its value.
    async fn on(&self, object: &str, declaration: &str, arguments: Vec<Value>) -> Result<Value, Refused> {
        let called = self
            .page
            .call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": declaration, "arguments": arguments, "returnByValue": true}))
            .await
            .map_err(lost)?;
        Ok(called["result"]["value"].clone())
    }

    /// The field's length if it still has the keyboard on the same
    /// document at the same origin (`HELD`); None if anything moved.
    async fn held(&self, object: &str) -> Option<i64> {
        match self.page.frame().await {
            Ok(now) if now.id == self.top.id && now.loader == self.top.loader && https_origin(&now.origin) == self.origin => {}
            _ => return None,
        }
        let length = self.on(object, HELD, vec![json!({"value": self.origin})]).await.ok()?.as_i64()?;
        (length >= 0).then_some(length)
    }

    /// Stops a sign-in partway: what went into the field is taken out.
    async fn abort(&self, object: &str) -> Refused {
        let _ = self.on(object, CLEAR, Vec::new()).await;
        refused("interrupted", MOVED)
    }

    /// The mouse goes to the field over a person's time and presses it, as
    /// a person clicks into a field, unless it has the keyboard already.
    async fn click_into(&self, hand: &mut super::reader::Hand<'_>, object: &str) -> Result<(), Refused> {
        if self.held(object).await.is_some() {
            return Ok(());
        }
        self.press_on(hand, object, "field").await?;
        if self.held(object).await.is_none() {
            return Err(refused("unsupported_target", "the field didn't take the keyboard when clicked: read the page again"));
        }
        Ok(())
    }

    /// The mouse goes to `object` (a `what`, as the agent is told) over a
    /// person's time and presses it.
    async fn press_on(&self, hand: &mut super::reader::Hand<'_>, object: &str, what: &str) -> Result<(), Refused> {
        let place = self.on(object, PLACE, Vec::new()).await?;
        let number = |at: usize| place[at].as_f64().filter(|n| n.is_finite() && (-1e7..1e7).contains(n));
        let (Some(left), Some(top), Some(width), Some(height), Some(view_width), Some(view_height)) =
            (number(0), number(1), number(2), number(3), number(4), number(5))
        else {
            return Err(refused("stale_target", format!("the {what} went: read the page again")));
        };
        if width < 1.0 || height < 1.0 || view_width < 1.0 || view_height < 1.0 {
            return Err(refused("stale_target", format!("the {what} isn't shown: read the page again")));
        }
        let view = (view_width, view_height);
        let mut at = super::pace::aim(hand.rng, (left, top, width, height), view);
        if self.on(object, HITS, vec![json!({"value": at.0}), json!({"value": at.1})]).await? != true {
            at = super::pace::pixel((left + width / 2.0, top + height / 2.0), view);
        }
        self.page.move_to(hand, at, view, width.min(height)).await?;
        let pause = super::pace::hover(hand.rng);
        hand.wait_until(std::time::Instant::now() + pause).await?;
        // Checked right before the press, nothing awaited between.
        if self.on(object, HITS, vec![json!({"value": at.0}), json!({"value": at.1})]).await? != true {
            return Err(refused("stale_target", format!("something covers the {what}: read the page again")));
        }
        self.mouse("mousePressed", at).await?;
        tokio::time::sleep(super::pace::hold(hand.rng)).await;
        self.mouse("mouseReleased", at).await?;
        let pause = super::pace::hover(hand.rng);
        hand.wait_until(std::time::Instant::now() + pause).await
    }

    /// Whether the field holds exactly `value`: yes or no, asked in the
    /// reader's world, never the value itself.
    async fn kept(&self, object: &str, value: &str) -> bool {
        self.on(object, KEPT, vec![json!({"value": value})]).await.is_ok_and(|kept| kept == true)
    }

    /// The object `declaration` gives when called on `object`, if any.
    async fn object(&self, object: &str, declaration: &str) -> Option<String> {
        let called = self.page.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": declaration})).await.ok()?;
        called["result"]["objectId"].as_str().filter(|id| id.len() <= 256).map(str::to_owned)
    }

    /// Makes sure the field holds exactly `value` as typed; if the keys
    /// typed something else, it's typed once more a character at a time as
    /// text (as a keyboard the browser doesn't know types), with the same
    /// checks at every character, and checked again. Still not: the field is emptied and nothing is sent.
    async fn keep(&self, hand: &mut super::reader::Hand<'_>, object: &str, value: &str) -> Result<(), Refused> {
        if self.kept(object, value).await {
            return Ok(());
        }
        self.type_as(hand, object, value, Keys::Text).await?;
        if self.kept(object, value).await {
            return Ok(());
        }
        let _ = self.on(object, CLEAR, Vec::new()).await;
        Err(refused("interrupted", "the page didn't keep exactly what was typed into the field, so nothing was sent"))
    }

    /// Ctrl+A then Backspace, as a person clears a field that holds the
    /// keyboard; stops if it doesn't empty it.
    async fn select_and_delete(&self, hand: &mut super::reader::Hand<'_>, object: &str) -> Result<(), Refused> {
        let control = if cfg!(target_os = "macos") { 4 } else { 2 };
        let key = |kind: &str, modifiers: u32| json!({"type": kind, "key": "a", "code": "KeyA", "windowsVirtualKeyCode": 65, "modifiers": modifiers, "commands": ["selectAll"]});
        self.page.call("Input.dispatchKeyEvent", key("rawKeyDown", control)).await.map_err(lost)?;
        tokio::time::sleep(super::pace::key_hold(hand.rng)).await;
        self.page.call("Input.dispatchKeyEvent", key("keyUp", control)).await.map_err(lost)?;
        let pause = gap(hand.rng);
        hand.wait_until(std::time::Instant::now() + pause).await?;
        self.press("Backspace", "Backspace", 8, "", hand).await?;
        match self.held(object).await {
            Some(0) => Ok(()),
            _ => Err(self.abort(object).await),
        }
    }

    async fn mouse(&self, kind: &str, at: super::pace::Point) -> Result<(), Refused> {
        self.page.mouse(kind, at).await.map(drop).map_err(lost)
    }

    /// Types `value` into the field with the browser's own keys, at a
    /// person's pace (134 to 190 ms from one key going down to
    /// the next, each held 70 to 102 ms, drawn from the errand's random
    /// source, never from the characters). Before every key the field must
    /// hold the keyboard with what was typed so far, and after it one more
    /// character, else it stops at once and empties the field:
    /// at most one character can land elsewhere. What was in the field is
    /// selected and typed over.
    async fn type_value(&self, hand: &mut super::reader::Hand<'_>, object: &str, value: &str) -> Result<(), Refused> {
        self.type_as(hand, object, value, Keys::Keys).await
    }

    /// `type_value`, each character by its key or as text.
    async fn type_as(&self, hand: &mut super::reader::Hand<'_>, object: &str, value: &str, keys: Keys) -> Result<(), Refused> {
        use unicode_segmentation::UnicodeSegmentation;
        self.click_into(hand, object).await?;
        let Some(mut length) = self.held(object).await else { return Err(self.abort(object).await) };
        if length > 0 {
            self.select_and_delete(hand, object).await?;
            length = 0;
        }
        let mut next = std::time::Instant::now() + gap(hand.rng);
        for grapheme in value.graphemes(true) {
            if let Err(stopped) = hand.wait_until(next).await {
                let _ = self.abort(object).await;
                return Err(stopped);
            }
            if self.held(object).await != Some(length) {
                return Err(self.abort(object).await);
            }
            let down = std::time::Instant::now();
            let held = std::time::Duration::from_millis(u64::from(hand.rng.whole(70, 102)));
            let typed = match keys {
                Keys::Keys => self.type_one(grapheme, held).await,
                Keys::Text => self.page.call("Input.insertText", json!({"text": grapheme})).await.map(drop),
            };
            if typed.is_err() {
                return Err(self.abort(object).await);
            }
            length += grapheme.encode_utf16().count() as i64;
            if self.held(object).await != Some(length) {
                return Err(self.abort(object).await);
            }
            next = down + gap(hand.rng);
        }
        Ok(())
    }

    /// One character's key, held `held` (Shift first for a capital or a
    /// symbol), or the character as text when no key types it. No key
    /// event carries `nativeVirtualKeyCode`: Chrome reads it as the
    /// platform's own key number, so a Windows code on a Mac presses other
    /// keys (Shift and C open Chrome's settings).
    async fn type_one(&self, grapheme: &str, held: std::time::Duration) -> Result<(), super::cdp::Error> {
        let Some(key) = super::keys::key(grapheme) else {
            return self.page.call("Input.insertText", json!({"text": grapheme})).await.map(drop);
        };
        let modifiers = if key.shift { SHIFT } else { 0 };
        if key.shift {
            let shift = json!({"type": "rawKeyDown", "key": "Shift", "code": "ShiftLeft", "windowsVirtualKeyCode": 16, "modifiers": SHIFT});
            self.page.call("Input.dispatchKeyEvent", shift).await?;
            tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        }
        let down = json!({
            "type": "keyDown", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.vk,
            "text": grapheme, "unmodifiedText": key.unshifted, "modifiers": modifiers,
        });
        self.page.call("Input.dispatchKeyEvent", down).await?;
        tokio::time::sleep(held).await;
        let up = json!({"type": "keyUp", "key": key.key, "code": key.code, "windowsVirtualKeyCode": key.vk, "modifiers": modifiers});
        self.page.call("Input.dispatchKeyEvent", up).await?;
        if key.shift {
            let shift = json!({"type": "keyUp", "key": "Shift", "code": "ShiftLeft", "windowsVirtualKeyCode": 16, "modifiers": 0});
            self.page.call("Input.dispatchKeyEvent", shift).await?;
        }
        Ok(())
    }

    /// A key that isn't a character's (Backspace, Enter), held as a
    /// person holds a key; `text` what it types ("\r" for Enter).
    async fn press(&self, key: &str, code: &str, vk: u32, text: &str, hand: &mut super::reader::Hand<'_>) -> Result<(), Refused> {
        let mut down = json!({"type": if text.is_empty() { "rawKeyDown" } else { "keyDown" }, "key": key, "code": code, "windowsVirtualKeyCode": vk});
        if !text.is_empty() {
            down["text"] = json!(text);
            down["unmodifiedText"] = json!(text);
        }
        self.page.call("Input.dispatchKeyEvent", down).await.map_err(lost)?;
        tokio::time::sleep(std::time::Duration::from_millis(u64::from(hand.rng.whole(70, 102)))).await;
        let up = json!({"type": "keyUp", "key": key, "code": code, "windowsVirtualKeyCode": vk});
        self.page.call("Input.dispatchKeyEvent", up).await.map(drop).map_err(lost)
    }
}

/// From one key going down to the next.
fn gap(rng: &mut super::pace::Rng) -> std::time::Duration {
    std::time::Duration::from_millis(u64::from(rng.whole(134, 190)))
}

/// What the page shows as a sign-in settles, asked in the reader's world:
/// its address, whether it's loaded, how many password fields a person
/// would see on it, and its words.
const AFTER: &str = r#"(() => {
  const shown = (field) => {
    if (!field.isConnected) return false;
    const box = field.getBoundingClientRect();
    if (box.width < 1 || box.height < 1 || box.right + scrollX <= 0 || box.bottom + scrollY <= 0) return false;
    const style = getComputedStyle(field);
    return style.display !== 'none' && style.visibility !== 'hidden' && Number(style.opacity) > 0;
  };
  const passwords = Array.from(document.querySelectorAll('input[type=password]')).filter(shown).length;
  const text = document.body ? document.body.innerText || '' : '';
  return {url: location.href, ready: document.readyState === 'complete', passwords, text: text.slice(0, 20000)};
})()"#;

/// The page as a sign-in settles (`AFTER`), and whether the reader sees a
/// field for a code on it.
#[derive(Clone, Debug, Default, PartialEq)]
struct After {
    url: String,
    ready: bool,
    passwords: u64,
    text: String,
    code: bool,
}

impl After {
    async fn of(page: &Page) -> Option<After> {
        let said = page.in_world(AFTER).await?;
        let code = match page.read(4096).await {
            Ok(crate::browser::schema::Answer::Page(read)) => asks_for_a_code(&read),
            _ => false,
        };
        Some(After {
            url: said["url"].as_str()?.to_owned(),
            ready: said["ready"] == true,
            passwords: said["passwords"].as_u64().unwrap_or_default(),
            text: said["text"].as_str().unwrap_or_default().to_lowercase(),
            code,
        })
    }
}

/// Whether the reader marked a field for a one-time code on the page.
fn asks_for_a_code(read: &crate::browser::schema::Page) -> bool {
    read.elements.iter().any(|element| {
        let why = element.sensitive.clone().unwrap_or_default().to_lowercase();
        why.contains("one-time-code") || why.contains("one digit of a code") || why.contains("verification code")
    })
}

/// How a sign-in went, as the page shows it.
#[derive(Clone, Debug, PartialEq)]
enum Settled {
    SignedIn,
    CodeNext,
    /// The form again, with words of a refusal it didn't have before.
    Refused,
    /// The sign-in form, still there.
    Form,
    /// Another page that isn't the site itself yet (a check, a challenge,
    /// another step): its path.
    Asks(String),
}

/// Words a site refuses a login with.
const REFUSALS: &[&str] = &[
    "incorrect",
    "wrong password",
    "wrong username",
    "wrong email",
    "wrong login",
    "wrong credentials",
    "password is wrong",
    "password was wrong",
    "unrecognized",
    "unrecognised",
    "not valid",
    "login failed",
    "sign-in failed",
    "sign in failed",
    "couldn't sign you in",
    "could not sign you in",
    "unable to sign in",
    "unable to log in",
    "invalid",
    "doesn't match",
    "does not match",
    "do not match",
    "don't match",
    "didn't match",
    "not recognized",
    "not recognised",
    "try again",
    "couldn't find",
    "could not find",
    "no account",
];

/// Words of an address's path that say it's still part of signing in: a
/// sign-in page, a check or another step.
const STILL_SIGNING_IN: &[&str] =
    &["login", "log-in", "logon", "signin", "sign-in", "sign_in", "auth", "checkpoint", "challenge", "verif", "two-factor", "two_factor", "2fa", "mfa", "captcha"];

/// An address without its fragment.
fn without_fragment(url: &str) -> &str {
    url.split('#').next().unwrap_or(url)
}

/// What `now` says of a sign-in sent from the page `before` showed: None
/// while it may still be on its way.
fn after_sign_in(before: &After, now: &After) -> Option<Settled> {
    if now.code {
        return Some(Settled::CodeNext);
    }
    if now.passwords > 0 {
        // New words, or the same said once more (a page that explains its
        // own errors beforehand, say).
        let (was, is) = (before.text.to_lowercase(), now.text.to_lowercase());
        let refused = REFUSALS.iter().any(|words| is.matches(words).count() > was.matches(words).count());
        return Some(if refused { Settled::Refused } else { Settled::Form });
    }
    if !now.ready {
        return None;
    }
    let path = url::Url::parse(&now.url).map(|url| url.path().to_owned()).unwrap_or_default();
    if without_fragment(&now.url) == without_fragment(&before.url) {
        return Some(Settled::Asks(path));
    }
    let lower = path.to_lowercase();
    if STILL_SIGNING_IN.iter().any(|words| lower.contains(words)) {
        return Some(Settled::Asks(path));
    }
    Some(Settled::SignedIn)
}

/// How long the page may stay as it is after a sign-in before it's taken
/// as its answer: a site checking a login takes a few seconds.
const QUIET: std::time::Duration = std::time::Duration::from_secs(10);

/// The longest a sign-in is watched after it's sent.
const LONGEST: std::time::Duration = std::time::Duration::from_secs(30);

/// How long after Enter a form that did nothing with it is sent with its
/// button instead.
const ENTER_IGNORED: std::time::Duration = std::time::Duration::from_secs(3);

/// The button that sends the password field's form, if one shows: its
/// form's submit button, else a button named as a sign-in's is.
const SUBMIT: &str = r#"function () {
  const shown = (button) => { const box = button.getBoundingClientRect(); return box.width >= 1 && box.height >= 1; };
  const scope = this.form || this.closest('form, [role=dialog], main') || document;
  const buttons = Array.from(scope.querySelectorAll('button, input[type=submit], [role=button]')).filter(shown);
  const named = (button) => (button.innerText || button.value || button.getAttribute('aria-label') || '').trim();
  return buttons.find((button) => (button.type || '').toLowerCase() === 'submit' && this.form && button.form === this.form)
    || buttons.find((button) => /^(sign in|log in|login|sign on|continue|next|submit)$/i.test(named(button)))
    || null;
}"#;

/// Why a field in a frame isn't signed in to.
const FRAMED: &str = "Claktory signs in only into the page's own fields, never one in a frame: open the sign-in page itself with browser_open, or say in your reply that you couldn't sign in";

/// browser_sign_in:
/// signs `agent`'s browser in on `page`, whose username and password
/// fields are `username` and `password` (refs of `document` as the agent's
/// last read gave them; at least one, a username alone for the first page
/// of a two-page sign-in). The caller holds the lane and the input lease.
///
/// - **Where:** the page's top frame, at an https origin with a name, as
///   Chrome says it, never on Chrome's warning about an unsafe site, a
///   password manager or a field in a frame.
/// - **With what:** the login the agent holds for exactly that origin,
///   typed with real keys (`Typing`), then sent with Enter, and what the
///   page does next read for the card. Never its values to the agent, a
///   log or a tool result.
/// - **Or:** it holds none, and the login card asks the person (its
///   details make the card), once; while a card waits, nothing is typed.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn sign_in(
    factory: &Arc<Factory>,
    agent: &str,
    page: &Arc<Page>,
    hand: &mut super::reader::Hand<'_>,
    document: &str,
    username: Option<&str>,
    password: Option<&str>,
    reason: &str,
) -> Result<(String, Value), String> {
    let username = username.map(str::trim).filter(|target| !target.is_empty());
    let password = password.map(str::trim).filter(|target| !target.is_empty());
    if username.is_none() && password.is_none() {
        return Err(said(refused("invalid_arguments", "name the password field, the username field or both, as browser_read gave them")));
    }
    let top = page.frame().await.map_err(|error| said(lost(error)))?;
    if top.url.starts_with("chrome-error:") {
        return Err(said(refused("unsafe_site", "Chrome warned this site is unsafe, so Claktory won't sign in here")));
    }
    if url::Url::parse(&top.url).is_ok_and(|url| super::fence::password_manager(&url)) {
        return Err(said(refused("sensitive_action", super::fence::PASSWORD_MANAGERS)));
    }
    let origin = https_origin(&top.origin);
    if origin.is_empty() || crate::browser::origin::canonical(&origin).is_err() {
        return Err(said(refused("unsupported_target", "Claktory signs in only on https sites with a name, not this page")));
    }
    // Each field the page's own (the reader resolves only the top
    // document's), and of its kind: never a password into another field.
    let mut fields = Vec::new();
    for (target, wanted) in [(username, Field::Username), (password, Field::Password)] {
        let Some(target) = target else { continue };
        if !page.in_a_frame(target) {
            let _ = page.bring_clear(hand, target).await;
        }
        let (object, frame) = match page.element(document, target).await {
            Ok(found) => found,
            Err(_) if page.in_a_frame(target) => return Err(said(refused("unsupported_target", FRAMED))),
            Err(why) => return Err(said(why)),
        };
        if frame.id != top.id || frame.loader != top.loader {
            return Err(said(refused("stale_document", "the page changed: read it again")));
        }
        let typing = Typing { page, top: &top, origin: &origin };
        let kind = typing.on(&object, LOGIN_FIELD, Vec::new()).await.map_err(said)?;
        let field = match kind.as_str() {
            Some("password") => Field::Password,
            Some("username") => Field::Username,
            Some("gone") => return Err(said(refused("stale_target", "the field went: read the page again"))),
            _ => return Err(said(refused("unsupported_target", format!("{target} isn't a {} field", wanted.word())))),
        };
        if field != wanted {
            return Err(said(refused("unsupported_target", format!("{target} isn't a {} field", wanted.word()))));
        }
        if field == Field::Password && typing.on(&object, SENDS_IN_ADDRESS, Vec::new()).await.map_err(said)? == true {
            return Err(said(refused("unsupported_target", "this form would put the password in the page's address, so Claktory won't sign in here")));
        }
        fields.push(LoginField { object, field });
    }
    let person = factory.lock().config.person().to_owned();
    // While the person is asked, nothing is typed: the login they give is
    // what's tried next.
    if let Some(waiting) = factory.vault.login_request(agent, &origin).map_err(|error| error.to_string())?
        && waiting.state() == factory_protocol::v1::SecretRequestState::Pending
    {
        return Ok((
            format!(
                "Your sign-in to {} waits for {person} on request {}. You're told when it's in: then call {SIGN_IN_TOOL} again.",
                waiting.login_site, waiting.id
            ),
            Value::Null,
        ));
    }
    let Some(login) = factory.vault.login_for(agent, &origin)? else {
        let (request, _) = factory
            .vault
            .ask_login(agent, &origin, reason, None, factory_protocol::v1::SecretLoginOutcome::Unspecified)
            .map_err(|error| error.to_string())?;
        factory.vault_changed();
        let said = format!(
            "Asked {person} to sign you in to {}: they type your login on a request in your conversation, or give you one of theirs. You're told when it's in: then call {SIGN_IN_TOOL} again on this page. You never see it: Claktory types it.",
            request.login_site
        );
        return Ok((said, crate::vault::asks::details(&request)));
    };
    type_login(factory, agent, page, hand, &top, &origin, &fields, &login, &person).await
}

/// Signs in with `login` on `fields`, logs it, and tells the card how it
/// went: what the agent is told, and the details of a card to show (when
/// the site refused a login no card asked for).
#[allow(clippy::too_many_arguments)]
async fn type_login(
    factory: &Arc<Factory>,
    agent: &str,
    page: &Arc<Page>,
    hand: &mut super::reader::Hand<'_>,
    top: &super::reader::Frame,
    origin: &str,
    fields: &[LoginField],
    login: &Fillable,
    person: &str,
) -> Result<(String, Value), String> {
    use crate::vault::logins::{PASSWORD, USERNAME};
    use factory_protocol::v1::{SecretLoginOutcome, SecretRequestState};
    let site = crate::vault::logins::site_of(origin);
    let value_of = |field: Field| {
        login.values.iter().find(|value| match field {
            Field::Password => value.variable == PASSWORD,
            _ => value.variable == USERNAME || names_a_user(&value.variable),
        })
    };
    for field in fields {
        if value_of(field.field).is_none_or(|value| value.value.is_empty()) {
            return Err(format!("{} has no {}, so it can't fill this field.", login.name, field.field.word()));
        }
    }
    // The card it was asked on, if any, says Signing in.
    let card = factory.vault.login_request(agent, origin).ok().flatten().filter(|asked| asked.given_secret_id == login.id);
    let outcome = |outcome: SecretLoginOutcome| {
        if let Some(asked) = &card {
            match factory.vault.set_login_outcome(&asked.id, outcome) {
                Ok(now) => crate::vault::requests::show_everywhere(factory, now),
                Err(error) => tracing::warn!(%error, "a login card's outcome wasn't kept"),
            }
        }
    };
    outcome(SecretLoginOutcome::SigningIn);
    let errand = factory.driving(agent).map(|errand| errand.id).unwrap_or_default();
    let entry = |action: String, outcome: String, target: String| log::Entry {
        errand: errand.clone(),
        agent: agent.to_owned(),
        origin: origin.to_owned(),
        action,
        outcome,
        target,
        ..Default::default()
    };
    let typing = Typing { page, top, origin };
    for field in fields {
        let Some(value) = value_of(field.field) else { continue };
        page.remember_filled(&field.object);
        let typed = typing.type_value(hand, &field.object, &value.value).await;
        let target = format!("{} field", field.field.word());
        if let Err(stopped) = typed {
            log::write(&factory.store, &entry(format!("Typed {} into the {target}", login.name), said(stopped.clone()), target.clone()));
            outcome(SecretLoginOutcome::Unspecified);
            return Err(said(stopped));
        }
        // Where, never what or how much.
        log::write(&factory.store, &entry(format!("Typed {} into the {target}", login.name), String::new(), target.clone()));
        let detail = format!("typed into a {} field on {origin}", field.field.word());
        if let Err(error) = factory.vault.record_fill(&login.id, agent, &errand, &detail) {
            tracing::error!(%error, "a sign-in wasn't logged in the login's uses");
        }
    }
    // Exactly what the login holds, or nothing is sent: a key the
    // browser's keyboard turned into another character leaves the length
    // right and the value wrong. Asked in the reader's world, yes or no.
    for field in fields {
        let Some(value) = value_of(field.field) else { continue };
        if let Err(stopped) = typing.keep(hand, &field.object, &value.value).await {
            log::write(&factory.store, &entry(format!("Typed {} into the {} field", login.name, field.field.word()), said(stopped.clone()), String::new()));
            outcome(SecretLoginOutcome::Unspecified);
            return Err(said(stopped));
        }
    }
    // Sent as a person sends it: Enter in the last field, still holding the
    // keyboard.
    let last = fields.last().map(|field| field.object.as_str()).unwrap_or_default();
    if typing.held(last).await.is_none() {
        let stopped = typing.abort(last).await;
        outcome(SecretLoginOutcome::Unspecified);
        return Err(said(stopped));
    }
    let before = After::of(page).await.unwrap_or_default();
    let pause = gap(hand.rng);
    hand.wait_until(std::time::Instant::now() + pause).await.map_err(said)?;
    typing.press("Enter", "Enter", 13, "\r", hand).await.map_err(said)?;
    let password_field = fields.iter().find(|field| field.field == Field::Password);
    let Some(password_field) = password_field else {
        tokio::time::sleep(super::pace::settle(hand.rng)).await;
        outcome(SecretLoginOutcome::Unspecified);
        return Ok((
            format!("Typed the username of {} on {site} and sent it. Read the page, then call {SIGN_IN_TOOL} with the password field.", login.name),
            Value::Null,
        ));
    };
    let password = value_of(Field::Password).map(|value| value.value.as_str()).unwrap_or_default();
    let (settled, now) = settle(page, hand, &typing, &password_field.object, password, &before).await.map_err(said)?;
    // Never its query or fragment: a sign-in's next address can carry a
    // token, or a form's values.
    let address = url::Url::parse(&now.url).map(|url| format!("{}{}", url.origin().ascii_serialization(), super::fence::path_of(&url))).unwrap_or_default();
    match settled {
        Seen::Settled(Settled::SignedIn) => {
            outcome(SecretLoginOutcome::SignedIn);
            log::write(&factory.store, &entry(format!("Signed in with {}", login.name), String::new(), String::new()));
            Ok((format!("Signed in to {site} with {}: it left its sign-in page for {address}. Read the page to go on.", login.name), Value::Null))
        }
        Seen::Settled(Settled::CodeNext) => {
            outcome(SecretLoginOutcome::CodeNext);
            Ok((
                format!("Typed {} on {site}; it asks for a code next. Read the page, and hand the code to {person} as your browser tools say.", login.name),
                Value::Null,
            ))
        }
        Seen::Settled(Settled::Refused) => {
            log::write(&factory.store, &entry(format!("Signed in with {}", login.name), "the site didn't accept it".to_owned(), String::new()));
            // The person is asked again, on the card they gave it on, or
            // on a new one; Sign in replaces the login's values.
            if let Some(asked) = &card {
                outcome(SecretLoginOutcome::Refused);
                let again = factory.vault.find_request(&asked.id).ok().flatten().is_some_and(|now| now.state() == SecretRequestState::Pending);
                if again {
                    return Ok((
                        format!("{site} didn't accept {}. {person} is asked to check it on the request in your conversation; you're told when it's in.", login.name),
                        Value::Null,
                    ));
                }
            }
            let (request, _) = factory
                .vault
                .ask_login(agent, origin, "", Some((&login.id, &login.name)), SecretLoginOutcome::Refused)
                .map_err(|error| error.to_string())?;
            factory.vault_changed();
            Ok((
                format!("{site} didn't accept {}. {person} is asked to check it; you're told when it's in: then call {SIGN_IN_TOOL} again.", login.name),
                crate::vault::asks::details(&request),
            ))
        }
        Seen::Check(who) => {
            outcome(SecretLoginOutcome::Unspecified);
            log::write(&factory.store, &entry(format!("Signed in with {}", login.name), format!("a check that you're a person ({who})"), String::new()));
            Ok((
                format!(
                    "Typed {} on {site} and sent it, and the site shows a check that you're a person. You're not signed in yet: get past the check yourself (browser_screenshot shows it; act on it as a person would), then read the page to see whether you're signed in.",
                    login.name
                ),
                Value::Null,
            ))
        }
        Seen::Settled(Settled::Form) => {
            outcome(SecretLoginOutcome::Unspecified);
            log::write(&factory.store, &entry(format!("Signed in with {}", login.name), "still on the sign-in page".to_owned(), String::new()));
            Ok((
                format!(
                    "Not signed in: {site} still shows its sign-in form at {address}, with no error Claktory recognized. Take a browser_screenshot to see what it says (an error, a check that you're a person, or something else) and go on from there; don't say you're signed in."
                ),
                Value::Null,
            ))
        }
        Seen::Settled(Settled::Asks(path)) => {
            outcome(SecretLoginOutcome::Unspecified);
            log::write(&factory.store, &entry(format!("Signed in with {}", login.name), format!("the site asks for more at {path}"), String::new()));
            Ok((
                format!(
                    "Typed {} on {site} and sent it. Not signed in: the site went to {address}, which asks for something more (a check, a code or another step). The site hasn't said whether the login was right, so don't say it was accepted or that you're signed in. Take a browser_screenshot and read the page to see what it asks, and go on from there.",
                    login.name
                ),
                Value::Null,
            ))
        }
    }
}

/// What a sent sign-in came to.
enum Seen {
    Settled(Settled),
    /// A check that the visitor is a person: whose.
    Check(String),
}

/// Watches the page after a sign-in was sent until it says how it went:
/// the same answer twice in a row on a loaded page, or the page unchanged
/// for [`QUIET`]. A form that did nothing with Enter for
/// [`ENTER_IGNORED`] (the same document and address, the password still
/// in its field) is sent once with its button, as a person would.
async fn settle(
    page: &Page,
    hand: &mut super::reader::Hand<'_>,
    typing: &Typing<'_>,
    password_field: &str,
    password: &str,
    before: &After,
) -> Result<(Seen, After), Refused> {
    let sent = std::time::Instant::now();
    let mut changed = sent;
    let mut clicked = false;
    let mut last: Option<(String, u64, Option<Settled>)> = None;
    loop {
        hand.wait_until(std::time::Instant::now() + std::time::Duration::from_millis(700)).await?;
        if let Some(who) = page.check().await {
            let now = After::of(page).await.unwrap_or_default();
            return Ok((Seen::Check(who), now));
        }
        let Some(now) = After::of(page).await else { continue };
        let judged = after_sign_in(before, &now);
        let shape = (without_fragment(&now.url).to_owned(), now.passwords, judged.clone());
        if last.as_ref() != Some(&shape) {
            changed = std::time::Instant::now();
        } else if now.ready
            && let Some(settled @ (Settled::SignedIn | Settled::CodeNext | Settled::Refused)) = judged.clone()
        {
            return Ok((Seen::Settled(settled), now));
        }
        let unchanged = without_fragment(&now.url) == without_fragment(&before.url) && now.text == before.text;
        if !clicked && unchanged && sent.elapsed() >= ENTER_IGNORED && typing.kept(password_field, password).await {
            clicked = true;
            if let Some(button) = typing.object(password_field, SUBMIT).await {
                typing.press_on(hand, &button, "sign-in button").await?;
                changed = std::time::Instant::now();
            }
        }
        if changed.elapsed() >= QUIET || sent.elapsed() >= LONGEST {
            let settled = judged.unwrap_or_else(|| Settled::Asks(url::Url::parse(&now.url).map(|url| url.path().to_owned()).unwrap_or_default()));
            return Ok((Seen::Settled(settled), now));
        }
        last = Some(shape);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::schema::Answer;
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

    /// A step's hand for a test: a person drawn from a fixed seed, and an
    /// errand that may always drive.
    fn hand_for<'a>(person: &'a super::super::pace::Person, rng: &'a mut super::super::pace::Rng, pointer: &'a mut Option<super::super::pace::Point>) -> super::super::reader::Hand<'a> {
        static STILL: fn() -> Result<(), (String, String)> = || Ok(());
        super::super::reader::Hand { person, rng, pointer, still: &STILL }
    }

    /// Types `value` into the field named `name` of the page as a sign-in
    /// does: Ok, or why it stopped.
    async fn type_into(page: &Arc<Page>, name: &str, value: &str) -> Result<(), Refused> {
        let mut rng = super::super::pace::Rng::seeded(7);
        let person = super::super::pace::Person::drawn(&mut rng);
        let mut pointer = None;
        let mut hand = hand_for(&person, &mut rng, &mut pointer);
        let (document, target) = read(page, name).await;
        let (object, _) = page.element(&document, &target).await.expect("the field");
        let top = page.frame().await.expect("the page");
        let origin = https_origin(&top.origin);
        Typing { page, top: &top, origin: &origin }.type_value(&mut hand, &object, value).await
    }

    /// Counts the keys the page's own script sees go down, as the browser
    /// sends them (isTrusted), and does `then` at the `at`th.
    fn counting(at: u32, then: &str) -> String {
        format!(
            "<script>window.keys = 0; addEventListener('keydown', e => {{ if (e.isTrusted && ++window.keys === {at}) {{ {then} }} }}, true);</script>"
        )
    }

    #[tokio::test]
    async fn a_login_is_typed_with_the_browsers_own_keys_and_never_logged() {
        let page_html = format!(
            r#"<!doctype html><title>Sign in</title>{}
            <label>Email <input type=email id=email></label>
            <label>Password <input type=password id=pw value="left over"></label>"#,
            counting(0, "")
        );
        let Some((_local, page, profile)) = chrome_on(&[("https://shop.example/login", &page_html)]).await else { return };
        // Everything said at debug level meanwhile, Claktory's and Chrome's
        // connection's alike.
        #[derive(Clone, Default)]
        struct Captured(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for Captured {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                crate::locking::lock(&self.0).extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let captured = Captured::default();
        let writer = captured.clone();
        let subscriber =
            tracing_subscriber::fmt().with_ansi(false).with_max_level(tracing::Level::TRACE).with_writer(move || writer.clone()).finish();
        let logging = tracing::subscriber::set_default(subscriber);
        type_into(&page, "Email", "matt@example.com").await.expect("typed");
        type_into(&page, "Password", "Sw0rdfish!").await.expect("typed");
        drop(logging);
        assert_eq!(page_says(&page, "document.getElementById('email').value").await, "matt@example.com");
        assert_eq!(page_says(&page, "document.getElementById('pw').value").await, "Sw0rdfish!", "typed over what was there");
        // A key went down for each character, as the page saw it.
        let keys = page_says(&page, "window.keys").await.as_u64().expect("counted");
        assert!(keys >= 16 + 10, "{keys}");
        let said = String::from_utf8(crate::locking::lock(&captured.0).clone()).unwrap_or_default();
        assert!(!said.contains("Sw0rdfish") && !said.contains("matt@example.com"), "{said}");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn a_page_that_moves_the_keyboard_mid_password_stops_the_sign_in_and_gets_at_most_one_character() {
        // After the third key the page hands the keyboard to another site's
        // frame, whose field tells the page how much it got.
        let ad = r#"<!doctype html><input id=ad>
            <script>const ad = document.getElementById('ad');
            addEventListener('focus', () => ad.focus());
            ad.addEventListener('input', () => parent.postMessage(ad.value.length, '*'));</script>"#;
        let login = format!(
            r#"<!doctype html><title>Sign in</title>{}
            <script>window.leaked = 0; addEventListener('message', e => {{ window.leaked = e.data; }});</script>
            <label>Password <input type=password id=pw></label>
            <iframe src="https://ads.example/frame"></iframe>"#,
            counting(3, "document.querySelector('iframe').contentWindow.focus();")
        );
        let Some((_local, page, profile)) = chrome_on(&[("https://shop.example/login", &login), ("https://ads.example/frame", ad)]).await
        else {
            return;
        };
        let stopped = type_into(&page, "Password", "abcdefghij").await.unwrap_err();
        assert_eq!(stopped, refused("interrupted", MOVED));
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        assert_eq!(page_says(&page, "document.getElementById('pw').value").await, "", "what went in was taken out");
        let leaked = page_says(&page, "window.leaked").await.as_u64().unwrap_or_default();
        assert!(leaked <= 1, "{leaked} characters reached the other site's frame");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn a_page_that_goes_elsewhere_mid_password_stops_the_sign_in() {
        let login = format!(
            r#"<!doctype html><title>Sign in</title>{}<label>Password <input type=password id=pw></label>"#,
            counting(3, "location.href = 'https://shop.example/elsewhere';")
        );
        let elsewhere = "<!doctype html><title>Elsewhere</title><input type=password>";
        let Some((_local, page, profile)) =
            chrome_on(&[("https://shop.example/login", &login), ("https://shop.example/elsewhere", elsewhere)]).await
        else {
            return;
        };
        let stopped = type_into(&page, "Password", "abcdefghij").await.unwrap_err();
        assert_eq!(stopped.0, "interrupted", "{stopped:?}");
        let _ = std::fs::remove_dir_all(profile);
    }

    #[tokio::test]
    async fn a_field_in_a_frame_is_known_as_one_and_the_pages_own_is_not() {
        let shop = r#"<!doctype html><title>Shop</title><label>Search <input id=q></label>
            <iframe src="https://login.shop.example/frame" width=400 height=300></iframe>"#;
        let framed = r#"<!doctype html><label>Password <input type=password id=pw></label>"#;
        let pages = [("https://shop.example/", shop), ("https://login.shop.example/frame", framed)];
        let Some((_local, page, profile)) = chrome_on(&pages).await else { return };
        let mut read_once = None;
        for _ in 0..50 {
            if let Ok(Answer::Page(read)) = page.read(4096).await
                && let Some(field) = read.frames.iter().flat_map(|frame| frame.elements.iter()).find(|element| element.name.contains("Password"))
            {
                read_once = Some((field.reference.clone(), read.elements.iter().find(|element| element.name.contains("Search")).map(|element| element.reference.clone())));
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        let (framed, own) = read_once.expect("the frame's password field, read");
        assert!(page.in_a_frame(&framed));
        assert!(!page.in_a_frame(&own.expect("the page's own field")));
        let _ = std::fs::remove_dir_all(profile);
    }

    #[test]
    fn a_sign_in_is_signed_in_only_once_the_site_left_its_sign_in_pages() {
        let login = "https://www.linkedin.com/login";
        let form = After { url: login.into(), ready: true, passwords: 1, text: "sign in\nforgot password?".into(), code: false };
        let at = |url: &str, passwords: u64, text: &str| After { url: url.into(), ready: true, passwords, text: text.into(), code: false };
        assert_eq!(after_sign_in(&form, &at("https://www.linkedin.com/feed/", 0, "home")), Some(Settled::SignedIn));
        assert_eq!(after_sign_in(&form, &at("https://x.com/home", 0, "for you")), Some(Settled::SignedIn));
        // Still on the sign-in page, with its form and no new words: not signed in.
        assert_eq!(after_sign_in(&form, &form), Some(Settled::Form));
        assert_eq!(after_sign_in(&form, &at("https://www.linkedin.com/checkpoint/lg/login-submit", 1, "sign in")), Some(Settled::Form));
        // The form again, now saying it's wrong.
        for refusal in ["wrong email or password. try again.", "that's not the right password", "login failed", "we couldn't sign you in.", "unrecognized username"] {
            let refused = after_sign_in(&form, &at(login, 1, &format!("sign in\n{refusal}")));
            assert!(matches!(refused, Some(Settled::Refused | Settled::Form)), "{refusal}");
        }
        assert_eq!(after_sign_in(&form, &at(login, 1, "sign in\nwrong email or password. try again.")), Some(Settled::Refused));
        // Words a refusal uses that were on the form before it was sent say nothing.
        let said_before = At::with(&form, "sign in\ntry again later");
        assert_eq!(after_sign_in(&said_before, &said_before), Some(Settled::Form));
        // Unless said once more, as a page that explains its errors beforehand does.
        let explains = At::with(&form, "Test with an invalid password to see an error.");
        let invalid = At::with(&form, "Your password is invalid!\nTest with an invalid password to see an error.");
        assert_eq!(after_sign_in(&explains, &invalid), Some(Settled::Refused));
        let sauce = At::with(&form, "Epic sadface: Username and password do not match any user in this service");
        assert_eq!(after_sign_in(&form, &sauce), Some(Settled::Refused));
        // The form went, but the site isn't signed in to yet: a check, a challenge, or the same page.
        assert_eq!(after_sign_in(&form, &at("https://www.linkedin.com/checkpoint/challenge/AgF", 0, "let's do a quick security check")), Some(Settled::Asks("/checkpoint/challenge/AgF".into())));
        assert_eq!(after_sign_in(&form, &at("https://www.linkedin.com/login#step2", 0, "")), Some(Settled::Asks("/login".into())));
        assert_eq!(after_sign_in(&form, &at("https://accounts.shop.example/two-factor", 0, "")), Some(Settled::Asks("/two-factor".into())));
        // A code asked for next.
        let code = After { code: true, ..at("https://x.com/i/flow/login", 0, "enter your code") };
        assert_eq!(after_sign_in(&form, &code), Some(Settled::CodeNext));
        // Still loading.
        let loading = After { ready: false, ..at("https://www.linkedin.com/feed/", 0, "") };
        assert_eq!(after_sign_in(&form, &loading), None);
    }

    /// A page as it was, with other words.
    struct At;
    impl At {
        fn with(page: &After, text: &str) -> After {
            After { text: text.into(), ..page.clone() }
        }
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
