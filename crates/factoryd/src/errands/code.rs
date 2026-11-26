//! What only the person may give an errand, and what they settle about
//! its sign-ins (#435 W8, Fugo 41236, Giorno 41238):
//!
//! - **A code sent to the person** (a text message, an email): the agent
//!   names the field (errand_handoff, kind code, with the field as its last
//!   read named it), the person types the code on a card, and Claktory
//!   fills it into that field itself, only if the page and the field are
//!   still the ones the card named. The agent never sees the code; nothing
//!   keeps it (the card's answer is stored empty, never in the Workspace,
//!   the transcript or the log).
//!
//! - **An agreement** (R12.2, Fugo 40593): the agent names the box, the
//!   person reads its words on a card, fenced as the site's, and Accept has
//!   Claktory tick only that box, if the page is the same and the box still
//!   says the same and isn't ticked. A reply ticks nothing and goes to the
//!   agent.
//!
//! - **The first sign-in at an identity provider** (Google, Apple,
//!   Microsoft) from the vault puts a card in the errand agent's
//!   conversation, in Claktory's words: the browser stays signed in there
//!   for later errands, and Approve signs it out. The errand doesn't wait on
//!   it, and the agent can't take it back. Dismissing it keeps the account.
//! - **Signing out** asks the browser (`Browser::sign_out`): done at once,
//!   or, while an errand drives it, when that errand ends. The vault forgets
//!   the account only once it's done (`Factory::signed_out`, which the host
//!   calls), and until then nothing is filled or pressed on that provider's
//!   sites (`fill::while_signing_out`).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use factory_agent::ToolResult;
use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

use super::reader::Page;
use super::{HandoffRow, SignedOut, State, log};
use crate::factory::{Factory, RequestError};
use crate::transcript::now_ms;
use crate::vault::sites::Fillable;

/// Whether the node (in the reader's world) takes a code: a text, number,
/// telephone or password input, or one marked for a one-time code;
/// connected, enabled and writable.
const CODE_FIELD: &str = r#"function () {
  if (!(this instanceof HTMLInputElement)) return false;
  if (!this.isConnected || this.ownerDocument !== document || this.disabled || this.readOnly) return false;
  const type = (this.type || 'text').toLowerCase();
  const tokens = (this.getAttribute('autocomplete') || '').toLowerCase().split(/\s+/);
  return tokens.includes('one-time-code') || ['text', 'tel', 'number', 'password'].includes(type);
}"#;

/// Whether the code field (in the reader's world) takes digits only.
const NUMERIC: &str = r#"function () {
  const type = (this.type || 'text').toLowerCase();
  const mode = (this.getAttribute('inputmode') || '').toLowerCase();
  const pattern = this.getAttribute('pattern') || '';
  return type === 'number' || type === 'tel' || mode === 'numeric' || /^(\[0-9\]|\\d)(\*|\+|\{[0-9,]+\})?$/.test(pattern);
}"#;

/// The most characters a code has.
pub(crate) const CODE_MAX: usize = 16;

/// The field a code card fills, or the box an agreement card ticks, kept
/// from the card's asking to its answer: by Claktory and errand. Never written
/// anywhere: a restart ends the errand anyway.
struct Waiting {
    page: Arc<Page>,
    object: String,
    frame: String,
    loader: String,
    /// The frame's origin as Chrome said it when the card was asked, which
    /// it must still say when Claktory acts.
    chrome_origin: String,
    /// An agreement card's words, which the box must still have.
    words: String,
    /// How Claktory's part went, once the person answered: for the agent.
    done: Option<Result<(), String>>,
}

static WAITING: Mutex<Option<HashMap<String, Waiting>>> = Mutex::new(None);

fn waiting() -> std::sync::MutexGuard<'static, Option<HashMap<String, Waiting>>> {
    crate::locking::lock(&WAITING)
}

/// Tests: the Claktories whose code and agreement cards may be asked on a
/// page Claktory showed itself (`show_for_test`), which has no https origin.
pub(crate) static ANY_ORIGIN_FOR_TEST: Mutex<Vec<std::sync::Weak<Factory>>> = Mutex::new(Vec::new());

/// Whether the node (in the reader's world) is a box to tick that isn't
/// ticked yet: a checkbox or radio input, or an element with the role of
/// one (or a switch); connected and enabled.
const BOX: &str = r#"function () {
  if (!this.isConnected || this.ownerDocument !== document) return false;
  if (this instanceof HTMLInputElement) {
    const type = (this.type || '').toLowerCase();
    return (type === 'checkbox' || type === 'radio') && !this.disabled && !this.checked;
  }
  const role = (this.getAttribute('role') || '').toLowerCase();
  return ['checkbox', 'radio', 'switch'].includes(role) && this.getAttribute('aria-disabled') !== 'true' && this.getAttribute('aria-checked') !== 'true';
}"#;

/// The words a box says, as the person reads them: its labels, the label
/// around it, aria-labelledby or aria-label; never a value.
const BOX_WORDS: &str = r#"function () {
  const text = (el) => (el.innerText || el.textContent || '');
  const by = this.getAttribute('aria-labelledby');
  if (by) {
    const parts = by.split(/\s+/).map((id) => document.getElementById(id)).filter(Boolean).map(text).join(' ');
    if (parts.trim()) return parts;
  }
  if (this.labels && this.labels.length) return Array.from(this.labels).map(text).join(' ');
  const around = this.closest('label');
  if (around) return text(around);
  return this.getAttribute('aria-label') || (this instanceof HTMLInputElement ? '' : text(this));
}"#;

/// Ticks the box (its words and state checked just before), as a click on
/// it (dispatched: the reader's world's click() does nothing): whether it's
/// ticked after.
const TICK: &str = r#"function () {
  if (!this.isConnected || this.ownerDocument !== document) return false;
  this.scrollIntoView({block: 'center'});
  this.dispatchEvent(new MouseEvent('click', {bubbles: true, cancelable: true, composed: true, view: window}));
  return this instanceof HTMLInputElement ? this.checked : this.getAttribute('aria-checked') === 'true';
}"#;

/// The most characters of an agreement's words the card shows.
pub(crate) const WORDS_MAX: usize = 1000;

/// The node an errand's card is about, as the agent's last read named it.
struct Target {
    errand: v1::Errand,
    page: Arc<Page>,
    object: String,
    frame: super::reader::Frame,
    /// Chrome's origin of the page, https only.
    origin: String,
    host: String,
    /// The page's title now, its words.
    title: String,
}

/// Finds `target` of `document` on the page `agent`'s errand drives, on an
/// https page of the errand's sites that isn't an identity provider's
/// sign-in; `card` names the card in what the agent is told.
async fn target(factory: &Arc<Factory>, agent: &str, document: &str, target: &str, card: &str) -> Result<Target, String> {
    let errand = factory.driving(agent).map_err(super::said)?;
    let mut live = factory.errands.live.lock().await;
    let page = factory.page(&mut live, &errand).await.map_err(super::said)?;
    match page.elsewhere(&errand.sites).await {
        Ok(None) => {}
        Ok(Some(origin)) => {
            return Err(super::said(("off_site".to_owned(), format!("this tab is on {origin}, not one of the errand's sites: go back to one with errand_navigate"))));
        }
        Err(error) => return Err(super::said(("interrupted".to_owned(), crate::browser::schema::one_line(&error.to_string(), 200)))),
    }
    let (object, frame) = page.element(document, target).await.map_err(super::said)?;
    let mut origin = url::Url::parse(&frame.origin)
        .ok()
        .filter(|url| url.scheme() == "https")
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default();
    if origin.is_empty() && crate::locking::lock(&ANY_ORIGIN_FOR_TEST).iter().any(|test| test.upgrade().is_some_and(|test| Arc::ptr_eq(&test, factory))) {
        origin = "https://shown.test".to_owned();
    }
    if origin.is_empty() {
        return Err(super::said(("unsupported_target".to_owned(), format!("{card} is only for an https page"))));
    }
    if let Some(provider) = super::fill::provider(&origin) {
        return Err(super::said((
            "sensitive_action".to_owned(),
            format!("{provider}'s sign-in is the person's: hand it over with errand_handoff, kind sign_in"),
        )));
    }
    let title = page
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
        .await
        .ok()
        .and_then(|evaluated| evaluated["result"]["value"].as_str().map(|title| super::handoff::plain_line(title, 300)))
        .unwrap_or_default();
    drop(live);
    let host = url::Url::parse(&origin).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| origin.clone());
    Ok(Target { errand, page, object, frame, origin, host, title })
}

/// What `declaration` (a function on the node) answers, as a bool.
async fn says(page: &Page, object: &str, declaration: &str) -> bool {
    page.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": declaration, "returnByValue": true}))
        .await
        .is_ok_and(|answer| answer["result"]["value"] == true)
}

/// A card Claktory acts on from its answer: the ask, the node kept for it,
/// the errand the person's turn, the log. Its id.
struct Card<'a> {
    agent: &'a str,
    title: String,
    body: String,
    wants_text: bool,
    handoff: HandoffRow,
    words: String,
    logged: &'static str,
    /// Whether Claktory acts on the node later (a code card, an agreement
    /// card), so keeps it; a recovery-codes card read it already.
    keeps_node: bool,
}

fn post(factory: &Arc<Factory>, at: Target, card: Card<'_>) -> Result<i64, String> {
    let action = serde_json::to_string(&crate::card_actions::Action::Handoff { handoff: Box::new(card.handoff.clone()) })
        .map_err(|error| format!("Couldn't hand it over: {error}"))?;
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action) VALUES (?1, ?2, ?3, ?4, ?5, 'pending', '', ?6)",
            params![card.agent, card.title, card.body, card.wants_text, now_ms(), action],
        )
        .map_err(|error| format!("Couldn't hand it over: {error}"))?;
        db.last_insert_rowid()
    };
    if card.keeps_node {
        waiting().get_or_insert_with(HashMap::new).insert(
            errand_key(factory, &at.errand.id),
            Waiting { page: at.page, object: at.object, frame: at.frame.id, loader: at.frame.loader, chrome_origin: at.frame.origin, words: card.words, done: None },
        );
    }
    let mut record = at.errand.clone();
    record.state = v1::ErrandState::PersonsTurn as i32;
    record.handoff = Some(card.handoff.wire());
    factory.save_errand(record, None);
    log::write(
        &factory.store,
        &log::Entry { errand: at.errand.id.clone(), agent: card.agent.to_owned(), origin: at.origin, action: card.logged.to_owned(), ..Default::default() },
    );
    Ok(id)
}

/// what: one sentence, or why not.
fn sentence(what: &str) -> Result<String, String> {
    let what = super::handoff::plain_line(what, super::handoff::WHAT_MAX + 1);
    if what.is_empty() || what.chars().count() > super::handoff::WHAT_MAX {
        return Err(format!("what is one sentence, at most {} characters, of what the person should do.", super::handoff::WHAT_MAX));
    }
    Ok(what)
}

/// errand_handoff with kind code: the person types the code sent to them on
/// a card, and Claktory fills it into `target` of `document`, the field the
/// agent's last read named. What the agent is told, with the card.
pub(crate) async fn hand(factory: &Arc<Factory>, agent: &str, what: &str, document: Option<&str>, target: Option<&str>) -> Result<ToolResult, String> {
    let what = sentence(what)?;
    let (Some(document), Some(node)) = (document.filter(|d| !d.is_empty()), target.filter(|t| !t.is_empty())) else {
        return Err("kind code needs document and target: the field the code goes into, as your last errand_read named it. The person types the code on the card and Claktory fills it in.".to_owned());
    };
    let person = factory.lock().config.person().to_owned();
    let at = self::target(factory, agent, document, node, "a code").await?;
    if !says(&at.page, &at.object, CODE_FIELD).await {
        return Err(super::said(("unsupported_target".to_owned(), "only a text, number or one-time code field takes a code".to_owned())));
    }
    let numeric = says(&at.page, &at.object, NUMERIC).await;
    let title = format!("{} asks for a code", at.host);
    let body = format!(
        "{what}\n\nErrand: {}. Type the code here: Claktory fills it into the field on {} itself, and the agent never sees it. If no code came, press Can't do this.",
        at.errand.purpose, at.origin
    );
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::Code as i32,
        what,
        origin: at.origin.clone(),
        title: at.title.clone(),
        seq: 0,
        errand: at.errand.id.clone(),
        numeric,
        agreement: String::new(),
    };
    let card = Card { agent, title: title.clone(), body: body.clone(), wants_text: true, handoff: handoff.clone(), words: String::new(), logged: "Handed to you: a code", keeps_node: true };
    let id = post(factory, at, card)?;
    let said = format!(
        "Handed to {person} on a card: \"{title}\". They type the code there and Claktory fills it into the field; you never see it. Until they answer, errand_read, errand_act and errand_navigate wait. You're told when it's filled, with the page as it is then; if they can't, the errand ends."
    );
    let details = json!({"ask": id.to_string(), "title": title, "body": body, "wants_text": true, "code": true, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// errand_handoff with kind agreement (R12.2, Fugo 40593): the person reads
/// the box's words on a card, as the site's, and Accept has Claktory tick
/// only that box, `target` of `document`, on that page. What the agent is
/// told, with the card.
pub(crate) async fn hand_agreement(
    factory: &Arc<Factory>,
    agent: &str,
    what: &str,
    document: Option<&str>,
    target: Option<&str>,
) -> Result<ToolResult, String> {
    let what = sentence(what)?;
    let (Some(document), Some(node)) = (document.filter(|d| !d.is_empty()), target.filter(|t| !t.is_empty())) else {
        return Err("kind agreement needs document and target: the box to tick, as your last errand_read named it. The person reads its words on the card, and Claktory ticks it if they accept.".to_owned());
    };
    let person = factory.lock().config.person().to_owned();
    let at = self::target(factory, agent, document, node, "an agreement card").await?;
    if !says(&at.page, &at.object, BOX).await {
        return Err(super::said(("unsupported_target".to_owned(), "only a box that isn't ticked yet (a checkbox, a radio button or a switch) can be accepted on a card".to_owned())));
    }
    let words = at
        .page
        .call("Runtime.callFunctionOn", json!({"objectId": at.object, "functionDeclaration": BOX_WORDS, "returnByValue": true}))
        .await
        .ok()
        .and_then(|answer| answer["result"]["value"].as_str().map(|words| super::handoff::plain_line(words, WORDS_MAX)))
        .unwrap_or_default();
    if words.is_empty() {
        return Err(super::said(("unsupported_target".to_owned(), "this box says nothing the person could read and agree to".to_owned())));
    }
    let title = format!("{} asks you to agree", at.host);
    let body = format!(
        "{what}\n\nErrand: {}. Accept, and Claktory ticks only this box on {}. Reply instead, and nothing is ticked: the agent gets your words. The terms themselves aren't shown here; open {} to read them.",
        at.errand.purpose, at.origin, at.host
    );
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::Agreement as i32,
        what,
        origin: at.origin.clone(),
        title: at.title.clone(),
        seq: 0,
        errand: at.errand.id.clone(),
        numeric: false,
        agreement: words.clone(),
    };
    let card = Card { agent, title: title.clone(), body: body.clone(), wants_text: false, handoff: handoff.clone(), words, logged: "Handed to you: an agreement", keeps_node: true };
    let id = post(factory, at, card)?;
    let said = format!(
        "Handed to {person} on a card: \"{title}\", with the box's words. If they accept, Claktory ticks only that box; if they reply, nothing is ticked and you get their words. Until they answer, errand_read, errand_act and errand_navigate wait. You're told how it went, with the page as it is then; if they can't, the errand ends."
    );
    let details = json!({"ask": id.to_string(), "title": title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// The text of the node (in the reader's world), as the page shows it.
const SHOWN_TEXT: &str = "function () { return this.innerText || this.textContent || ''; }";

/// The fewest and most codes a recovery-codes card takes, and the longest
/// one.
pub(crate) const CODES_MIN: usize = 4;
pub(crate) const CODES_MAX: usize = 32;
const CODE_CHARS_MAX: usize = 32;

/// Whether `word` is code-shaped: letters and digits, 6 to 20 of them, at
/// least one a digit, with at most one hyphen or space inside.
fn code_shaped(word: &str) -> bool {
    let parts: Vec<&str> = word.split(['-', ' ']).collect();
    let bare: String = parts.concat();
    parts.len() <= 2
        && parts.iter().all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric()))
        && (6..=20).contains(&bare.len())
        && bare.chars().any(|c| c.is_ascii_digit())
        && word.len() <= CODE_CHARS_MAX
}

/// The recovery codes in a block's text, in its order: a line that is one
/// code ("1234 5678", "a8f2-91kd", after a list number such as "1."), or
/// else each code-shaped word on it. None unless there are 4 to 32 and
/// nothing else on their lines but list numbers.
pub(crate) fn recovery_codes(text: &str) -> Option<Vec<String>> {
    let mut codes = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        // A list number before it: "1." "2)".
        let line = match line.split_once(['.', ')']) {
            Some((number, rest)) if !number.is_empty() && number.len() <= 2 && number.chars().all(|c| c.is_ascii_digit()) => rest.trim(),
            _ => line,
        };
        if line.is_empty() {
            continue;
        }
        if code_shaped(line) {
            codes.push(line.to_owned());
            continue;
        }
        let words: Vec<&str> = line.split_whitespace().collect();
        if words.iter().all(|word| code_shaped(word)) {
            codes.extend(words.iter().map(|word| (*word).to_owned()));
        }
        // A line of words (a heading, "Each code works once") is the
        // block's, not a code.
    }
    (CODES_MIN..=CODES_MAX).contains(&codes.len()).then_some(codes)
}

/// Recovery codes held in memory while the vault is locked (Fugo 42966),
/// by Claktory and card: a restart loses them, as the card says.
static IN_MEMORY: Mutex<Option<HashMap<String, zeroize::Zeroizing<Vec<String>>>>> = Mutex::new(None);

fn memory_key(factory: &Factory, card: i64) -> String {
    format!("{}#{card}", factory.home.root().display())
}

/// Where a recovery-codes card's codes are: Ok(None) when they're gone.
pub(crate) fn held_codes(factory: &Factory, card: i64) -> Result<Option<zeroize::Zeroizing<Vec<String>>>, String> {
    if let Some(codes) = crate::locking::lock(&IN_MEMORY).as_ref().and_then(|all| all.get(&memory_key(factory, card))) {
        return Ok(Some(codes.clone()));
    }
    match factory.vault.held_codes(&card.to_string()) {
        Ok(codes) => Ok(codes),
        Err(crate::vault::VaultError::Locked) => {
            Err("Claktory's vault is locked: unlock it in Settings → Secrets to see these codes.".to_owned())
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Forgets a recovery-codes card's codes, wherever they're held.
pub(crate) fn drop_codes(factory: &Factory, card: i64) {
    if let Some(all) = crate::locking::lock(&IN_MEMORY).as_mut() {
        all.remove(&memory_key(factory, card));
    }
    if let Err(error) = factory.vault.drop_codes(&card.to_string()) {
        tracing::error!(%error, card, "a recovery-codes card's codes weren't deleted from the vault");
    }
}

/// errand_handoff with kind recovery_codes (#435 W8b, Fugo 40691, 42966):
/// Claktory reads the codes off `target` of `document`, the block the
/// agent's last read withheld, seals them in the vault at once (in memory
/// while it's locked), and the person gets them on a card. The agent never
/// sees them. What the agent is told, with the card.
pub(crate) async fn hand_codes(factory: &Arc<Factory>, agent: &str, what: &str, document: Option<&str>, target: Option<&str>) -> Result<ToolResult, String> {
    let what = sentence(what)?;
    let (Some(document), Some(node)) = (document.filter(|d| !d.is_empty()), target.filter(|t| !t.is_empty())) else {
        return Err("kind recovery_codes needs document and target: the block your last errand_read withheld as recovery codes. Claktory reads them and hands them to the person; you never see them.".to_owned());
    };
    let person = factory.lock().config.person().to_owned();
    let at = self::target(factory, agent, document, node, "recovery codes").await?;
    let text = at
        .page
        .call("Runtime.callFunctionOn", json!({"objectId": at.object, "functionDeclaration": SHOWN_TEXT, "returnByValue": true}))
        .await
        .ok()
        .and_then(|answer| answer["result"]["value"].as_str().map(|text| zeroize::Zeroizing::new(text.to_owned())))
        .unwrap_or_default();
    let Some(codes) = recovery_codes(&text).map(zeroize::Zeroizing::new) else {
        return Err(super::said((
            "unsupported_target".to_owned(),
            format!("this doesn't look like a list of recovery codes ({CODES_MIN} to {CODES_MAX} codes of letters and digits): name the block that holds them"),
        )));
    };
    sweep_codes(factory);
    let sealed = factory.vault.is_open();
    let title = format!("{} recovery codes", at.host);
    let first = if sealed {
        "Claktory keeps them safe until you choose."
    } else {
        "Secrets is locked, so Claktory keeps them only until it restarts: save them now."
    };
    let body = format!(
        "{first}\n\n{what}\n\nErrand: {}. {} showed these codes once, and the agent never saw them. Show them and save them somewhere safe, then choose I saved them, or choose Keep in Secrets.",
        at.errand.purpose, at.host
    );
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::RecoveryCodes as i32,
        what,
        origin: at.origin.clone(),
        title: at.title.clone(),
        seq: 0,
        errand: at.errand.id.clone(),
        numeric: false,
        agreement: String::new(),
    };
    let card =
        Card { agent, title: title.clone(), body: body.clone(), wants_text: false, handoff: handoff.clone(), words: String::new(), logged: "Handed to you: recovery codes", keeps_node: false };
    let id = post(factory, at, card)?;
    let held = if sealed { factory.vault.hold_codes(&id.to_string(), &codes) } else { Err(crate::vault::VaultError::Locked) };
    if let Err(error) = held {
        if !matches!(error, crate::vault::VaultError::Locked) {
            tracing::error!(%error, "recovery codes weren't sealed in the vault: kept in memory");
        }
        crate::locking::lock(&IN_MEMORY).get_or_insert_with(HashMap::new).insert(memory_key(factory, id), codes);
    }
    let said = format!(
        "Claktory read the recovery codes off the page and handed them to {person} on a card: \"{title}\". You never see them. Until they've saved them, errand_read, errand_act and errand_navigate wait. You're told when they have, with the page as it is then."
    );
    let details = json!({"ask": id.to_string(), "title": title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// The person saved recovery-codes card `handoff`'s codes (`kept`: the
/// secret they're kept in): the errand goes on, told so, with the page as
/// it is now; or, once it's over, the agent is told only that.
pub(crate) async fn codes_saved(factory: &Arc<Factory>, agent: &str, handoff: &HandoffRow, kept: Option<&str>) {
    let person = factory.lock().config.person_capitalized().to_owned();
    let (logged, told) = match kept {
        Some(name) => ("You kept the recovery codes in Secrets", format!("{person} kept the recovery codes in Secrets, as {name}, given to no agent.")),
        None => ("You saved the recovery codes", format!("{person} saved the recovery codes.")),
    };
    let Some(errand) = factory.errands.get(&handoff.errand).filter(|errand| State::from_wire(errand.state) == State::PersonsTurn) else {
        log::write(&factory.store, &log::Entry { errand: handoff.errand.clone(), origin: handoff.origin.clone(), action: logged.to_owned(), ..Default::default() });
        factory.deliver_event(agent, format!("{told} (Errand {} was over by then.)", handoff.errand), crate::factory::host_event("Recovery codes saved".to_owned()));
        return;
    };
    let mut record = errand.clone();
    record.state = v1::ErrandState::Running as i32;
    record.handoff = None;
    factory.save_errand(record, None);
    log::write(&factory.store, &log::Entry { errand: errand.id.clone(), origin: handoff.origin.clone(), action: logged.to_owned(), ..Default::default() });
    let page = match factory.errand_step_handing(agent, super::Step::Read { limit: 4096 }, false).await {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with errand_read."),
    };
    factory.deliver_event(agent, format!("{told} {page}"), crate::factory::host_event(format!("Errand {}: your turn again", errand.id)));
}

/// Two-step sign-in an errand may turn on, approved by the person (#435
/// W8b, Fugo 40691, 42966), by Claktory and errand: the origins. Only in
/// memory: a restart ends the errand anyway.
static APPROVED: Mutex<Option<HashMap<String, Vec<String>>>> = Mutex::new(None);
/// The origin, as Chrome says it, a two-step card waiting for the person
/// would approve: by Claktory and errand.
static ASKED: Mutex<Option<HashMap<String, String>>> = Mutex::new(None);

fn errand_key(factory: &Factory, errand: &str) -> String {
    format!("{}#{errand}", factory.home.root().display())
}

/// The origins where `errand` may turn on two-step sign-in.
pub(crate) fn two_step_origins(factory: &Factory, errand: &str) -> Vec<String> {
    crate::locking::lock(&APPROVED).as_ref().and_then(|all| all.get(&errand_key(factory, errand)).cloned()).unwrap_or_default()
}

/// Why a two-step control isn't the agent's.
pub(crate) const TWO_STEP: &str = "two-step sign-in: the person decides (errand_handoff, kind two_step)";
/// Why an authenticator, security key or passkey isn't the agent's even
/// when two-step is approved.
pub(crate) const TWO_STEP_METHOD: &str =
    "an authenticator app, security key or passkey: never the agent's (two-step only with codes by text message or email, once the person approves)";

/// Whether a control's words (its name and what it shows) turn on two-step
/// sign-in or set up a method of it, and so aren't the agent's on `origin`
/// unless `approved` holds it, and an authenticator app, a security key or
/// a passkey never: why, or None. A heuristic over words, as the reader's
/// rules are; it errs toward the person.
pub(crate) fn two_step_refusal(words: &str, origin: &str, approved: &[String]) -> Option<&'static str> {
    let words = words.to_lowercase();
    let has = |list: &[&str]| list.iter().any(|word| words.contains(word));
    let subject = has(&["two-step", "2-step", "two step", "2 step", "two-factor", "2-factor", "two factor", "2fa", "multi-factor", "mfa", "login verification", "authenticator", "security key", "passkey"]);
    let turns_on = has(&["turn on", "enable", "set up", "setup", "add ", "activate", "get started", "start", "use ", "choose", "create", "register", "next"]);
    if !subject || !turns_on {
        return None;
    }
    if has(&["authenticator", "security key", "passkey", "totp", "yubikey", "google prompt", "authy"]) {
        return Some(TWO_STEP_METHOD);
    }
    if approved.iter().any(|allowed| allowed == origin) { None } else { Some(TWO_STEP) }
}

/// errand_handoff with kind two_step (#435 W8b, Fugo 40691, option 4(c)):
/// turning on two-step sign-in on this page's site is the person's call; at
/// an identity provider, never an errand's. What the agent is told, with
/// the card.
pub(crate) async fn hand_two_step(factory: &Arc<Factory>, agent: &str, what: &str) -> Result<ToolResult, String> {
    let what = sentence(what)?;
    let person = factory.lock().config.person().to_owned();
    let errand = factory.driving(agent).map_err(super::said)?;
    let mut live = factory.errands.live.lock().await;
    let page = factory.page(&mut live, &errand).await.map_err(super::said)?;
    drop(live);
    let frame = page.frame().await.map_err(|error| super::said(("interrupted".to_owned(), crate::browser::schema::one_line(&error.to_string(), 200))))?;
    let mut origin = url::Url::parse(&frame.origin)
        .ok()
        .filter(|url| url.scheme() == "https")
        .map(|url| url.origin().ascii_serialization())
        .unwrap_or_default();
    if origin.is_empty() && crate::locking::lock(&ANY_ORIGIN_FOR_TEST).iter().any(|test| test.upgrade().is_some_and(|test| Arc::ptr_eq(&test, factory))) {
        origin = "https://shown.test".to_owned();
    }
    if origin.is_empty() || !super::reader::errands_own(&errand.sites, &frame.url) && origin != "https://shown.test" {
        return Err(super::said(("unsupported_target".to_owned(), "two-step sign-in is handed over only on an https page of the errand's sites".to_owned())));
    }
    if let Some(provider) = super::fill::provider(&origin) {
        return Err(super::said(("sensitive_action".to_owned(), format!("{provider}'s two-step sign-in is never an errand's: report it to your lead"))));
    }
    let host = url::Url::parse(&origin).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or_else(|| origin.clone());
    let title = format!("{host} wants two-step sign-in turned on");
    let body = format!(
        "{what}\n\nErrand: {}. Approve to let the agent turn on two-step sign-in on {origin} for this errand. Codes come to you by text message or email only, never through an authenticator app, a security key or a passkey, and any recovery codes come to you on a card. Can't do this ends the errand.",
        errand.purpose
    );
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::TwoStep as i32,
        what,
        origin: origin.clone(),
        title: String::new(),
        seq: 0,
        errand: errand.id.clone(),
        numeric: false,
        agreement: String::new(),
    };
    crate::locking::lock(&ASKED).get_or_insert_with(HashMap::new).insert(errand_key(factory, &errand.id), frame.origin.clone());
    let at = Target { errand, page, object: String::new(), frame, origin, host, title: String::new() };
    let card = Card { agent, title: title.clone(), body: body.clone(), wants_text: false, handoff: handoff.clone(), words: String::new(), logged: "Asked you about two-step sign-in", keeps_node: false };
    let id = post(factory, at, card)?;
    let said = format!(
        "Handed to {person} on a card: \"{title}\". Until they answer, errand_read, errand_act and errand_navigate wait. Approved, you're told so, with the page as it is then; if they can't, the errand ends."
    );
    let details = json!({"ask": id.to_string(), "title": title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// The person approved two-step card `handoff`: the errand may turn it on
/// at its origin, and goes on, told so with the page as it is now.
pub(crate) async fn two_step_approved(factory: &Arc<Factory>, agent: &str, handoff: &HandoffRow) {
    let Some(errand) = factory.errands.get(&handoff.errand).filter(|errand| State::from_wire(errand.state) == State::PersonsTurn) else { return };
    // The origin Chrome said when the card was asked, which the reader
    // compares with Chrome's own.
    let asked = crate::locking::lock(&ASKED).as_mut().and_then(|all| all.remove(&errand_key(factory, &errand.id)));
    if let Some(origin) = asked {
        let mut all = crate::locking::lock(&APPROVED);
        let origins = all.get_or_insert_with(HashMap::new).entry(errand_key(factory, &errand.id)).or_default();
        if !origins.contains(&origin) {
            origins.push(origin);
        }
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    let mut record = errand.clone();
    record.state = v1::ErrandState::Running as i32;
    record.handoff = None;
    factory.save_errand(record, None);
    log::write(
        &factory.store,
        &log::Entry { errand: errand.id.clone(), origin: handoff.origin.clone(), action: format!("You approved two-step sign-in on {}", handoff.origin), ..Default::default() },
    );
    let page = match factory.errand_step_handing(agent, super::Step::Read { limit: 4096 }, false).await {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with errand_read."),
    };
    factory.deliver_event(
        agent,
        format!(
            "{person} approved turning on two-step sign-in on {}. Turn it on with codes sent by text message or email only: an authenticator app, a security key or a passkey stays theirs, and is refused. A code sent to them goes through errand_handoff, kind code; recovery codes the site shows, through kind recovery_codes, as soon as they show. {page}",
            handoff.origin
        ),
        crate::factory::host_event(format!("Errand {}: your turn again", errand.id)),
    );
}

/// Sealed recovery codes whose card no longer waits for the person (closed
/// with the conversation by /clear or a fork, or its agent deleted) become
/// a secret given to no agent, so the person never loses them without
/// choosing (Fugo 43868); only I saved them deletes them for good. A card
/// Claktory can't read the state of keeps its codes for the next sweep.
/// At start and with each new recovery-codes card.
pub(crate) fn sweep_codes(factory: &Factory) {
    let Ok(cards) = factory.vault.code_cards() else { return };
    let mut kept = false;
    for card in cards {
        let row: rusqlite::Result<Option<(String, String)>> = match card.parse::<i64>() {
            Ok(id) => factory
                .store
                .db()
                .query_row("SELECT state, action FROM asks WHERE id = ?1", [id], |row| Ok((row.get(0)?, row.get(1)?)))
                .optional(),
            Err(_) => Ok(None),
        };
        let handoff = match row {
            Err(error) => {
                tracing::error!(%error, %card, "couldn't read a recovery-codes card's state: its codes stay sealed");
                continue;
            }
            Ok(Some((state, _))) if state == "pending" => continue,
            Ok(Some((_, action))) => match serde_json::from_str(&action) {
                Ok(crate::card_actions::Action::Handoff { handoff }) => Some(*handoff),
                _ => None,
            },
            Ok(None) => None,
        };
        let codes = match factory.vault.held_codes(&card) {
            Ok(Some(codes)) => codes,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!(%error, %card, "an orphaned card's recovery codes couldn't be read: they stay sealed");
                continue;
            }
        };
        let host = handoff
            .as_ref()
            .and_then(|handoff| url::Url::parse(&handoff.origin).ok())
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default();
        let site = if host.is_empty() { "A site".to_owned() } else { host };
        let why = format!("Recovery codes for {site} were kept in Secrets because their card closed before you chose.");
        match factory.vault.keep_codes(&site, &codes, &why) {
            Ok(name) => {
                if let Some(handoff) = &handoff {
                    log::write(
                        &factory.store,
                        &log::Entry { errand: handoff.errand.clone(), origin: handoff.origin.clone(), action: format!("Recovery codes kept in Secrets as {name}: their card closed"), ..Default::default() },
                    );
                }
                let _ = factory.vault.drop_codes(&card);
                kept = true;
            }
            Err(error) => tracing::error!(%error, %card, "an orphaned card's recovery codes weren't kept in Secrets: they stay sealed"),
        }
    }
    // Settings → Secrets shows the new secret.
    if kept {
        factory.broadcast_workspace(&factory.lock());
    }
}

/// Why Claktory didn't act on a card: the person can try again on it
/// (`Retry`), or the node is gone from Claktory's memory (`Gone`: the card
/// is answered, and the agent told).
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum NotFilled {
    Retry(String),
    Gone,
}

/// The node kept for `errand`'s card, if the page is still the one it was
/// asked on: the page and the node; or why not (`what` says what didn't
/// arrive in time).
async fn still_there(factory: &Factory, errand: &str, what: &str) -> Result<Result<(Arc<Page>, String, String), String>, NotFilled> {
    let held = waiting()
        .as_ref()
        .and_then(|all| all.get(&errand_key(factory, errand)))
        .map(|held| (held.page.clone(), held.object.clone(), held.frame.clone(), held.loader.clone(), held.chrome_origin.clone(), held.words.clone()));
    let Some((page, object, frame, loader, origin, words)) = held else { return Err(NotFilled::Gone) };
    let Ok(now) = page.frame().await else { return Ok(Err("the page went".to_owned())) };
    if now.id != frame || now.loader != loader || now.origin != origin {
        return Ok(Err(format!("the page changed before {what}")));
    }
    Ok(Ok((page, object, words)))
}

/// Keeps how Claktory's part went for `errand`'s card, for the agent.
fn keep(factory: &Factory, errand: &str, outcome: &Result<(), String>) {
    if let Some(held) = waiting().as_mut().and_then(|all| all.get_mut(&errand_key(factory, errand))) {
        held.done = Some(outcome.clone());
    }
}

/// The person typed `code` on code card `handoff`: Claktory fills it into
/// the field the card named, once, if the page and its document are still
/// the ones it was asked on. Called before the card's answer is kept, so a
/// code that isn't one leaves it waiting (`Retry`). Ok holds how the fill
/// went: Err(why) ends the card all the same, saying why on it.
pub(crate) async fn fill(factory: &Factory, handoff: &HandoffRow, code: &str) -> Result<Result<(), String>, NotFilled> {
    let code = zeroize::Zeroizing::new(code.trim().to_owned());
    if code.is_empty()
        || code.chars().count() > CODE_MAX
        || !code.chars().all(|c| c.is_ascii_alphanumeric() || c == ' ' || c == '-')
    {
        return Err(NotFilled::Retry(format!("A code is up to {CODE_MAX} letters and digits, as it was sent to you.")));
    }
    let outcome = match still_there(factory, &handoff.errand, "the code arrived").await? {
        Err(why) => Err(why),
        Ok((page, object, _)) => {
            let argument = || vec![json!({"value": code.as_str()})];
            let call = |declaration: &'static str| {
                page.call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": declaration, "arguments": argument(), "returnByValue": true}))
            };
            match call(super::fill::SET).await {
                Ok(set) if set["result"]["value"] == true => {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    if call(super::fill::KEPT).await.is_ok_and(|kept| kept["result"]["value"] == true) {
                        Ok(())
                    } else {
                        Err("the page didn't keep the code".to_owned())
                    }
                }
                _ => Err("the field went before the code arrived".to_owned()),
            }
        }
    };
    keep(factory, &handoff.errand, &outcome);
    Ok(outcome)
}

/// The person accepted agreement card `handoff`: Claktory ticks the box the
/// card named, once, if the page is still the one it was asked on and the
/// box still says what the card showed and isn't ticked. Called before the
/// card's answer is kept. Ok holds how it went: Err(why) ends the card all
/// the same, saying why on it.
pub(crate) async fn tick(factory: &Factory, handoff: &HandoffRow) -> Result<Result<(), String>, NotFilled> {
    let outcome = match still_there(factory, &handoff.errand, "you accepted").await? {
        Err(why) => Err(why),
        Ok((page, object, words)) => {
            let now = page
                .call("Runtime.callFunctionOn", json!({"objectId": object, "functionDeclaration": BOX_WORDS, "returnByValue": true}))
                .await
                .ok()
                .and_then(|answer| answer["result"]["value"].as_str().map(|now| super::handoff::plain_line(now, WORDS_MAX)));
            if now.as_deref() != Some(words.as_str()) {
                Err("the box's words changed before you accepted".to_owned())
            } else if !says(&page, &object, BOX).await {
                Err("the box was already ticked, or went".to_owned())
            } else {
                let ticked = says(&page, &object, TICK).await;
                if ticked { Ok(()) } else { Err("the page didn't keep the box ticked".to_owned()) }
            }
        }
    };
    keep(factory, &handoff.errand, &outcome);
    Ok(outcome)
}

/// Why a card's answer went nowhere when Claktory no longer holds its
/// node (`NotFilled::Gone`).
pub(crate) const LOST: &str = "Claktory had lost the page (it restarted, or the browser did)";

/// The person answered code or agreement card `handoff` (Claktory's part
/// done, or not; or, on an agreement, replied with `reason`, nothing
/// ticked) or dismissed it: the errand goes on, told how it went and what
/// the page is now, or ends with their reason.
pub(crate) async fn answered(factory: &Arc<Factory>, agent: &str, handoff: &HandoffRow, done: bool, reason: &str) {
    let held = waiting().as_mut().and_then(|all| all.remove(&errand_key(factory, &handoff.errand)));
    let Some(errand) = factory.errands.get(&handoff.errand) else { return };
    if State::from_wire(errand.state) != State::PersonsTurn {
        return;
    }
    if !done {
        // Dismissed: as any handoff's Can't do this.
        return super::handoff::answered(factory, agent, handoff, false, reason).await;
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    let agreement = handoff.kind == v1::ErrandHandoffKind::Agreement as i32;
    let replied = agreement && !reason.trim().is_empty();
    let outcome = held.and_then(|held| held.done).unwrap_or_else(|| Err(LOST.to_owned()));
    let (logged, why, told) = match (&outcome, agreement) {
        _ if replied => (
            "You replied: nothing ticked".to_owned(),
            String::new(),
            format!(
                "{person} replied on the agreement card, and nothing was ticked. They say:\n\n{}\n\nDo what they say if you can; the box is still theirs to accept.",
                super::handoff::plain_line(reason, 2000)
            ),
        ),
        (Ok(()), false) => (format!("Code filled on {}", handoff.origin), String::new(), format!("{person} typed the code on the card, and Claktory filled it in.")),
        (Ok(()), true) => (format!("Agreement ticked on {}", handoff.origin), String::new(), format!("{person} accepted on the card, and Claktory ticked the box.")),
        (Err(why), false) => (
            "Couldn't fill the code".to_owned(),
            why.clone(),
            format!("{person} typed the code on the card, but Claktory couldn't fill it in: {why}. Read the page; if it still asks, hand it over again."),
        ),
        (Err(why), true) => (
            "Couldn't tick the agreement".to_owned(),
            why.clone(),
            format!("{person} accepted on the card, but Claktory couldn't tick the box: {why}. Read the page; if it still asks, hand it over again."),
        ),
    };
    let mut record = errand.clone();
    record.state = v1::ErrandState::Running as i32;
    record.handoff = None;
    factory.save_errand(record, None);
    log::write(&factory.store, &log::Entry { errand: errand.id.clone(), origin: handoff.origin.clone(), action: logged, outcome: why, ..Default::default() });
    let page = match factory.errand_step_handing(agent, super::Step::Read { limit: 4096 }, false).await {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with errand_read."),
    };
    factory.deliver_event(agent, format!("{told} {page}"), crate::factory::host_event(format!("Errand {}: your turn again", errand.id)));
}

/// The card's title.
fn title(provider: &str) -> String {
    format!("Claktory's browser is signed in to {provider}")
}

/// Posts the card telling the person the browser just signed in to
/// `provider` with `fillable`, for `errand`: its details, for the fill's
/// result, which shows it in `agent`'s conversation; or why not.
pub(crate) fn signed_in(factory: &Arc<Factory>, agent: &str, errand: &v1::Errand, provider: &str, fillable: &Fillable) -> Result<Value, String> {
    let title = title(provider);
    let description = format!(
        "For the errand \"{}\", Claktory's browser signed in to {provider} with {}, and stays signed in there for later errands. Approve signs it out of {provider}: at once, or when the errand ends if it's still going. Dismiss to stay signed in.",
        errand.purpose, fillable.name
    );
    let action = crate::card_actions::Action::BrowserSignOut {
        provider: provider.to_owned(),
        secret: fillable.id.clone(),
        errand: errand.id.clone(),
    };
    let action = serde_json::to_string(&action).map_err(|error| error.to_string())?;
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action, action_description, action_reason) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5, ?3, '')",
            params![agent, title, description, now_ms(), action],
        )
        .map_err(|error| error.to_string())?;
        db.last_insert_rowid()
    };
    log::write(
        &factory.store,
        &log::Entry {
            errand: errand.id.clone(),
            agent: agent.to_owned(),
            action: format!("Asked you whether the browser stays signed in to {provider}"),
            ..Default::default()
        },
    );
    Ok(json!({"ask": id.to_string(), "title": title, "body": description, "wants_text": false, "action_description": description}))
}

/// Whether the card can still be approved: the browser is still signed in
/// to `provider` with `secret`.
pub(crate) fn check(factory: &Factory, provider: &str, secret: &str) -> Result<(), String> {
    if factory.vault.browser_account(provider).as_deref() == Some(secret) {
        return Ok(());
    }
    Err(format!("Claktory's browser isn't signed in to {provider} with that account any more: dismiss this card."))
}

/// The person signs the browser out of `provider` (by name, "Google"):
/// approving the card of errand `errand`, or with Sign out in the browser's
/// settings (`errand` empty: nothing to log it in). What was done, or why
/// it couldn't be, as the card's answer says it.
pub(crate) async fn sign_out(factory: &Arc<Factory>, provider: &str, errand: &str) -> Result<String, String> {
    let browser = factory.errands.browser().ok_or("Claktory's browser isn't available here, so it couldn't sign out.")?;
    let signed_out = browser.sign_out(provider).await;
    let (action, outcome) = match &signed_out {
        Ok(SignedOut::Applied) => (format!("Signed out of {provider}"), String::new()),
        Ok(SignedOut::Pending) => (format!("Signing out of {provider} when the errand ends"), String::new()),
        Err(why) => (format!("Signing out of {provider}"), why.clone()),
    };
    if !errand.is_empty() {
        log::write(&factory.store, &log::Entry { errand: errand.to_owned(), action, outcome, ..Default::default() });
    }
    // The browser's settings show it signing out, or signed out.
    factory.broadcast_workspace(&factory.lock());
    match signed_out {
        Ok(SignedOut::Applied) => {
            factory.signed_out(provider);
            Ok(format!("Claktory's browser is signed out of {provider}."))
        }
        Ok(SignedOut::Pending) => Ok(format!(
            "Claktory's browser signs out of {provider} when the errand using it ends; until then nothing is filled or pressed on {provider}'s sites. It hasn't signed out yet."
        )),
        Err(why) => Err(format!("signing out of {provider} didn't work: {why}")),
    }
}

impl Factory {
    /// The browser is signed out of `provider` (its cookies deleted): the
    /// vault forgets its account there, so the next sign-in tells the
    /// person again. The host calls it each time a sign-out is applied;
    /// calling it again changes nothing.
    pub(crate) fn signed_out(&self, provider: &str) {
        if self.vault.browser_account(provider).is_none() {
            return;
        }
        if let Err(error) = self.vault.forget_browser_accounts(Some(provider)) {
            tracing::error!(%error, %provider, "the vault didn't forget the browser's account after a sign-out");
        }
        self.broadcast_workspace(&self.lock());
    }

    /// The host's cookie check saw `provider`'s session cookies in the
    /// browser (Giorno 41843): at an errand's end, after a sign-in turn, or
    /// on the turn's own end. The first time, the vault records the account
    /// (with no secret: the person signed in themselves) and the person gets
    /// the first-sign-in card with Sign out. Idempotent: the host calls it
    /// again while the cookies are there.
    pub(crate) fn browser_signed_in(&self, provider: &str) {
        // The card itself comes with the sign-in turn's card (after W8):
        // until then the account is recorded, and shown in the browser's
        // settings with Sign out.
        match self.vault.set_browser_account(provider, "") {
            Ok(true) => self.broadcast_workspace(&self.lock()),
            Ok(false) => {}
            Err(error) => tracing::error!(%error, %provider, "the browser's account at an identity provider wasn't kept"),
        }
    }

    /// Refused on `page` while the browser waits to sign out of the
    /// identity provider whose site it is on: no click or fill there.
    pub(crate) async fn while_signing_out(&self, page: &Arc<Page>) -> Option<(String, String)> {
        let browser = self.errands.browser()?;
        let origin = page.frame().await.ok()?.origin;
        super::fill::while_signing_out(&origin, |provider| browser.signing_out(provider))
    }
}

/// The identity providers the browser is signed in to, for the Workspace
/// (AgentBrowser.accounts); `signing_out`, the host's word on whether a
/// sign-out of one waits.
pub(crate) fn accounts(factory: &Factory, signing_out: impl Fn(&str) -> bool) -> Vec<v1::AgentBrowserAccount> {
    factory
        .vault
        .browser_accounts()
        .into_iter()
        .map(|account| v1::AgentBrowserAccount {
            provider: account.provider.to_lowercase(),
            signing_out: signing_out(&account.provider),
            name: account.provider,
            secret_id: account.secret_id,
            secret_name: account.secret_name,
            signed_in_ms: account.since,
        })
        .collect()
}

/// SignOutAgentBrowserAccount: the person's Sign out beside an account in
/// the browser's settings.
pub(crate) async fn sign_out_request(factory: &Arc<Factory>, request: &v1::SignOutAgentBrowserAccount) -> Result<(), RequestError> {
    let refused = |why: String| RequestError::Command(why);
    let Some(provider) = super::fill::IDENTITY_PROVIDERS.iter().find(|known| known.name.eq_ignore_ascii_case(request.provider.trim())) else {
        return Err(refused("Claktory's browser doesn't sign in to that provider.".to_owned()));
    };
    let name = provider.name;
    if factory.vault.browser_account(name).is_none() {
        return Err(refused(format!("Claktory's browser isn't signed in to {name}.")));
    }
    let errand = factory.errands.begun().map(|errand| errand.id).unwrap_or_default();
    sign_out(factory, name, &errand).await.map(|_| ()).map_err(|why| {
        let mut chars = why.chars();
        refused(chars.next().map(|first| first.to_uppercase().chain(chars).collect()).unwrap_or(why))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recovery_codes_are_read_line_by_line_and_nothing_else_is_one() {
        let google = "Backup codes\n1. 1234 5678\n2. 2345 6789\n3. 3456 7890\n4. 4567 8901\nEach code works once.";
        assert_eq!(recovery_codes(google), Some(vec!["1234 5678".into(), "2345 6789".into(), "3456 7890".into(), "4567 8901".into()]));
        let grid = "a8f2-91kd c3m9-2wle\nk1vv-o3ds 7bq1-x0pe\np0za-77rt";
        assert_eq!(recovery_codes(grid).map(|codes| codes.len()), Some(5));
        // Too few, words, dates and order numbers aren't codes.
        assert_eq!(recovery_codes("a8f2-91kd\nc3m9-2wle"), None);
        assert_eq!(recovery_codes("Order 1234 shipped on 2026-11-02\nThanks for shopping"), None);
        assert_eq!(recovery_codes(&"x1y2z3w4\n".repeat(CODES_MAX + 1)), None);
    }

    #[test]
    fn turning_on_two_step_is_the_persons_and_an_authenticator_never_the_agents() {
        let shop = "https://shop.example";
        let approved = vec![shop.to_owned()];
        for words in ["Turn on 2-Step Verification", "Enable two-factor authentication", "Set up 2FA "] {
            assert_eq!(two_step_refusal(words, shop, &[]), Some(TWO_STEP), "{words}");
            assert_eq!(two_step_refusal(words, shop, &approved), None, "{words}");
            assert_eq!(two_step_refusal(words, "https://other.example", &approved), Some(TWO_STEP), "{words}");
        }
        for words in ["Set up an authenticator app", "Add a security key", "Create a passkey", "Use Google Authenticator "] {
            assert_eq!(two_step_refusal(words, shop, &approved), Some(TWO_STEP_METHOD), "{words}");
        }
        for words in ["Learn about two-step verification", "Continue", "Add to cart", "Next"] {
            assert_eq!(two_step_refusal(words, shop, &[]), None, "{words}");
        }
    }
}
