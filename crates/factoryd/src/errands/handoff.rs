//! Handoffs (agent-browser.md §4): what only the person may do on a page
//! (pay, prove who they are, type a code, accept an agreement, solve a
//! captcha, sign in), handed to them on a card through Ask: Show me opens
//! the browser's live view, Done gives the browser back to the agent with
//! the page as it is then, Can't do this ends the errand with their reason.
//! While the card waits the agent can't read or act.

use std::sync::Arc;

use factory_agent::ToolResult;
use factory_protocol::v1;
use rusqlite::params;
use serde_json::json;

use super::{HandoffRow, State, log};
use crate::factory::{Factory, host_event};
use crate::transcript::now_ms;

/// The tool that hands the person a moment on a card.
pub(crate) const TOOL: &str = "errand_handoff";

/// The tools whose result can be an errand's card: errand_handoff, the
/// steps that find a check that the visitor is a person, and a fill that
/// is the browser's first sign-in at an identity provider
/// (`super::code::signed_in`).
pub(crate) const TOOLS: [&str; 6] = [TOOL, "errand_read", "errand_act", "errand_navigate", "browser_download", super::fill::TOOL];

/// The most characters of what the agent says the person should do.
pub(crate) const WHAT_MAX: usize = 300;

/// A kind, from the agent's word.
pub(crate) fn kind_of(word: &str) -> Option<v1::ErrandHandoffKind> {
    Some(match word {
        "payment" => v1::ErrandHandoffKind::Payment,
        "identity" => v1::ErrandHandoffKind::Identity,
        "code" | "sms_code" => v1::ErrandHandoffKind::Code,
        "agreement" => v1::ErrandHandoffKind::Agreement,
        "captcha" => v1::ErrandHandoffKind::Captcha,
        "sign_in" => v1::ErrandHandoffKind::SignIn,
        "other" => v1::ErrandHandoffKind::Other,
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

/// What the log says was handed over.
fn logged(kind: v1::ErrandHandoffKind) -> &'static str {
    match kind {
        v1::ErrandHandoffKind::Payment => "Handed to you: a payment",
        v1::ErrandHandoffKind::Identity => "Handed to you: an identity check",
        v1::ErrandHandoffKind::Code => "Handed to you: a code",
        v1::ErrandHandoffKind::Agreement => "Handed to you: an agreement",
        v1::ErrandHandoffKind::Captcha => "Handed to you: a captcha",
        v1::ErrandHandoffKind::SignIn => "Handed to you: signing in",
        _ => "Handed to you",
    }
}

/// Text from a page or an agent, made one plain line: control and
/// invisible characters out, at most `max` characters.
pub(super) fn plain_line(text: &str, max: usize) -> String {
    let clean: String =
        text.chars().map(|c| if c.is_control() || factory_agent::text::hidden(c) { ' ' } else { c }).collect();
    clean.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

/// The page the errand is on now: its origin, title and address, as
/// Claktory reads them; empty when it can't.
async fn where_now(factory: &Factory) -> (String, String, String) {
    let live = factory.errands.live.lock().await;
    let Some(page) = live.as_ref().map(|live| live.page.clone()) else { return Default::default() };
    drop(live);
    let address = page.frame().await.ok().and_then(|frame| url::Url::parse(&frame.url).ok());
    let origin = address.as_ref().map(|url| url.origin().ascii_serialization()).unwrap_or_default();
    let address = address.filter(|url| url.scheme() == "https").map(String::from).unwrap_or_default();
    let origin = if origin == "null" { String::new() } else { origin };
    let title = page
        .call("Runtime.evaluate", json!({"expression": "document.title", "returnByValue": true}))
        .await
        .ok()
        .and_then(|evaluated| evaluated["result"]["value"].as_str().map(|title| plain_line(title, 300)))
        .unwrap_or_default();
    (origin, title, address)
}

/// errand_handoff: hands the person `kind` with the agent's `what`, on a
/// card, and pauses the agent's driving until they answer it.
pub(crate) async fn hand(factory: &Arc<Factory>, agent: &str, kind: &str, what: &str) -> Result<ToolResult, String> {
    let kind = kind_of(kind).ok_or("kind is payment, identity, code, agreement, captcha, sign_in or other.")?;
    let what = plain_line(what, WHAT_MAX + 1);
    if what.is_empty() || what.chars().count() > WHAT_MAX {
        return Err(format!("what is one sentence, at most {WHAT_MAX} characters, of what the person should do."));
    }
    hand_over(factory, agent, kind, what).await
}

/// Hands the person a check that they're a person, which the page put up
/// and which didn't clear by itself: Claktory's own handoff, in its words.
/// What the agent is told, and the card's details for its entry.
pub(crate) async fn hand_check(factory: &Arc<Factory>, agent: &str, origin: &str) -> (String, serde_json::Value) {
    let person = factory.lock().config.person().to_owned();
    let site = if origin.is_empty() { "the page".to_owned() } else { origin.to_owned() };
    let what = format!("Pass the check on {site} that asks whether you're a person.");
    match hand_over(factory, agent, v1::ErrandHandoffKind::Captcha, what).await {
        Ok(result) => (
            format!(
                "{site} put up a check that you're a person, which only {person} may pass, so Claktory handed it to them on a card (\"{}\"). The browser is theirs until they answer: you're told when they're done, with the page as it is then.",
                heading(v1::ErrandHandoffKind::Captcha)
            ),
            result.details,
        ),
        Err(why) => (
            format!(
                "{site} put up a check that you're a person, and handing it to {person} didn't work ({why}): hand it over with errand_handoff, kind \"captcha\"."
            ),
            serde_json::Value::Null,
        ),
    }
}

async fn hand_over(factory: &Arc<Factory>, agent: &str, kind: v1::ErrandHandoffKind, what: String) -> Result<ToolResult, String> {
    let person = factory.lock().config.person().to_owned();
    let errand = factory.driving(agent).map_err(super::said)?;
    let (origin, title, address) = where_now(factory).await;
    // The browser is the person's from now: a sign-in or a check that
    // they're a person runs Chrome with no automation, on this page
    // (Bruno 37121, 37122).
    if let Some(browser) = factory.errands.browser() {
        let undriven = matches!(kind, v1::ErrandHandoffKind::SignIn | v1::ErrandHandoffKind::Captcha);
        let at = Some(address.as_str()).filter(|address| undriven && !address.is_empty());
        browser.handoff(undriven, at).map_err(|why| format!("Couldn't hand it over: {why}"))?;
    }
    let title_line = if title.is_empty() { String::new() } else { format!(" (\"{title}\")") };
    let on = if origin.is_empty() { String::new() } else { format!(" The page is on {origin}{title_line}.") };
    let card_title = heading(kind);
    let body = format!(
        "{what}\n\nErrand: {}.{on} Show me opens the browser; press Done when you've finished, or Can't do this to end the errand.",
        errand.purpose
    );
    let handoff = HandoffRow { kind: kind as i32, what: what.clone(), origin: origin.clone(), title, seq: 0, errand: errand.id.clone(), numeric: false, agreement: String::new() };
    let action = serde_json::to_string(&crate::card_actions::Action::Handoff { handoff: Box::new(handoff.clone()) })
        .map_err(|error| format!("Couldn't hand it over: {error}"))?;
    let id = {
        let db = factory.store.db();
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, action) VALUES (?1, ?2, ?3, 0, ?4, 'pending', '', ?5)",
            params![agent, card_title, body, now_ms(), action],
        )
        .map_err(|error| format!("Couldn't hand it over: {error}"))?;
        db.last_insert_rowid()
    };
    let mut record = errand.clone();
    record.state = v1::ErrandState::PersonsTurn as i32;
    record.handoff = Some(handoff.clone().wire());
    factory.save_errand(record, None);
    log::write(
        &factory.store,
        &log::Entry { errand: errand.id.clone(), agent: agent.to_owned(), origin, action: logged(kind).to_owned(), ..Default::default() },
    );
    let said = format!(
        "Handed to {person} on a card: \"{card_title}\". Until they answer it, the browser is theirs: errand_read, errand_act and errand_navigate wait. When they press Done you're told what the page is then; if they can't do it, you're told why and the errand ends."
    );
    let details = json!({"ask": id.to_string(), "title": card_title, "body": body, "wants_text": false, "handoff": handoff});
    Ok(ToolResult::text(said, details))
}

/// The handoff a card's details carry, for its entry.
pub(crate) fn in_details(details: &serde_json::Value) -> Option<v1::ErrandHandoff> {
    serde_json::from_value::<HandoffRow>(details.get("handoff")?.clone()).ok().map(HandoffRow::wire)
}

/// The person answered handoff card `ask` of `agent`'s: Done (`done`), or
/// Can't do this with `reason`.
pub(crate) async fn answered(factory: &Arc<Factory>, agent: &str, handoff: &HandoffRow, done: bool, reason: &str) {
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
    if State::from_wire(errand.state) != State::PersonsTurn {
        return;
    }
    if !done {
        let why = plain_line(reason, 500);
        factory.end_errand(&errand.id, State::Cancelled, &why).await;
        log::write(
            &factory.store,
            &log::Entry { errand: errand.id.clone(), action: "You couldn't do it".to_owned(), ..Default::default() },
        );
        let because = if why.is_empty() { String::new() } else { format!(": \"{why}\"") };
        let text = format!(
            "{person} couldn't do your handoff ({}){because}. Errand {} is over: tell your lead, or whoever opened it, what was done and what wasn't.",
            heading(v1::ErrandHandoffKind::try_from(handoff.kind).unwrap_or_default()).to_lowercase(),
            errand.id
        );
        factory.deliver_event(agent, text, host_event(format!("Errand {} cancelled", errand.id)));
        return;
    }
    let mut record = errand.clone();
    record.state = v1::ErrandState::Running as i32;
    record.handoff = None;
    factory.save_errand(record, None);
    log::write(&factory.store, &log::Entry { errand: errand.id.clone(), action: "You finished your turn".to_owned(), ..Default::default() });
    let resumed = factory.errands.browser().map_or(Ok(()), |browser| browser.resume());
    // The browser may have restarted for the person's turn (sign-in runs
    // without automation): its old connection is closed then, and the read
    // connects again, on whatever page it's on now. A check still there
    // isn't handed over again from here: no tool result would post its
    // card (Ghiaccio 40233).
    let now = match resumed {
        Ok(()) => factory.errand_step_handing(agent, super::Step::Read { limit: 4096 }, false).await,
        Err(why) => Err(why),
    };
    let page = match now {
        Ok((page, _)) => format!("The page is now:\n{page}"),
        Err(why) => format!("Reading the page didn't work ({why}): read it with errand_read."),
    };
    let text = format!("{person} finished your handoff: {} {page}", sentence(&handoff.what));
    factory.deliver_event(agent, text, host_event(format!("Errand {}: your turn again", errand.id)));
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
        assert!(heading(v1::ErrandHandoffKind::Payment).starts_with("Your turn"));
    }

    #[test]
    fn page_and_agent_text_is_one_plain_line() {
        assert_eq!(plain_line("Pay\u{202e} now\n\nplease", 300), "Pay now please");
        assert_eq!(plain_line(&"a".repeat(400), 300).len(), 300);
    }
}
