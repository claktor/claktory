//! Handoffs: what only the person may do on a page, handed to them in a
//! request through Ask: a payment (this module: they open the page's address
//! in their own browser, so the agent's never holds a card), a check that
//! the visitor is a person the agent couldn't get past (this module: they
//! get past it in a live view of that one tab), a code, an agreement,
//! recovery codes or two-step sign-in (`code`). While the request waits the
//! agent's browser tools wait; however the person answers, the browser is
//! the agent's again and it's told how it went.

use std::sync::Arc;

use factory_agent::ToolResult;
use factory_protocol::v1;
use rusqlite::params;
use serde_json::json;

use super::{HandoffRow, State, log};
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// The tool that hands the person a moment on a card.
pub(crate) const TOOL: &str = "browser_handoff";

/// The tools whose result can be an errand's card: browser_handoff, the
/// steps that find a check that the visitor is a person, and a fill that
/// is the browser's first sign-in at an identity provider
/// (`super::code::signed_in`).
pub(crate) const TOOLS: [&str; 6] = [TOOL, "browser_read", "browser_act", "browser_open", "browser_download", super::fill::SIGN_IN_TOOL];

/// The most characters of what the agent says the person should do.
pub(crate) const WHAT_MAX: usize = 300;

/// A kind, from the agent's word: payment and check here; code and
/// agreement in `code`. The kinds that handed the person the whole browser
/// (sign_in, identity, other) went with browser-simple: their values stay
/// in the protocol for requests made before.
pub(crate) fn kind_of(word: &str) -> Option<v1::ErrandHandoffKind> {
    Some(match word {
        "payment" => v1::ErrandHandoffKind::Payment,
        "check" => v1::ErrandHandoffKind::Captcha,
        "code" | "sms_code" => v1::ErrandHandoffKind::Code,
        "agreement" => v1::ErrandHandoffKind::Agreement,
        _ => return None,
    })
}

/// The card's heading, Claktory's words from the kind: the agent's own
/// go apart, in its body.
pub(crate) fn heading(kind: v1::ErrandHandoffKind) -> &'static str {
    match kind {
        v1::ErrandHandoffKind::Payment => "Your turn: a payment",
        v1::ErrandHandoffKind::Identity => "Your turn: prove it's you",
        v1::ErrandHandoffKind::Code => "Your turn: a code sent to you",
        v1::ErrandHandoffKind::Agreement => "Your turn: an agreement to accept",
        v1::ErrandHandoffKind::Captcha => "Your turn: a check that you're a person",
        v1::ErrandHandoffKind::SignIn => "Your turn: sign in",
        v1::ErrandHandoffKind::RecoveryCodes => "Your turn: recovery codes to save",
        v1::ErrandHandoffKind::TwoStep => "Your turn: two-step sign-in",
        _ => "Your turn in the browser",
    }
}

/// Text from a page or an agent, made one plain line: control and
/// invisible characters out, at most `max` characters.
pub(super) fn plain_line(text: &str, max: usize) -> String {
    let clean: String =
        text.chars().map(|c| if c.is_control() || factory_agent::text::hidden(c) { ' ' } else { c }).collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The most characters of a payment page's address.
pub(crate) const URL_MAX: usize = 2048;

/// The page's top-level address as Chrome reports it now, and its title:
/// what a payment or check request is about, or why it can't be one
/// (`doing`: what the person would be asked to do there).
async fn shown_page(factory: &Factory, agent: &str, doing: &str) -> Result<(String, String), String> {
    let seat = factory.errands.seat_of(agent).ok_or("open the page with browser_open first")?;
    let live = seat.live.lock().await;
    let page = live.as_ref().map(|live| live.page.clone()).ok_or("open the page with browser_open first")?;
    drop(live);
    let frame = page.frame().await.map_err(|error| crate::browser::schema::one_line(&error.to_string(), 200))?;
    if frame.url.starts_with("chrome-error:") {
        return Err(super::on_error_page(&format!("ask the person to {doing}")));
    }
    if !frame.url.starts_with("https://") || super::fence::check(&frame.url).is_err() {
        return Err(format!("the person is asked to {doing} only on an https page of a site on the internet: go to the page first"));
    }
    if frame.url.chars().count() > URL_MAX {
        return Err(format!("this page's address is longer than {URL_MAX} characters, too long to hand over"));
    }
    let title = page
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
        .await
        .ok()
        .and_then(|evaluated| evaluated["result"]["value"].as_str().map(|title| plain_line(title, 300)))
        .unwrap_or_default();
    Ok((frame.url, title))
}

/// `url`'s registrable domain, in ASCII ("pay.stripe.com" → "stripe.com");
/// its host when it has none.
pub(crate) fn domain(url: &url::Url) -> String {
    let host = url.host_str().unwrap_or_default();
    psl::domain_str(host).unwrap_or(host).to_owned()
}

/// browser_handoff: hands the person `kind` with the agent's `what`, in a
/// request, and pauses the agent's driving until they answer it.
pub(crate) async fn hand(factory: &Arc<Factory>, agent: &str, kind: &str, what: &str) -> Result<ToolResult, String> {
    let kind = kind_of(kind).ok_or(
        "kind is payment, check, code, agreement, recovery_codes or two_step. A passkey, a sign-in link sent by email or a request to prove who the person is can't be handed over: say in your reply that you couldn't get past it.",
    )?;
    let what = plain_line(what, WHAT_MAX + 1);
    if what.is_empty() || what.chars().count() > WHAT_MAX {
        return Err(format!("what is one sentence, at most {WHAT_MAX} characters, of what the person should do."));
    }
    match kind {
        v1::ErrandHandoffKind::Payment => hand_payment(factory, agent, what).await,
        v1::ErrandHandoffKind::Captcha => hand_check(factory, agent, what).await,
        _ => Err("kinds code and agreement need document and target: the field, or the box to tick.".to_owned()),
    }
}

/// A payment request: the page the agent's
/// browser shows now, as Chrome reports it, for the person to open in
/// their own browser. One at a time per agent; the agent's browser waits
/// until they answer.
async fn hand_payment(factory: &Arc<Factory>, agent: &str, what: String) -> Result<ToolResult, String> {
    let (person, name) = {
        let inner = factory.lock();
        let name = inner.config.agent(agent).map_or_else(|| agent.to_owned(), |found| found.name.clone());
        (inner.config.person().to_owned(), name)
    };
    let errand = factory.driving(agent).map_err(super::said)?;
    if factory.errands.seat_of(agent).is_some_and(|seat| seat.browser.lists_missing()) {
        return Err(super::said(("paused".to_owned(), super::LISTS_MISSING.to_owned())));
    }
    if payment_waiting(factory, agent) {
        return Err("A payment request of yours already waits for the person: wait for their answer.".to_owned());
    }
    let (address, title) = shown_page(factory, agent, "pay there").await.map_err(|why| super::said(("unsupported_target".to_owned(), why)))?;
    let parsed = url::Url::parse(&address).map_err(|_| super::said(("unsupported_target".to_owned(), "this page's address can't be read".to_owned())))?;
    let domain = domain(&parsed);
    let origin = parsed.origin().ascii_serialization();
    let card_title = format!("{name} asks you to pay on {domain}");
    let body = format!("\"{what}\"\n\nOpen the page on your device and pay there, then come back and say you've paid. {name} waits until you do.");
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::Payment as i32,
        what: what.clone(),
        origin: origin.clone(),
        title,
        errand: errand.id.clone(),
        url: address,
        domain: domain.clone(),
        ..Default::default()
    };
    let action = serde_json::to_string(&crate::card_actions::Action::Handoff { handoff: Box::new(handoff.clone()) })
        .map_err(|error| format!("Couldn't ask: {error}"))?;
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5)",
            params![agent, card_title, body, now_ms(), action],
        )
        .map_err(|error| format!("Couldn't ask: {error}"))?;
        db.last_insert_rowid()
    };
    let mut record = errand.clone();
    record.state = v1::ErrandState::PersonsTurn as i32;
    record.handoff = Some(handoff.clone().wire());
    factory.save_errand(record, None);
    log::write(
        &factory.store,
        &log::Entry { errand: errand.id.clone(), agent: agent.to_owned(), origin, action: format!("Asked you to pay on {domain}"), ..Default::default() },
    );
    let said = format!(
        "Asked {person} to pay on {domain}, in a request: they open this page in their own browser and pay there. Until they answer, your other browser tools wait. When they say they've paid you're told, with your page as it is then; if they can't, you're told why."
    );
    let details = json!({"ask": id.to_string(), "title": card_title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// A check request: the person gets past the check that the visitor is a
/// person on the page the agent's browser shows now, in a live view of that
/// tab alone, while the agent's browser tools wait. The browser stays
/// driven by Claktory, and no download starts meanwhile.
async fn hand_check(factory: &Arc<Factory>, agent: &str, what: String) -> Result<ToolResult, String> {
    let (person, name) = {
        let inner = factory.lock();
        let name = inner.config.agent(agent).map_or_else(|| agent.to_owned(), |found| found.name.clone());
        (inner.config.person().to_owned(), name)
    };
    let errand = factory.driving(agent).map_err(super::said)?;
    if waiting_of(factory, agent, v1::ErrandHandoffKind::Captcha) {
        return Err("A check request of yours already waits for the person: wait for their answer.".to_owned());
    }
    if !check_shown(factory, agent).await {
        return Err(super::said(("unsupported_target".to_owned(), "there's no check that you're a person on this page to hand over".to_owned())));
    }
    let (address, title) = shown_page(factory, agent, "hand it over").await.map_err(|why| super::said(("unsupported_target".to_owned(), why)))?;
    let parsed = url::Url::parse(&address).map_err(|_| super::said(("unsupported_target".to_owned(), "this page's address can't be read".to_owned())))?;
    let domain = crate::browser_host::tab_view::site_of(&address);
    let origin = parsed.origin().ascii_serialization();
    let card_title = format!("{name} needs you to get past a check on {domain}");
    let body = format!(
        "\"{what}\"\n\n{name} couldn't get past a check that it's a person on {domain}. Get past it in the live view of {name}'s browser, then tap Done, and {name} carries on from there. If you can't, tap Can't do this."
    );
    let handoff = HandoffRow {
        kind: v1::ErrandHandoffKind::Captcha as i32,
        what: what.clone(),
        origin: origin.clone(),
        title,
        errand: errand.id.clone(),
        domain: domain.clone(),
        ..Default::default()
    };
    let action = serde_json::to_string(&crate::card_actions::Action::Handoff { handoff: Box::new(handoff.clone()) })
        .map_err(|error| format!("Couldn't ask: {error}"))?;
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5)",
            params![agent, card_title, body, now_ms(), action],
        )
        .map_err(|error| format!("Couldn't ask: {error}"))?;
        db.last_insert_rowid()
    };
    let target = match factory.hold_check(agent, &errand, id, &domain).await {
        Ok(target) => target,
        Err(why) => {
            crate::extensions::ask::withdraw_for(factory, id, "The check's tab couldn't be handed over.");
            return Err(super::said(why));
        }
    };
    tokio::spawn(open_check_view(factory.clone(), agent.to_owned(), errand.id.clone(), id, target, domain.clone()));
    let mut record = errand.clone();
    record.handoff = Some(handoff.clone().wire());
    factory.save_errand(record, None);
    factory.downloads_while_checked(agent, true).await;
    log::write(
        &factory.store,
        &log::Entry { errand: errand.id.clone(), agent: agent.to_owned(), origin, action: format!("Asked you to get past a check on {domain}"), ..Default::default() },
    );
    let said = format!(
        "Asked {person} to get past the check on {domain}, in a live view of its tab. Meanwhile your browser tools work in a new tab of your own, anywhere but {domain}. When they answer, you're back on the check's tab and told how it went, with the page as it is then."
    );
    let details = json!({"ask": id.to_string(), "title": card_title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// The check request `ask`'s live view may open once its card is in the
/// conversation (its seq is the view's key): on tab `target`, of `site`.
/// If the tab leaves the site, the request is taken back and the agent
/// drives that tab again.
async fn open_check_view(factory: Arc<Factory>, agent: String, errand: String, ask: i64, target: String, site: String) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let seq = loop {
        if factory.held_check(&agent).map(|(held, _)| held) != Some(ask) {
            return;
        }
        if let Some(seq) = check_card(&factory, &agent, &errand) {
            break seq;
        }
        if std::time::Instant::now() >= until {
            tracing::warn!(%agent, "a check request's card didn't come: its live view can't open");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    };
    let (weak, runtime, whose) = (Arc::downgrade(&factory), tokio::runtime::Handle::current(), agent.clone());
    // Where it went is the page's say: the agent hears the site it left.
    let left = Box::new(move |_went: String| {
        if let Some(factory) = weak.upgrade() {
            runtime.spawn(check_left(factory, whose, ask));
        }
    });
    if factory.held_check_seq(&agent, ask, seq) {
        crate::browser_host::tab_view::check_opened(&factory, &agent, seq, &target, &site, left);
    }
}

/// The seq of `agent`'s check card for `errand` waiting in its
/// conversation, if it's there.
fn check_card(factory: &Factory, agent: &str, errand: &str) -> Option<u64> {
    let inner = factory.lock();
    let transcript = inner.agents.get(agent)?.transcript.as_ref()?;
    transcript.waiting_seqs().into_iter().rev().find(|seq| {
        matches!(
            transcript.entry(*seq).and_then(|entry| entry.kind.as_ref()),
            Some(v1::entry::Kind::AskPerson(asked)) if asked.handoff.as_ref().is_some_and(|shown| {
                shown.errand_id == errand && shown.kind == v1::ErrandHandoffKind::Captcha as i32
            })
        )
    })
}

/// The check's tab left its site (it let the person through, or went
/// elsewhere): the request is taken back and the agent drives that tab
/// again, told where it is.
pub(super) async fn check_left(factory: Arc<Factory>, agent: String, ask: i64) {
    if factory.held_check(&agent).map(|(held, _)| held) != Some(ask) {
        return;
    }
    let Some((_, site)) = factory.end_held_check(&agent).await else { return };
    crate::extensions::ask::withdraw_for(&factory, ask, &format!("The page left {site}."));
    let Some(errand) = factory.errands.open_of(&agent) else { return };
    let mut record = errand.clone();
    record.handoff = None;
    factory.save_errand(record, None);
    log::write(&factory.store, &log::Entry { errand: errand.id.clone(), agent: agent.clone(), action: format!("The check's tab left {site}"), ..Default::default() });
    let page = match factory.errand_step(&agent, super::Step::Read { limit: 4096 }).await {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with browser_read."),
    };
    let text = format!("The check's tab left {site}, so its request was taken back: you're on that tab again and your browser is yours. {page}");
    factory.deliver_event(&agent, text, host_event("Your browser is yours again".to_owned()));
}

/// Whether `agent`'s tab shows a check that the visitor is a person now,
/// as Claktory's reader sees it: never on the agent's or the page's say.
async fn check_shown(factory: &Factory, agent: &str) -> bool {
    let Some(seat) = factory.errands.seat_of(agent) else { return false };
    let page = seat.live.lock().await.as_ref().map(|live| live.page.clone());
    match page {
        Some(page) => page.check().await.is_some(),
        None => false,
    }
}

/// Whether a request of `kind` of `agent`'s waits for the person.
fn waiting_of(factory: &Factory, agent: &str, kind: v1::ErrandHandoffKind) -> bool {
    let pending: Vec<String> =
        crate::store::query(&factory.store.db(), "SELECT action FROM asks WHERE agent = ?1 AND state = 'pending' AND action != ''", [agent], |row| row.get(0))
            .unwrap_or_default();
    pending.iter().any(|action| {
        matches!(serde_json::from_str(action), Ok(crate::card_actions::Action::Handoff { handoff }) if handoff.kind == kind as i32)
    })
}

/// Whether a payment request of `agent`'s waits for the person.
fn payment_waiting(factory: &Factory, agent: &str) -> bool {
    waiting_of(factory, agent, v1::ErrandHandoffKind::Payment)
}

/// PaymentOpened: the person opened the payment request at `seq` of
/// `agent`'s on one of their devices. Every device then shows it opened.
pub(crate) fn opened(factory: &Arc<Factory>, request: &v1::PaymentOpened) -> Result<(), crate::factory::RequestError> {
    use crate::factory::RequestError;
    let gone = || RequestError::Ask("That payment request no longer waits: it was answered or withdrawn.".to_owned());
    let ask = {
        let inner = factory.lock();
        let entry = inner.agents.get(&request.agent_id).and_then(|runtime| runtime.transcript.as_ref()).and_then(|transcript| transcript.entry(request.seq));
        match entry.and_then(|entry| entry.kind.as_ref()) {
            Some(v1::entry::Kind::AskPerson(ask)) if ask.handoff.as_ref().is_some_and(|handoff| handoff.kind == v1::ErrandHandoffKind::Payment as i32) => {
                ask.id.parse::<i64>().map_err(|_| gone())?
            }
            _ => return Err(RequestError::Ask("That isn't a payment request.".to_owned())),
        }
    };
    let action: String = factory
        .store
        .db()
        .query_row("SELECT action FROM asks WHERE id = ?1 AND agent = ?2 AND state = 'pending'", params![ask, request.agent_id], |row| row.get(0))
        .map_err(|_| gone())?;
    let Ok(crate::card_actions::Action::Handoff { mut handoff }) = serde_json::from_str(&action) else { return Err(gone()) };
    if handoff.opened_ms != 0 {
        return Ok(());
    }
    handoff.opened_ms = now_ms();
    let action = serde_json::to_string(&crate::card_actions::Action::Handoff { handoff: handoff.clone() }).map_err(|error| RequestError::Command(error.to_string()))?;
    factory.store.db().execute("UPDATE asks SET action = ?2 WHERE id = ?1 AND state = 'pending'", params![ask, action])?;
    if let Some(mut errand) = factory.errands.get(&handoff.errand).filter(|errand| errand.handoff.is_some()) {
        if let Some(shown) = errand.handoff.as_mut() {
            shown.opened_ms = handoff.opened_ms;
        }
        factory.save_errand(errand, None);
    }
    log::write(
        &factory.store,
        &log::Entry { errand: handoff.errand.clone(), origin: handoff.origin.clone(), action: format!("You opened the payment page on {}", handoff.domain), ..Default::default() },
    );
    crate::extensions::ask::show(factory, &request.agent_id, ask, false);
    Ok(())
}

/// The handoff a card's details carry, for its entry.
pub(crate) fn in_details(details: &serde_json::Value) -> Option<v1::ErrandHandoff> {
    serde_json::from_value::<HandoffRow>(details.get("handoff")?.clone()).ok().map(HandoffRow::wire)
}

/// The person answered handoff card `ask` of `agent`'s: Done (`done`), or
/// Can't do this with `reason`.
pub(crate) async fn answered(factory: &Arc<Factory>, agent: &str, handoff: &HandoffRow, done: bool, reason: &str) {
    // What a recovery-codes or two-step page showed leaves Chrome's caches,
    // whatever the answer (Fugo 1642).
    if super::shows_secrets(handoff.kind) {
        factory.forget_cached(&handoff.errand, &handoff.origin).await;
    }
    // A code card fills its field from the person's answer, an agreement
    // card ticks its box (`super::code`).
    let acts = handoff.kind == v1::ErrandHandoffKind::Code as i32
        || (handoff.kind == v1::ErrandHandoffKind::Agreement as i32 && !handoff.agreement.is_empty());
    if done && acts {
        return Box::pin(super::code::answered(factory, agent, handoff, done, reason)).await;
    }
    // I saved them on a recovery-codes card (Keep in Secrets comes on its
    // own request).
    if done && handoff.kind == v1::ErrandHandoffKind::RecoveryCodes as i32 {
        return Box::pin(super::code::codes_saved(factory, agent, handoff, None)).await;
    }
    // Approved two-step sign-in: the errand may turn it on there.
    if done && handoff.kind == v1::ErrandHandoffKind::TwoStep as i32 {
        return Box::pin(super::code::two_step_approved(factory, agent, handoff)).await;
    }
    let person = factory.lock().config.person_capitalized().to_owned();
    let Some(errand) = factory.errands.get(&handoff.errand) else { return };
    let kind = v1::ErrandHandoffKind::try_from(handoff.kind).unwrap_or_default();
    let paid = kind == v1::ErrandHandoffKind::Payment;
    let checked = kind == v1::ErrandHandoffKind::Captcha;
    // A check holds only its own tab: the errand ran on meanwhile.
    let held = if checked { factory.end_held_check(agent).await.is_some() } else { State::from_wire(errand.state) == State::PersonsTurn };
    if !held || !State::from_wire(errand.state).open() {
        return;
    }
    let mut record = errand.clone();
    record.state = v1::ErrandState::Running as i32;
    record.handoff = None;
    factory.save_errand(record, None);
    let why = plain_line(reason, 500);
    let logged = match (done, paid, checked) {
        (true, true, _) => format!("You paid on {}", handoff.domain),
        (true, _, true) => format!("You got past a check on {}", handoff.domain),
        (true, false, false) => "You finished your turn".to_owned(),
        (false, _, _) => "You couldn't do it".to_owned(),
    };
    log::write(&factory.store, &log::Entry { errand: errand.id.clone(), origin: handoff.origin.clone(), action: logged, outcome: if done { String::new() } else { why.clone() }, ..Default::default() });
    // Requests made before browser-simple handed the person the browser
    // itself: it's driven again. A payment never left the agent's hands.
    let resumed = if paid || checked { Ok(()) } else { factory.errands.seat_of(agent).map_or(Ok(()), |seat| seat.browser.resume()) };
    let now = match resumed {
        Ok(()) => factory.errand_step(agent, super::Step::Read { limit: 4096 }).await,
        Err(why) => Err(why),
    };
    let page = match now {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with browser_read."),
    };
    let text = match (done, paid) {
        (true, true) => format!("{person} says they've paid on {}: check the page before you go on. {page}", handoff.domain),
        (true, false) if checked => format!("{person} got past the check on {}: your browser is yours again. {page}", handoff.domain),
        (true, false) => format!("{person} finished your handoff: {} {page}", sentence(&handoff.what)),
        (false, _) => {
            let because = if why.is_empty() { String::new() } else { format!(": \"{why}\"") };
            format!(
                "{person} couldn't do your handoff ({}){because}. Your browser is yours again: carry on without it if you can, or say in your reply what's left. {page}",
                heading(kind).to_lowercase()
            )
        }
    };
    factory.deliver_event(agent, text, host_event("Your browser is yours again".to_owned()));
}

/// `what` as a sentence: ended with a period unless it already ends a
/// sentence.
fn sentence(what: &str) -> String {
    let what = what.trim_end();
    if what.ends_with(['.', '!', '?', '…']) { what.to_owned() } else { format!("{what}.") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_ends_with_one_period() {
        assert_eq!(sentence("Type the password and press Login."), "Type the password and press Login.");
        assert_eq!(sentence("Sign in"), "Sign in.");
        assert_eq!(sentence("Done? "), "Done?");
    }

    #[test]
    fn kinds_are_the_listed_words() {
        assert_eq!(kind_of("sms_code"), Some(v1::ErrandHandoffKind::Code));
        assert_eq!(kind_of("payment"), Some(v1::ErrandHandoffKind::Payment));
        assert_eq!(kind_of("Payment"), None);
        assert_eq!(kind_of("check"), Some(v1::ErrandHandoffKind::Captcha));
        for gone in ["sign_in", "captcha", "identity", "other"] {
            assert_eq!(kind_of(gone), None, "{gone}");
        }
        assert!(heading(v1::ErrandHandoffKind::Payment).starts_with("Your turn"));
    }

    #[test]
    fn a_payment_names_its_registrable_domain() {
        let domain = |address: &str| domain(&url::Url::parse(address).unwrap());
        assert_eq!(domain("https://pay.stripe.com/c/1"), "stripe.com");
        assert_eq!(domain("https://checkout.shop.co.uk/"), "shop.co.uk");
        assert_eq!(domain("https://x.github.io/pay"), "x.github.io");
    }

    #[test]
    fn page_and_agent_text_is_one_plain_line() {
        assert_eq!(plain_line("Pay\u{202e} now\n\nplease", 300), "Pay now please");
        assert_eq!(plain_line(&"a".repeat(400), 300).len(), 300);
    }
}
