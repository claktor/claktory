//! Browsing sessions end to end: an agent with the Browser extension opens
//! its own session in a real headless Chrome, reads and acts on the page,
//! hands the person a payment on a card, and closes it; the fence keeps it
//! to https sites on the internet and away from password managers; a code
//! the person types on a card goes into the page, never to the agent; the
//! log keeps no payload; turning the browser off ends the session. Skipped
//! where there's no Chrome.
//!
//! A file of its own: the browser becomes part of a Claktory once it
//! exists, process-wide, and the shared test binary counts the extensions
//! it lists.
//!
//! Claktory's browser isn't on Windows.
#![cfg(unix)]

#[path = "common/mod.rs"]
mod common;

use common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ErrandState, client_message, entry::Kind, server_message::Body};
use serde_json::json;
use std::time::Duration;

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

async fn select(client: &mut Client, view: &mut View, agent: &str) {
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() });
    apply_request(client, view, select).await;
}

/// `agent` (selected) calls `tool` with `args`: what it said, and the card it shows.
async fn call(
    client: &mut Client,
    view: &mut View,
    agent: &str,
    tool: &str,
    args: serde_json::Value,
) -> (String, Option<(u64, v1::AskPerson)>) {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    let new = &view.entries[before..];
    let output = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let card = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
        _ => None,
    });
    (output.expect("a tool call"), card)
}

/// The newest browsing session.
fn session(view: &View) -> v1::Errand {
    view.workspace.errands.first().cloned().expect("a session")
}

/// The element named `name` in a read's fenced page JSON: its ref.
fn reference(said: &str, name: &str) -> (String, String) {
    let json = said.lines().find(|line| line.starts_with('{')).unwrap_or_else(|| panic!("a page in {said}"));
    let page: serde_json::Value = serde_json::from_str(json).expect("the page's JSON");
    let element = page["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["name"].as_str().unwrap_or_default().contains(name))
        .unwrap_or_else(|| panic!("{name} in {page}"));
    (page["document"].as_str().unwrap().to_owned(), element["ref"].as_str().unwrap().to_owned())
}

/// The file input in a read's fenced page JSON: its document and ref.
fn file_input(said: &str) -> (String, String) {
    let json = said.lines().find(|line| line.starts_with('{')).unwrap_or_else(|| panic!("a page in {said}"));
    let page: serde_json::Value = serde_json::from_str(json).expect("the page's JSON");
    let input = page["elements"]
        .as_array()
        .unwrap()
        .iter()
        .find(|element| element["sensitive"] == "file")
        .unwrap_or_else(|| panic!("a file input in {page}"));
    (page["document"].as_str().unwrap().to_owned(), input["ref"].as_str().unwrap().to_owned())
}

/// Whether the card at `seq` was answered.
fn answered(view: &View, seq: u64) -> bool {
    view.entries
        .iter()
        .any(|entry| entry.seq == seq && matches!(&entry.kind, Some(Kind::AskPerson(ask)) if ask.state() == v1::AskPersonState::Answered))
}

/// The events `agent`'s conversation got whose text holds `text`.
fn told(view: &View, text: &str) -> bool {
    view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains(text)))
}

/// A project with Lead and Runner, Runner with the Browser extension on.
async fn runner_with_browser(client: &mut Client, view: &mut View, dir: &std::path::Path) {
    apply_request(client, view, create_project("Site", &dir.join("site"))).await;
    apply_request(client, view, create_agent("site", "Lead", "")).await;
    apply_request(client, view, create_agent("site", "Runner", "")).await;
    let reply = apply_request(client, view, browser("runner", true)).await;
    assert!(!matches!(reply, Body::Error(_)), "{reply:?}");
}

/// The person turns `agent`'s Browser extension on or off, in Agent settings.
fn browser(agent: &str, on: bool) -> client_message::Body {
    let change = v1::ExtensionChange { id: "browser".into(), enabled: Some(on), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), extension: Some(change), ..Default::default() })
}

/// Whether `agent`'s Browser extension is on, as the app shows it.
fn browsing(view: &View, agent: &str) -> bool {
    view.agent(agent).extensions.iter().any(|extension| extension.id == "browser" && extension.enabled)
}

/// What `agent` read from outside, as Claktory marks it.
fn read_outside(dir: &std::path::Path, agent: &str) -> Option<String> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    db.query_row("SELECT what FROM errand_outside_read WHERE agent = ?1", [agent], |row| row.get(0)).ok()
}

/// Needs you's History, newest first.
async fn history(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    let list = client_message::Body::ListNeedsYouHistory(v1::ListNeedsYouHistory::default());
    match apply_request(client, view, list).await {
        Body::NeedsYouHistory(history) => history.records,
        other => panic!("History: {other:?}"),
    }
}

/// A sign-up form with no agreement on it.
const SIGN_UP: &str = r#"<!doctype html><title>Sign up</title>
    <h1>Organization</h1><p>Fill this in.</p>
    <label>Name <input id=name oninput="document.title = 'named ' + this.value.length"></label>
    <button id=next onclick="document.querySelector('h1').textContent = 'Thanks'">Next</button>"#;

/// Story 1, posting: an agent with Browser on opens a page itself, reads
/// it, acts on it and closes its browser. Story 2, a routine: on a later
/// turn it opens it again, with no one's approval. Reading a page marks it
/// as having read outside content, as web_fetch does, without pipes' taint.
#[tokio::test]
async fn an_agent_opens_reads_acts_and_closes_its_own_browser_and_opens_it_again_later() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://forms.example.org/signup", Some(SIGN_UP));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    select(&mut client, &mut view, "runner").await;
    assert!(view.workspace.errands.is_empty());

    let (said, card) = call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    assert!(card.is_none() && said.contains("/signup"), "{said}");
    let first = session(&view);
    assert_eq!((first.agent_id.as_str(), first.state()), ("runner", ErrandState::Running));
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Organization") && said.contains("a stranger's words"), "{said}");
    assert!(read_outside(&dir, "runner").is_some(), "reading a page marks it");
    assert!(view.agent("runner").tainted_by_pipes.is_empty(), "not pipes' taint: its secrets don't start asking");
    let (document, name) = reference(&said, "Name");
    let (_, next) = reference(&said, "Next");
    let fill = json!({"op": "fill", "document": document, "target": name, "text": "Copium Labs"});
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", fill).await;
    assert!(said.contains("\"took\":true"), "{said}");
    let (said, _) =
        call(&mut client, &mut view, "runner", "browser_act", json!({"op": "click", "document": document, "target": next})).await;
    assert!(said.contains("\"done\":true"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Thanks") && said.contains("named 11"), "{said}");

    let (said, _) = call(&mut client, &mut view, "runner", "browser_close", json!({})).await;
    assert_eq!(said, "Closed your browser. Your sign-ins stay for next time.");
    assert_eq!(session(&view).state(), ErrandState::Done);
    let (said, _) = call(&mut client, &mut view, "runner", "browser_close", json!({})).await;
    assert_eq!(said, "Your browser wasn't open.");

    // A later turn opens it again by itself: no request, no approval. The
    // test's Chrome takes one connection where the browser's host takes
    // any number, so a fresh one stands in.
    assert!(factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile-again")), "a browser again");
    let (said, card) = call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    assert!(card.is_none() && said.contains("/signup"), "{said}");
    assert_ne!(session(&view).id, first.id);
    assert_eq!(session(&view).state(), ErrandState::Running);
}

/// A payment page, as a checkout shows it.
const PAY: &str = r#"<!doctype html><title>Pay Acme</title><h1>Pay $25.00</h1><button>Pay</button>"#;

/// Story 3: a payment goes to the person as a request to open the page in
/// their own browser; one at a time; opening it shows on every device;
/// I've paid tells the agent to check the page, Can't do this tells it
/// why, and either way the browser stays its own.
#[tokio::test]
async fn a_payment_is_the_persons_on_the_page_they_open_in_their_own_browser() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let checkout = "https://pay.stripe.com/c/pay/cs_test_a1?locale=en";
    factoryd::errands::serve_for_test(checkout, Some(PAY));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": checkout})).await;

    let pay = json!({"kind": "payment", "what": "Pay Acme's $25 sign-up fee."});
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", pay.clone()).await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert!(said.starts_with("Asked the person to pay on stripe.com, in a request"), "{said}");
    assert_eq!(card.title, "Runner asks you to pay on stripe.com");
    let handoff = card.handoff.clone().expect("its handoff");
    assert_eq!(
        (handoff.kind(), handoff.url.as_str(), handoff.domain.as_str(), handoff.opened_ms),
        (v1::ErrandHandoffKind::Payment, checkout, "stripe.com", 0)
    );
    assert_eq!(handoff.what, "Pay Acme's $25 sign-up fee.");
    assert_eq!(session(&view).state(), ErrandState::PersonsTurn);
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", pay.clone()).await;
    assert!(card.is_none() && said.contains("until they answer your handoff request"), "{said}");

    // Opened on one device: every device shows it opened.
    let opened = Request::PaymentOpened(v1::PaymentOpened { agent_id: "runner".into(), seq });
    assert!(matches!(apply_request(&mut client, &mut view, opened).await, Body::Ack(_)));
    let card_opened = |view: &View| {
        view.entries.iter().any(|entry| {
            entry.seq == seq
                && matches!(&entry.kind, Some(Kind::AskPerson(ask)) if ask.handoff.as_ref().is_some_and(|handoff| handoff.opened_ms > 0))
        })
    };
    let session_opened = |view: &View| session(view).handoff.is_some_and(|handoff| handoff.opened_ms > 0);
    view.follow(&mut client, |view| card_opened(view) && session_opened(view)).await;

    // I've paid: the agent checks the page; its browser is its own again.
    let paid = Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, ..Default::default() });
    assert!(!matches!(apply_request(&mut client, &mut view, paid).await, Body::Error(_)));
    view.follow(&mut client, |view| told(view, "The person says they've paid on stripe.com: check the page") && idle(view, "runner")).await;
    assert_eq!(session(&view).state(), ErrandState::Running);

    // Can't do this, with why: the agent is told, and keeps its browser.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", pay).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    let no = Request::AnswerAsk(v1::AnswerAsk {
        agent_id: "runner".into(),
        seq,
        dismiss: true,
        text: "The card was declined".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, no).await;
    view.follow(&mut client, |view| told(view, "couldn't do your handoff") && idle(view, "runner")).await;
    assert!(told(&view, "The card was declined") && told(&view, "Your browser is yours again"));
    assert_eq!(session(&view).state(), ErrandState::Running);
}

/// Pages for the fence: one that sends itself to a password manager.
const AWAY: &str = r#"<!doctype html><title>Moving</title><p>One moment.</p>
    <script>setTimeout(() => { location.href = 'https://vault.bitwarden.com/#/vault' }, 50)</script>"#;

/// The fence: an agent's browser goes to https sites on the internet, never
/// another scheme, this computer or its network (by name, by address or by
/// where the name resolves), nor a web password manager, however it gets
/// there.
#[tokio::test]
async fn the_browser_goes_only_to_https_sites_on_the_internet_and_never_to_a_password_manager() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    for allowed in [
        "https://www.google.com/",
        "https://bitwarden.com/",
        "https://proton.me/pass",
        "https://1password.com/",
        "https://www.example.org/away",
    ] {
        factoryd::errands::serve_for_test(
            allowed,
            Some(if allowed.ends_with("/away") { AWAY } else { "<title>Fine</title><p>A page.</p>" }),
        );
    }
    factoryd::errands::serve_for_test(
        "https://www.example.org/log-in",
        Some(r#"<title>Bitwarden</title><a href="https://vault.bitwarden.com/#/login">Log In</a>"#),
    );
    factoryd::errands::resolve_for_test("intranet.example.org", std::net::IpAddr::from([10, 0, 0, 5]));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    select(&mut client, &mut view, "runner").await;
    let open =
        async |client: &mut Client, view: &mut View, url: &str| call(client, view, "runner", "browser_open", json!({"url": url})).await.0;

    for (url, why) in [
        ("http://www.example.org/", "http: addresses aren't for Claktory's browser"),
        ("file:///etc/passwd", "file: addresses aren't for Claktory's browser"),
        ("chrome://settings/", "chrome: addresses aren't for Claktory's browser"),
        ("data:text/html,hello", "data: addresses aren't for Claktory's browser"),
        ("javascript:alert(1)", "javascript: addresses aren't for Claktory's browser"),
        ("https://localhost/", "is a name on this computer or its network"),
        ("https://127.0.0.1/", "That's an IP address"),
        ("https://10.1.2.3/", "That's an IP address"),
        ("https://[::1]/", "That's an IP address"),
        ("https://intranet.example.org/", "points to 10.0.0.5, an address on this computer or its network"),
    ] {
        let said = open(&mut client, &mut view, url).await;
        assert!(said.contains(why), "{url}: {said}");
    }
    for url in [
        "https://passwords.google.com/",
        "https://my.1password.com/",
        "https://vault.bitwarden.com/",
        "https://lastpass.com/pricing",
        "https://lastpass.com/%76ault",
        "https://keepersecurity.com/vault",
        "https://app.dashlane.com/",
        "https://pass.proton.me/",
        "https://app.nordpass.com/",
    ] {
        let said = open(&mut client, &mut view, url).await;
        assert!(said.contains("Claktory's browser doesn't open password managers."), "{url}: {said}");
    }
    for url in ["https://www.google.com/", "https://bitwarden.com/", "https://proton.me/pass", "https://1password.com/"] {
        let said = open(&mut client, &mut view, url).await;
        assert!(!said.contains("doesn't open") && !said.contains("aren't for"), "{url}: {said}");
        let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
        assert!(said.contains("A page."), "{url}: {said}");
    }

    // A page that sends itself there is kept from it, and the agent told.
    open(&mut client, &mut view, "https://www.example.org/away").await;
    let mut said = String::new();
    for _ in 0..20 {
        said = call(&mut client, &mut view, "runner", "browser_read", json!({})).await.0;
        if said.contains("Claktory kept the page from going to") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert!(said.contains("Claktory kept the page from going to https://vault.bitwarden.com"), "{said}");

    // A click that would go there is told so in its own answer, at once.
    open(&mut client, &mut view, "https://www.example.org/log-in").await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, log_in) = reference(&said, "Log In");
    let started = std::time::Instant::now();
    let (said, _) =
        call(&mut client, &mut view, "runner", "browser_act", json!({"op": "click", "document": document, "target": log_in})).await;
    assert!(said.contains("Claktory's browser doesn't open password managers."), "{said}");
    assert!(started.elapsed() < Duration::from_secs(15), "{:?}", started.elapsed());
}

/// A session its agent leaves unused ends by itself; using it keeps it
/// open, and browser_open opens another.
#[tokio::test]
async fn a_browser_left_unused_closes_by_itself() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://forms.example.org/signup", Some(SIGN_UP));
    factoryd::errands::unused_after_for_test(&factory, Duration::from_secs(4));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    select(&mut client, &mut view, "runner").await;
    let open = json!({"url": "https://forms.example.org/signup"});
    call(&mut client, &mut view, "runner", "browser_open", open.clone()).await;
    let first = session(&view).id;
    for _ in 0..3 {
        tokio::time::sleep(Duration::from_secs(2)).await;
        call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    }
    assert_eq!(session(&view).state(), ErrandState::Running, "used within the time");
    view.follow(&mut client, |view| session(view).state() != ErrandState::Running).await;
    assert_eq!(session(&view).end_reason, "Its browser closed after 10 minutes unused.");
    assert!(factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile-again")), "a browser again");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_open", open).await;
    assert!(said.contains("/signup"), "{said}");
    assert!(session(&view).id != first && session(&view).state() == ErrandState::Running);
}

/// Safe Browsing: on a page Chrome warned about (its error page in the top
/// frame), the agent can't act, sign in, upload or ask for a payment; it
/// can go elsewhere. Nor, before a new profile has Safe Browsing's lists,
/// sign in or ask for a payment anywhere.
#[tokio::test]
async fn on_a_page_chrome_warned_about_the_agent_only_goes_elsewhere() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_with_turns_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://forms.example.org/signup", Some(SIGN_UP));
    factoryd::errands::serve_for_test("https://unsafe.example.org/", None);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    std::fs::create_dir_all(dir.join("site")).expect("its folder");
    std::fs::write(dir.join("site/logo.png"), b"\x89PNG").expect("a file");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, name) = reference(&said, "Name");

    // A new profile still without Safe Browsing's lists: it browses, but
    // signing in and payments wait for them.
    factoryd::errands::lists_missing_for_test(&factory, true);
    let waits = "still getting Safe Browsing's lists for this new profile";
    let (said, card) = call(&mut client, &mut view, "runner", "browser_sign_in", json!({"document": document, "password": name})).await;
    assert!(card.is_none() && said.contains(waits), "{said}");
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", json!({"kind": "payment", "what": "Pay the fee."})).await;
    assert!(card.is_none() && said.contains(waits), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Organization"), "{said}");
    factoryd::errands::lists_missing_for_test(&factory, false);

    let (said, _) = call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://unsafe.example.org/"})).await;
    assert!(said.contains("unsafe_site: Chrome warns that this site is dangerous"), "Safe Browsing's, not the fence's: {said}");
    let refused = [
        ("browser_act", json!({"op": "fill", "document": document, "target": name, "text": "hello"})),
        ("browser_sign_in", json!({"document": document, "password": name})),
        ("browser_upload", json!({"document": document, "target": name, "path": "logo.png"})),
        ("browser_handoff", json!({"kind": "payment", "what": "Pay the fee."})),
    ];
    for (tool, args) in refused {
        let (said, card) = call(&mut client, &mut view, "runner", tool, args).await;
        assert!(card.is_none() && said.contains("this tab shows Chrome's error page") && said.contains("so Claktory won't"), "{tool}: {said}");
    }
    let (said, _) = call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    assert!(said.contains("/signup"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Organization"), "{said}");
}

/// A form with a file field.
const ATTACH: &str = r#"<!doctype html><title>Attach</title>
    <label>Logo <input type=file id=logo onchange="document.title = 'chose ' + this.files[0].name"></label>
    <label>Caption <input id=caption></label>"#;

/// browser_upload attaches a file from the agent's own folder or its
/// uploads folder to a file field, and nothing else.
#[tokio::test]
async fn an_agent_attaches_only_a_file_of_its_own_to_a_file_field() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://forms.example.org/attach", Some(ATTACH));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    let site = dir.join("site");
    std::fs::create_dir_all(site.join("folder")).expect("folders");
    std::fs::write(site.join("logo.png"), b"\x89PNG logo").expect("a file");
    let uploads = dir.join("home/agents/runner/uploads/2026-11-27");
    std::fs::create_dir_all(&uploads).expect("uploads");
    std::fs::write(uploads.join("photo.jpg"), b"jpeg").expect("an attachment");
    let theirs = dir.join("home/agents/lead");
    std::fs::create_dir_all(&theirs).expect("another agent's folder");
    std::fs::write(theirs.join("notes.txt"), b"private").expect("its file");
    std::os::unix::fs::symlink("/etc/hosts", site.join("hosts")).expect("a link out");
    let big = std::fs::File::create(site.join("big.bin")).expect("a big file");
    big.set_len(101 * 1024 * 1024).expect("its size");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/attach"})).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, logo) = file_input(&said);
    let (_, caption) = reference(&said, "Caption");
    let upload = |path: &str, target: &str| json!({"document": document, "target": target, "path": path});

    let mine = "isn't a file in your folder or your uploads folder";
    for path in [
        "/etc/passwd".to_owned(),
        theirs.join("notes.txt").display().to_string(),
        "hosts".to_owned(),
        "folder".to_owned(),
        "../outside.txt".to_owned(),
    ] {
        let (said, _) = call(&mut client, &mut view, "runner", "browser_upload", upload(&path, &logo)).await;
        assert!(said.contains(mine), "{path}: {said}");
    }
    let (said, _) = call(&mut client, &mut view, "runner", "browser_upload", upload("big.bin", &logo)).await;
    assert!(said.contains("browser_upload takes at most 100 MB"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_upload", upload("logo.png", &caption)).await;
    assert!(said.contains("that element isn't a file field"), "{said}");

    let (said, _) = call(&mut client, &mut view, "runner", "browser_upload", upload("logo.png", &logo)).await;
    assert!(said.starts_with("Attached logo.png (") && said.contains("to the file field"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("chose logo.png"), "{said}");
    // Its label, listed first under the same name as the file picker it
    // opens, stands for the input.
    let (document, label) = reference(&said, "Logo");
    assert_ne!(label, file_input(&said).1, "the label is an element of its own");
    let upload = json!({"document": document, "target": label, "path": uploads.join("photo.jpg").display().to_string()});
    let (said, _) = call(&mut client, &mut view, "runner", "browser_upload", upload).await;
    assert!(said.starts_with("Attached photo.jpg ("), "{said}");
}

/// Only the person, in Agent settings, or an admin agent through the
/// settings file, gives an agent the browser; never factoryd's own
/// commands, and never to an admin agent or one limited to a pipe or to
/// reading reports. Turning it off, or deleting the agent, ends its
/// session, and its browser tools are refused.
#[tokio::test]
async fn who_gets_the_browser_and_what_turning_it_off_ends() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let models = common::Scripted::signed_in();
    let tools = models.tools.clone();
    let (addr, factory) = common::start_with(&dir, models).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://forms.example.org/signup", Some(SIGN_UP));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    apply_request(&mut client, &mut view, create_agent("site", "Scout", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Reader", "")).await;
    let refused = async |client: &mut Client, view: &mut View, agent: &str| match apply_request(client, view, browser(agent, true)).await {
        Body::Error(error) => error.message,
        other => panic!("{agent}: refused, got {other:?}"),
    };

    // Never an admin agent, nor one limited to a pipe or reading reports.
    assert_eq!(refused(&mut client, &mut view, "rivet").await, "Admin agents can't use the browser.");
    let pipe = factoryd::pipes::open_for_test(&factory, "site", "Partners", &["lead"], "lead", false);
    factoryd::pipes::join_unchecked_for_test(&factory, &pipe, "scout", true);
    assert_eq!(refused(&mut client, &mut view, "scout").await, "Agents limited to a pipe can't use the browser.");
    let reads = Request::SetReportReader(v1::SetReportReader { agent_id: "reader".into(), reads: true, fresh: false });
    assert!(!matches!(apply_request(&mut client, &mut view, reads).await, Body::Error(_)));
    assert_eq!(refused(&mut client, &mut view, "reader").await, "Agents that read reports can't use the browser.");

    // factoryd's own commands, which any agent's shell can run, can turn it
    // off but never on.
    let mut command = Client::open(addr).await;
    let hello = v1::Hello {
        protocol_version: factory_protocol::PROTOCOL_VERSION,
        client_version: format!("factoryd-cli {}", env!("CARGO_PKG_VERSION")),
        token: common::local_token(addr),
        ..Default::default()
    };
    command.send(v1::ClientMessage { id: 1, body: Some(Request::Hello(hello)) }).await;
    let (workspace, _) = command.welcomed().await;
    let mut seen = View { workspace, ..View::default() };
    let Body::Error(error) = apply_request(&mut command, &mut seen, browser("lead", true)).await else { panic!("refused from a command") };
    assert_eq!(error.message, "Only the person, in Agent settings, or an admin agent such as Rivet turns on the browser.");
    assert_eq!(error.code(), v1::ErrorCode::InvalidRequest);
    assert!(!matches!(apply_request(&mut command, &mut seen, browser("lead", false)).await, Body::Error(_)));

    // Turned off, its session ends and its browser tools are refused.
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    assert!(tools.lock().expect("lock").contains("\"browser_open\""));
    assert!(!matches!(apply_request(&mut client, &mut view, browser("runner", false)).await, Body::Error(_)));
    view.follow(&mut client, |view| session(view).state() != ErrandState::Running).await;
    assert_eq!(session(&view).end_reason, "Its browser was turned off.");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert_eq!(said, "The Browser extension is turned off for you, so browser_read doesn't work any more.");

    // Deleting a browsing agent ends its session too.
    assert!(!matches!(apply_request(&mut client, &mut view, browser("runner", true)).await, Body::Error(_)));
    assert!(factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile-again")), "a browser again");
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://forms.example.org/signup"})).await;
    assert_eq!(session(&view).state(), ErrandState::Running);
    let delete = Request::DeleteAgent(v1::DeleteAgent { agent_id: "runner".into() });
    assert!(matches!(apply_request(&mut client, &mut view, delete).await, Body::Ack(_)));
    view.follow(&mut client, |view| session(view).state() != ErrandState::Running).await;
    assert_eq!(session(&view).end_reason, "Its browser was turned off.");

    // Made admin in the settings file, a browsing agent loses the browser.
    assert!(!matches!(apply_request(&mut client, &mut view, browser("lead", true)).await, Body::Error(_)));
    view.follow(&mut client, |view| browsing(view, "lead")).await;
    let path = dir.join("home/config.json");
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    let lead = config["agents"].as_array_mut().expect("agents").iter_mut().find(|agent| agent["id"] == "lead").expect("lead");
    lead["extensions"]["agents"] = json!({"admin": true});
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("written");
    view.follow(&mut client, |view| !browsing(view, "lead")).await;
    let written = factoryd::config::Config::parse(&std::fs::read_to_string(&path).expect("config"), &dir.join("home")).expect("valid");
    let lead = written.agents.iter().find(|agent| agent.id == "lead").expect("lead");
    assert_ne!(lead.extensions.get("browser").and_then(|browser| browser.get("enabled")), Some(&json!(true)), "{:?}", lead.extensions);

    // Turned on in the settings file: Rivet's doing, as History says, with
    // the person's and Claktory's switches.
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    let writer = config["agents"].as_array_mut().expect("agents").iter_mut().find(|agent| agent["id"] == "writer").expect("writer");
    writer["extensions"]["browser"] = json!({"enabled": true});
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("written");
    view.follow(&mut client, |view| browsing(view, "writer")).await;
    let switches: Vec<String> = history(&mut client, &mut view)
        .await
        .into_iter()
        .filter(|record| record.kind() == v1::NeedsYouRecordKind::Browser)
        .map(|record| record.title)
        .collect();
    assert_eq!(
        switches,
        [
            "Rivet turned on the browser for Writer",
            "Claktory turned off the browser for Lead",
            "You turned on the browser for Lead",
            "You turned on the browser for runner",
            "You turned off the browser for runner",
            "You turned on the browser for runner",
        ]
    );
}

/// A settings file with an admin agent that has the browser reads with the
/// browser off; an old "browser" entry (browser control's id before) is
/// turned off once, at the first start; what the person turns on after
/// stays on. Errand requests from before answer that they're gone.
#[tokio::test]
async fn old_browser_settings_and_errand_requests_read_as_they_are_now() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let home = dir.join("home");
    let mut config: serde_json::Value =
        serde_json::from_str(&factoryd::config::Config::default().to_json(&home).expect("json")).expect("json");
    let mut old = config["agents"][0].clone();
    old["id"] = json!("old");
    old["name"] = json!("Old");
    old["extensions"] = json!({"browser": {"enabled": true}});
    config["agents"].as_array_mut().expect("agents").push(old);
    config["agents"][0]["extensions"]["browser"] = json!({"enabled": true});
    let text = serde_json::to_string_pretty(&config).expect("json");
    std::fs::create_dir_all(&home).expect("home");
    std::fs::write(home.join("config.json"), &text).expect("config");

    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let parsed = factoryd::config::Config::parse(&text, &home).expect("an admin agent with the browser still reads");
    assert_ne!(parsed.agents[0].extensions.get("browser").and_then(|browser| browser.get("enabled")), Some(&json!(true)));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| !browsing(view, "old")).await;
    assert!(!browsing(&view, "rivet"));
    assert!(!matches!(apply_request(&mut client, &mut view, browser("old", true)).await, Body::Error(_)));
    view.follow(&mut client, |view| browsing(view, "old")).await;
    // The app's own state is saved whole, as selecting an agent does.
    select(&mut client, &mut view, "old").await;
    drop(client);
    drop(factory);
    let (addr, _again) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let view = View { workspace, ..View::default() };
    assert!(browsing(&view, "old"), "turned off once only");

    let (addr, _third) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let switches: Vec<String> = history(&mut client, &mut view)
        .await
        .into_iter()
        .filter(|record| record.kind() == v1::NeedsYouRecordKind::Browser)
        .map(|record| record.title)
        .collect();
    assert_eq!(
        switches,
        ["You turned on the browser for Old", "Claktory turned off the browser for Old", "Claktory turned off the browser for Rivet"],
        "Claktory's switches are its own, once"
    );
    let open = Request::OpenErrand(v1::OpenErrand { agent_id: "old".into(), purpose: "Pay".into(), ..Default::default() });
    let contain = Request::SetAgentContained(v1::SetAgentContained { agent_id: "old".into(), contained: true });
    for request in [open, contain] {
        let Body::Error(error) = apply_request(&mut client, &mut view, request).await else { panic!("refused") };
        assert_eq!(error.code(), v1::ErrorCode::InvalidRequest, "{}", error.message);
    }
}

/// A check that the agent is a person, a payment, the person taking the
/// browser over and handing it back, and stopping it: the session follows
/// the browser's turns.
#[tokio::test]
async fn a_session_follows_the_browsers_turns() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_with_turns_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://pay.example.org/checkout", Some(PAY));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    let calls = || factoryd::errands::browser_calls_for_test(&factory);
    select(&mut client, &mut view, "runner").await;

    // Opening it begins the browser; driving it registers its page's
    // session to detach when the agent stops.
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://pay.example.org/checkout"})).await;
    let id = session(&view).id;
    let made = calls();
    assert!(made.first().is_some_and(|call| call.starts_with(&format!("begin {id}"))), "{made:?}");
    assert!(made.contains(&format!("release {id} 1")), "{made:?}");

    // A check that it's a person is no one's turn: Claktory tries again.
    factoryd::errands::check_timing_for_test(&factory, [Duration::from_secs(2); 3], Duration::from_millis(200));
    factoryd::errands::show_for_test(&factory, "<title>Just a moment...</title><p>Checking your browser</p>").await;
    let (said, card) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(card.is_none() && said.contains("tries the page again"), "{said}");
    factoryd::errands::show_for_test(&factory, "<title>Sign up</title><p>Organization</p>").await;
    view.follow(&mut client, |view| told(view, "cleared when Claktory tried") && idle(view, "runner")).await;

    // A payment stays driven, and I've paid drives it on.
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://pay.example.org/checkout"})).await;
    calls();
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", json!({"kind": "payment", "what": "Pay the fee."})).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert!(!calls().iter().any(|call| call.starts_with("handoff")), "no one takes the browser for a payment");
    let paid = client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, ..Default::default() });
    apply_request(&mut client, &mut view, paid).await;
    view.follow(&mut client, |view| session(view).state() == ErrandState::Running && idle(view, "runner")).await;

    // The person takes it over: the agent waits; handed back, it's told.
    factoryd::errands::browser_state_for_test(&factory, v1::AgentBrowserState::TakenOver as i32);
    view.follow(&mut client, |view| session(view).state() == ErrandState::TakenOver).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.starts_with("paused") && said.contains("took the browser over"), "{said}");
    factoryd::errands::browser_state_for_test(&factory, v1::AgentBrowserState::Driven as i32);
    view.follow(&mut client, |view| told(view, "handed the browser back") && idle(view, "runner")).await;
    assert_eq!(session(&view).state(), ErrandState::Running);

    // Stopping the browser ends the session; a request still waiting then
    // is withdrawn, with why.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", json!({"kind": "payment", "what": "Pay the fee."})).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    calls();
    factoryd::errands::browser_state_for_test(&factory, v1::AgentBrowserState::Stopping as i32);
    let withdrawn = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) if entry.seq == seq && ask.withdrawn => Some(ask.withdraw_reason.clone()),
            _ => None,
        })
    };
    view.follow(&mut client, |view| session(view).state() == ErrandState::Stopped && withdrawn(view).is_some() && idle(view, "runner"))
        .await;
    assert_eq!(session(&view).end_reason, "You stopped Claktory's browser.");
    assert!(withdrawn(&view).unwrap().contains("You stopped Claktory's browser."), "{:?}", withdrawn(&view));
    assert_eq!(calls(), [format!("end {id}")]);
}

/// A page that offers a file, and one that starts a download by itself.
const OFFER: &str = r#"<!doctype html><title>Assets</title>
    <a id=file href="data:text/plain;base64,aGVsbG8gd29ybGQ=" download="hello.txt" hidden></a>
    <button onclick="document.getElementById('file').click()">Get the logo</button>"#;
const PUSHY: &str = r#"<!doctype html><title>Free stuff</title><p>Nothing to click.</p>
    <a id=sneak href="data:text/plain;base64,aGk=" download="sneak.txt"></a>
    <script>document.getElementById('sneak').click()</script>"#;

/// A download the agent asks for lands in its downloads, named and hashed;
/// one the page starts by itself is never kept.
#[tokio::test]
async fn an_agents_download_lands_in_its_downloads_and_a_pages_own_never_does() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_with_turns_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::serve_for_test("https://assets.example.org/free", Some(PUSHY));
    factoryd::errands::serve_for_test("https://assets.example.org/logo", Some(OFFER));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    select(&mut client, &mut view, "runner").await;

    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://assets.example.org/free"})).await;
    let id = session(&view).id;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Nothing to click") && !said.contains("Saved"), "{said}");
    assert!(!factoryd::errands::browser_calls_for_test(&factory).iter().any(|call| call.starts_with("land")));

    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://assets.example.org/logo"})).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, get) = reference(&said, "Get the logo");
    let (said, _) =
        call(&mut client, &mut view, "runner", "browser_act", json!({"op": "click", "document": document, "target": get})).await;
    // Its mouse events went through the host's input gate, under a lease.
    assert!(factoryd::errands::browser_calls_for_test(&factory).contains(&"lease".to_owned()));
    let sha = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";
    assert!(
        said.contains(&format!("Saved hello.txt (11 bytes, sha256 {sha}) in downloads/{id}/")) && said.contains("<downloads-"),
        "{said}"
    );
    let kept = factory.home().root().join("errands/runner/downloads").join(&id).join("hello.txt");
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "hello world");
}

/// A page that asks for a code sent to the person; its title counts what
/// the field holds, so the agent's read shows a fill without the code.
const CODE: &str = r#"<!doctype html><title>Verify</title>
    <label>Code <input id=code autocomplete=one-time-code oninput="document.title = 'typed ' + this.value.length"></label>
    <label><input type=checkbox id=keep> Remember this browser</label>
    <button>Verify</button>"#;

#[tokio::test]
async fn a_code_goes_from_the_persons_card_into_the_field_and_never_to_the_agent() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    factoryd::errands::show_for_test(&factory, CODE).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, code) = reference(&said, "Code");
    let (_, keep) = reference(&said, "Remember");
    let hand = |target: &str| json!({"kind": "code", "what": "Type the code the shop texted you.", "document": document, "target": target});

    // The field is named, on an https page, and takes a code.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", json!({"kind": "code", "what": "Type the code."})).await;
    assert!(said.contains("kind code needs document and target") && card.is_none(), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_handoff", hand(&code)).await;
    assert!(said.contains("a code is only for an https page"), "{said}");
    factoryd::errands::code_cards_anywhere_for_test(&factory);
    let (said, _) = call(&mut client, &mut view, "runner", "browser_handoff", hand(&keep)).await;
    assert!(said.contains("only a text, number or one-time code field takes a code"), "{said}");
    let pay = json!({"kind": "payment", "what": "Pay.", "document": document, "target": code});
    let (said, _) = call(&mut client, &mut view, "runner", "browser_handoff", pay).await;
    assert!(said.contains("document and target are only for kinds code, agreement and recovery_codes"), "{said}");

    // The card is Claktory's, for a code only.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", hand(&code)).await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert!(said.contains("you never see it"), "{said}");
    assert_eq!(card.title, "shown.test asks for a code");
    assert!(card.code && card.wants_text, "{card:?}");
    let handoff = card.handoff.as_ref().expect("its handoff");
    assert_eq!((handoff.kind(), handoff.numeric), (v1::ErrandHandoffKind::Code, false));
    assert_eq!(session(&view).state(), ErrandState::PersonsTurn);
    let answer = |text: &str| Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, text: text.into(), ..Default::default() });

    // Something that isn't a code leaves the card waiting.
    for (text, why) in [("", "Type the code"), ("12 34!", "A code is up to 16"), ("12345678901234567", "A code is up to 16")] {
        let Body::Error(refused) = apply_request(&mut client, &mut view, answer(text)).await else { panic!("refused: {text}") };
        assert!(refused.message.contains(why), "{text}: {}", refused.message);
    }
    let Body::Error(refused) = apply_request(
        &mut client,
        &mut view,
        Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, text: "123456".into(), reply: true, ..Default::default() }),
    )
    .await
    else {
        panic!("a reply refused")
    };
    assert!(refused.message.contains("Fill in"), "{}", refused.message);

    // Filled in: the agent is told, with the page, and nothing keeps the code.
    apply_request(&mut client, &mut view, answer(" 123-456 ")).await;
    view.follow(&mut client, |view| told(view, "Claktory filled it in") && idle(view, "runner")).await;
    let events: Vec<&str> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) => Some(event.text.as_str()),
            _ => None,
        })
        .collect();
    assert!(told(&view, "typed 7"), "the page holds the code: {events:?}");
    assert_eq!(session(&view).state(), ErrandState::Running);
    let answered = view.entries.iter().find(|entry| entry.seq == seq).and_then(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some(ask.clone()),
        _ => None,
    });
    let answered = answered.expect("the card");
    assert_eq!((answered.state(), answered.answer.as_str(), answered.action_error.as_str()), (v1::AskPersonState::Answered, "", ""));
    assert!(!format!("{:?}", view.entries).contains("123-456"), "the code in the conversation");
    assert!(!format!("{:?}", view.workspace).contains("123-456"), "the code in the Workspace");
    let Body::ErrandLog(log) = apply_request(&mut client, &mut view, Request::ListErrandLog(v1::ListErrandLog::default())).await else {
        panic!("the log")
    };
    let actions: Vec<&str> = log.entries.iter().map(|entry| entry.action.as_str()).collect();
    assert!(actions.contains(&"Handed to you: a code") && actions.contains(&"Code filled on https://shown.test"), "{actions:?}");
    assert!(!format!("{log:?}").contains("123-456"));

    // The page changed under the card: the code goes nowhere, the card ends
    // saying why, and the agent reads the page again.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, code) = reference(&said, "Code");
    let again = json!({"kind": "code", "what": "Type the code.", "document": document, "target": code});
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", again).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    factoryd::errands::show_for_test(&factory, "<title>Signed out</title><p>Your session ended.</p>").await;
    let answer = Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, text: "654321".into(), ..Default::default() });
    apply_request(&mut client, &mut view, answer).await;
    view.follow(&mut client, |view| told(view, "couldn't fill it in") && idle(view, "runner")).await;
    let ended = view.entries.iter().find(|entry| entry.seq == seq).and_then(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some(ask.clone()),
        _ => None,
    });
    let ended = ended.expect("the card");
    assert!(ended.action_error.starts_with("It wasn't filled in: "), "{ended:?}");
    assert_eq!(session(&view).state(), ErrandState::Running);
    // History says what the card was, and why it didn't go through (row
    // 1645), never the code.
    let records = history(&mut client, &mut view).await;
    let record = records.iter().find(|record| record.seq == seq).unwrap_or_else(|| panic!("the record: {records:?}"));
    let handoff = record.handoff.as_ref().expect("its handoff");
    assert_eq!((handoff.kind(), handoff.origin.as_str()), (v1::ErrandHandoffKind::Code, "https://shown.test"));
    assert!(record.action_error.starts_with("It wasn't filled in: ") && !format!("{record:?}").contains("654321"), "{record:?}");

    // Can't do this gives the browser back with the person's reason. A
    // field for digits only says so, for a phone's number pad.
    factoryd::errands::show_for_test(
        &factory,
        r#"<title>Verify</title><label>Code <input id=code inputmode=numeric pattern="[0-9]*"></label>"#,
    )
    .await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, code) = reference(&said, "Code");
    let again = json!({"kind": "code", "what": "Type the code.", "document": document, "target": code});
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", again).await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert!(card.handoff.expect("its handoff").numeric);
    let no = Request::AnswerAsk(v1::AnswerAsk {
        agent_id: "runner".into(),
        seq,
        dismiss: true,
        text: "No code came".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, no).await;
    view.follow(&mut client, |view| told(view, "couldn't do your handoff") && idle(view, "runner")).await;
    assert!(told(&view, "No code came") && told(&view, "Your browser is yours again"));
    assert_eq!(session(&view).state(), ErrandState::Running);
}

/// A page with an agreement to tick; its title says whether it's ticked.
const AGREE: &str = r#"<!doctype html><title>not yet</title>
    <label>Name <input id=name></label>
    <label><input type=checkbox id=terms onchange="document.title = this.checked ? 'agreed' : 'not yet'"> I agree to the Developer Distribution Agreement</label>
    <button>Next</button>"#;

#[tokio::test]
async fn an_agreement_is_ticked_only_on_the_persons_accept_and_a_reply_ticks_nothing() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::code_cards_anywhere_for_test(&factory);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    factoryd::errands::show_for_test(&factory, AGREE).await;
    let refs = |said: &str| (reference(said, "I agree"), reference(said, "Name").1);
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let ((document, terms), name) = refs(&said);
    let agree = |document: &str, target: &str| json!({"kind": "agreement", "what": "Accept the distribution agreement.", "document": document, "target": target});

    // The box is named, and only a box that isn't ticked.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", json!({"kind": "agreement", "what": "Accept it."})).await;
    assert!(said.contains("kind agreement needs document and target") && card.is_none(), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_handoff", agree(&document, &name)).await;
    assert!(said.contains("only a box that isn't ticked yet"), "{said}");
    // Its commands still never tick it.
    let (said, _) =
        call(&mut client, &mut view, "runner", "browser_act", json!({"op": "click", "document": document, "target": terms})).await;
    assert!(said.contains("sensitive_action"), "{said}");

    // The card shows the box's words as the site's.
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", agree(&document, &terms)).await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert_eq!(card.title, "shown.test asks you to agree");
    let handoff = card.handoff.clone().expect("its handoff");
    assert_eq!(
        (handoff.kind(), handoff.agreement.as_str()),
        (v1::ErrandHandoffKind::Agreement, "I agree to the Developer Distribution Agreement")
    );
    assert!(!card.code && !card.wants_text, "{card:?}");
    assert_eq!(session(&view).state(), ErrandState::PersonsTurn);
    let answer = |seq: u64, text: &str, reply: bool| {
        Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, text: text.into(), reply, ..Default::default() })
    };

    // Accept takes no words; a reply ticks nothing, and the agent reads it.
    let Body::Error(refused) = apply_request(&mut client, &mut view, answer(seq, "Fine by me", false)).await else { panic!("refused") };
    assert!(refused.message.contains("takes no words"), "{}", refused.message);
    apply_request(&mut client, &mut view, answer(seq, "Which version is it?", true)).await;
    view.follow(&mut client, |view| told(view, "nothing was ticked") && idle(view, "runner")).await;
    assert!(told(&view, "Which version is it?") && told(&view, "not yet"), "the reply, and the box unticked");
    assert_eq!(session(&view).state(), ErrandState::Running);

    // Accepted: Claktory ticks only that box.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let ((document, terms), _) = refs(&said);
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", agree(&document, &terms)).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    apply_request(&mut client, &mut view, answer(seq, "", false)).await;
    view.follow(&mut client, |view| told(view, "Claktory ticked the box") && idle(view, "runner")).await;
    assert!(told(&view, "agreed"), "the box ticked");
    let card = view.entries.iter().find(|entry| entry.seq == seq).and_then(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some(ask.clone()),
        _ => None,
    });
    let card = card.expect("the card");
    assert_eq!((card.state(), card.action_error.as_str()), (v1::AskPersonState::Answered, ""));

    // The box's words changed under the card: nothing is ticked.
    factoryd::errands::show_for_test(&factory, AGREE).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let ((document, terms), _) = refs(&said);
    let (said, card) = call(&mut client, &mut view, "runner", "browser_handoff", agree(&document, &terms)).await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    factoryd::errands::show_for_test(&factory, &AGREE.replace("the Developer Distribution Agreement", "pay $99 a month")).await;
    apply_request(&mut client, &mut view, answer(seq, "", false)).await;
    view.follow(&mut client, |view| told(view, "couldn't tick the box") && idle(view, "runner")).await;
    let card = view.entries.iter().find(|entry| entry.seq == seq).and_then(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some(ask.clone()),
        _ => None,
    });
    assert!(card.expect("the card").action_error.starts_with("It wasn't ticked: "));

    let Body::ErrandLog(log) = apply_request(&mut client, &mut view, Request::ListErrandLog(v1::ListErrandLog::default())).await else {
        panic!("the log")
    };
    let actions: Vec<&str> = log.entries.iter().map(|entry| entry.action.as_str()).collect();
    for action in [
        "Handed to you: an agreement",
        "You replied: nothing ticked",
        "Agreement ticked on https://shown.test",
        "Couldn't tick the agreement",
    ] {
        assert!(actions.contains(&action), "{action} in {actions:?}");
    }
}

/// A site's backup codes, shown once.
const CODES: &str = r#"<!doctype html><title>Backup codes</title>
    <h1>Save your backup codes</h1><p>Each works once.</p>
    <div><ul><li>a8f2-91kd</li><li>c3m9-2wle</li><li>k1vv-o3ds</li><li>7bq1-x0pe</li><li>p0za-77rt</li></ul></div>
    <button>I've saved them</button>"#;

/// A security page that offers two-step sign-in.
const TWO_STEP_PAGE: &str = r#"<!doctype html><title>Security</title>
    <h1>Security</h1><button>Turn on 2-Step Verification</button><button>Set up an authenticator app</button><button>Done</button>"#;

#[tokio::test]
async fn recovery_codes_go_to_the_person_only_and_two_step_is_theirs_to_turn_on() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::code_cards_anywhere_for_test(&factory);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_read", json!({})).await;

    // Two-step: the agent can't turn it on; the person approves, then it
    // may, and an authenticator app stays refused.
    factoryd::errands::show_for_test(&factory, TWO_STEP_PAGE).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("two-step sign-in: the person decides"), "{said}");
    let (document, turn_on) = reference(&said, "Turn on 2-Step");
    let (_, authenticator) = reference(&said, "authenticator app");
    let click = |document: &str, target: &str| json!({"op": "click", "document": document, "target": target});
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", click(&document, &turn_on)).await;
    assert!(said.contains("sensitive_action") && said.contains("kind two_step"), "{said}");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "two_step", "what": "The shop asks to turn on 2-Step Verification."}),
    )
    .await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert_eq!(card.title, "shown.test wants two-step sign-in turned on");
    assert_eq!(card.handoff.as_ref().map(|handoff| handoff.kind()), Some(v1::ErrandHandoffKind::TwoStep));
    let approve = |seq: u64| Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, ..Default::default() });
    apply_request(&mut client, &mut view, approve(seq)).await;
    view.follow(&mut client, |view| told(view, "approved turning on two-step sign-in") && idle(view, "runner")).await;
    // What the page showed leaves Chrome's caches once the card ends,
    // whatever the answer.
    let cleared = || factoryd::errands::caches_cleared_for_test(&factory);
    assert_eq!(cleared(), ["https://shown.test"]);
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, turn_on) = reference(&said, "Turn on 2-Step");
    let (_, authenticator_now) = reference(&said, "authenticator app");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", click(&document, &turn_on)).await;
    assert!(!said.contains("sensitive_action"), "approved: {said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", click(&document, &authenticator_now)).await;
    assert!(said.contains("sensitive_action") && said.contains("authenticator app, security key or passkey"), "{said}");
    let _ = authenticator;

    // Recovery codes: read off the page by Claktory, never by the agent.
    factoryd::errands::show_for_test(&factory, CODES).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(!said.contains("a8f2-91kd"), "{said}");
    let (document, block) = reference(&said, "recovery codes (withheld)");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "recovery_codes", "what": "Save the shop's backup codes.", "document": document, "target": block}),
    )
    .await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    assert!(said.contains("You never see them"), "{said}");
    assert_eq!(card.title, "shown.test recovery codes");
    assert!(card.body.starts_with("Claktory keeps them safe until you choose."), "{}", card.body);
    assert_eq!(card.handoff.as_ref().map(|handoff| handoff.kind()), Some(v1::ErrandHandoffKind::RecoveryCodes));
    assert_eq!(session(&view).state(), ErrandState::PersonsTurn);

    // Only the person's Show sees them, on the connection that asked: a
    // second device connected meanwhile gets nothing of them (Gyro,
    // P-recovery-codes).
    let (mut other, _, _) = Client::connect(addr).await;
    let show = Request::ShowRecoveryCodes(v1::ShowRecoveryCodes { agent_id: "runner".into(), seq });
    let Body::RecoveryCodes(shown) = apply_request(&mut client, &mut view, show.clone()).await else { panic!("the codes") };
    assert_eq!(shown.codes, ["a8f2-91kd", "c3m9-2wle", "k1vv-o3ds", "7bq1-x0pe", "p0za-77rt"]);
    let elsewhere = other.ping(b"after the show").await;
    assert!(!elsewhere.iter().any(|message| matches!(message.body, Some(Body::RecoveryCodes(_)))), "another connection got them");
    assert!(!format!("{elsewhere:?}").contains("a8f2-91kd"), "another connection saw a code");
    // No Dismiss, reply or words.
    let refusals = [
        v1::AnswerAsk { agent_id: "runner".into(), seq, dismiss: true, ..Default::default() },
        v1::AnswerAsk { agent_id: "runner".into(), seq, reply: true, text: "Thanks".into(), ..Default::default() },
        v1::AnswerAsk { agent_id: "runner".into(), seq, text: "Saved".into(), ..Default::default() },
    ];
    for refused in refusals {
        let Body::Error(error) = apply_request(&mut client, &mut view, Request::AnswerAsk(refused.clone())).await else {
            panic!("refused: {refused:?}")
        };
        assert!(error.message.contains("I saved them"), "{}", error.message);
    }
    // Keep in Secrets: a secret given to no agent, and the card closes.
    let keep = Request::KeepRecoveryCodes(v1::KeepRecoveryCodes { agent_id: "runner".into(), seq });
    apply_request(&mut client, &mut view, keep.clone()).await;
    view.follow(&mut client, |view| told(view, "kept the recovery codes in Secrets") && idle(view, "runner")).await;
    assert_eq!(cleared().len(), 2, "{:?}", cleared());
    let vault = view.workspace.vault.clone().expect("the vault");
    let secret = vault.secrets.iter().find(|secret| secret.name == "shown.test recovery codes").expect("kept");
    assert!(secret.grants.is_empty(), "{secret:?}");
    // No hint: four characters of a code would reach every device.
    assert!(secret.variables.iter().all(|variable| variable.hint.is_empty()), "{secret:?}");
    assert!(!format!("{:?}", view.workspace).contains("77rt"), "a code's end in the Workspace");
    let Body::Error(gone) = apply_request(&mut client, &mut view, show).await else { panic!("gone") };
    assert!(gone.message.contains("These codes are gone"), "{}", gone.message);
    assert_eq!(session(&view).state(), ErrandState::Running);

    // I saved them: Claktory's copy goes.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, block) = reference(&said, "recovery codes (withheld)");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "recovery_codes", "what": "Save them.", "document": document, "target": block}),
    )
    .await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    apply_request(&mut client, &mut view, approve(seq)).await;
    view.follow(&mut client, |view| told(view, "saved the recovery codes") && idle(view, "runner")).await;
    assert_eq!(cleared().len(), 3, "{:?}", cleared());
    assert!(cleared().iter().all(|origin| origin == "https://shown.test"), "{:?}", cleared());
    let show = Request::ShowRecoveryCodes(v1::ShowRecoveryCodes { agent_id: "runner".into(), seq });
    let Body::Error(gone) = apply_request(&mut client, &mut view, show).await else { panic!("gone") };
    assert!(gone.message.contains("These codes are gone"), "{}", gone.message);
    // History tells Keep in Secrets from I saved them.
    let records = history(&mut client, &mut view).await;
    let codes: Vec<&str> = records
        .iter()
        .filter(|record| record.handoff.as_ref().is_some_and(|handoff| handoff.kind() == v1::ErrandHandoffKind::RecoveryCodes))
        .map(|record| record.codes_kept_as.as_str())
        .collect();
    assert_eq!(codes, ["", "shown.test recovery codes"], "{records:?}");

    // Codes lost (a restart while Secrets was locked): Show and Keep say
    // so, and I saved them closes the card.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, block) = reference(&said, "recovery codes (withheld)");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "recovery_codes", "what": "Save them.", "document": document, "target": block}),
    )
    .await;
    let (seq, card) = card.unwrap_or_else(|| panic!("a request: {said}"));
    factoryd::errands::lose_codes_for_test(&factory, &card.id);
    let show = Request::ShowRecoveryCodes(v1::ShowRecoveryCodes { agent_id: "runner".into(), seq });
    let Body::Error(gone) = apply_request(&mut client, &mut view, show).await else { panic!("gone") };
    assert!(gone.message.contains("Choose I saved them to close the request."), "{}", gone.message);
    let keep = Request::KeepRecoveryCodes(v1::KeepRecoveryCodes { agent_id: "runner".into(), seq });
    let Body::Error(gone) = apply_request(&mut client, &mut view, keep).await else { panic!("nothing to keep") };
    assert!(gone.message.contains("nothing to keep"), "{}", gone.message);
    apply_request(&mut client, &mut view, approve(seq)).await;
    view.follow(&mut client, |view| answered(view, seq) && idle(view, "runner")).await;

    // A card closed without the person choosing (/clear): its codes become
    // a secret given to no agent, never deleted.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, block) = reference(&said, "recovery codes (withheld)");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "recovery_codes", "what": "Save them.", "document": document, "target": block}),
    )
    .await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    let held = Request::ShowRecoveryCodes(v1::ShowRecoveryCodes { agent_id: "runner".into(), seq });
    assert!(matches!(apply_request(&mut client, &mut view, held).await, Body::RecoveryCodes(_)));
    let clear = v1::RunCommand { agent_id: "runner".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, Request::RunCommand(clear)).await;
    factoryd::errands::sweep_codes_for_test(&factory);
    let orphan_kept = |view: &View| {
        view.workspace.vault.as_ref().is_some_and(|vault| vault.secrets.iter().any(|secret| secret.name == "shown.test recovery codes 2"))
    };
    view.follow(&mut client, orphan_kept).await;
    let vault = view.workspace.vault.clone().expect("the vault");
    let orphaned = vault
        .secrets
        .iter()
        .find(|secret| secret.name == "shown.test recovery codes 2")
        .unwrap_or_else(|| panic!("kept: {:?}", vault.secrets));
    assert!(orphaned.grants.is_empty(), "{orphaned:?}");
    factoryd::errands::sweep_codes_for_test(&factory);
    assert!(
        !view.workspace.vault.clone().expect("the vault").secrets.iter().any(|secret| secret.name == "shown.test recovery codes 3"),
        "kept once"
    );

    // The codes are nowhere but the secret: not the conversation, the
    // Workspace or the log.
    for code in ["a8f2-91kd", "p0za-77rt"] {
        assert!(!format!("{:?}", view.entries).contains(code), "{code} in the conversation");
        assert!(!format!("{:?}", view.workspace).contains(code), "{code} in the Workspace");
    }
    let Body::ErrandLog(log) = apply_request(&mut client, &mut view, Request::ListErrandLog(v1::ListErrandLog::default())).await else {
        panic!("the log")
    };
    assert!(!format!("{log:?}").contains("a8f2-91kd"));
    // Nor any file Claktory keeps (factory.db and its -wal, the log, the
    // conversations; the vault holds them sealed), Chrome's own profile
    // aside, which had the page.
    fn files(dir: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                files(&path, found);
            } else {
                found.push(path);
            }
        }
    }
    let mut kept = Vec::new();
    files(&dir.join("home"), &mut kept);
    assert!(kept.iter().any(|path| path.ends_with("factory.db")), "{kept:?}");
    for path in &kept {
        let bytes = std::fs::read(path).unwrap_or_default();
        for code in ["a8f2-91kd", "c3m9-2wle", "p0za-77rt"] {
            assert!(!bytes.windows(code.len()).any(|window| window == code.as_bytes()), "{code} in {}", path.display());
        }
    }
    let actions: Vec<&str> = log.entries.iter().map(|entry| entry.action.as_str()).collect();
    for action in [
        "Asked you about two-step sign-in",
        "You approved two-step sign-in on https://shown.test",
        "Handed to you: recovery codes",
        "You kept the recovery codes in Secrets",
        "You saved the recovery codes",
        "Recovery codes kept in Secrets as shown.test recovery codes 2: their request closed",
    ] {
        assert!(actions.contains(&action), "{action} in {actions:?}");
    }
}

/// A recovery-codes card whose browsing session ended (Claktory restarted)
/// still closes with I saved them: its codes wait through the session's
/// end, and only the person's choice lets them go.
#[tokio::test]
async fn recovery_codes_outlast_the_browsing_session() {
    use client_message::Body as Request;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    factoryd::errands::code_cards_anywhere_for_test(&factory);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    factoryd::errands::show_for_test(&factory, CODES).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, block) = reference(&said, "recovery codes (withheld)");
    let (said, card) = call(
        &mut client,
        &mut view,
        "runner",
        "browser_handoff",
        json!({"kind": "recovery_codes", "what": "Save them.", "document": document, "target": block}),
    )
    .await;
    let (seq, _) = card.unwrap_or_else(|| panic!("a request: {said}"));
    // Its page may be in Chrome's caches: a clear is owed until it's made.
    // The browser goes away first, as at a restart, so the session's end
    // can't make it.
    let owed = || factoryd::errands::caches_owed_for_test(&factory);
    let cleared = || factoryd::errands::caches_cleared_for_test(&factory);
    assert_eq!(owed(), ["https://shown.test"]);
    factoryd::errands::drop_connection_for_test(&factory).await;
    assert!(factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile-again")), "a browser again");
    let ended = session(&view).id.clone();
    factoryd::errands::end_errand_for_test(&factory, &ended).await;
    assert_eq!(owed(), ["https://shown.test"]);
    assert!(cleared().is_empty(), "{:?}", cleared());
    let show = Request::ShowRecoveryCodes(v1::ShowRecoveryCodes { agent_id: "runner".into(), seq });
    assert!(matches!(apply_request(&mut client, &mut view, show.clone()).await, Body::RecoveryCodes(_)), "kept through the session's end");
    // Still no Dismiss.
    let dismiss = Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, dismiss: true, ..Default::default() });
    let Body::Error(refused) = apply_request(&mut client, &mut view, dismiss).await else { panic!("refused") };
    assert!(refused.message.contains("I saved them"), "{}", refused.message);
    let approve = Request::AnswerAsk(v1::AnswerAsk { agent_id: "runner".into(), seq, ..Default::default() });
    apply_request(&mut client, &mut view, approve).await;
    view.follow(&mut client, |view| told(view, "(Your browser had closed by then.)") && idle(view, "runner")).await;
    assert!(answered(&view, seq));
    let Body::Error(gone) = apply_request(&mut client, &mut view, show).await else { panic!("gone") };
    assert!(gone.message.contains("These codes are gone"), "{}", gone.message);
    // The browser's next driven start makes the owed clear before its
    // first page.
    assert_eq!(owed(), ["https://shown.test"]);
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert_eq!(cleared(), ["https://shown.test"], "{said}");
    assert!(owed().is_empty(), "{:?}", owed());
}

/// A page a cookie notice covers, which offers only its Accept.
const NOTICE: &str = r#"<!doctype html><title>Kettles</title><h1>Kettles</h1>
    <div role=dialog style="position:fixed;bottom:0;left:0;right:0;height:140px;background:#fff">We use cookies.
    <button onclick="this.parentNode.remove()">Got it</button></div>"#;

#[tokio::test]
async fn the_browser_clears_what_covers_the_page_and_logs_cookies_it_had_to_take() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    factoryd::errands::show_for_test(&factory, NOTICE).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains(r#""overlays":["cookie notice"]"#), "{said}");
    let (document, got_it) = reference(&said, "Got it");

    // A dismiss names only the page: Claktory picks what to press.
    let (said, _) =
        call(&mut client, &mut view, "runner", "browser_act", json!({"op": "dismiss", "document": document, "target": got_it})).await;
    assert!(said.contains("takes only document"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", json!({"op": "dismiss"})).await;
    assert!(said.contains("needs document"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_act", json!({"op": "dismiss", "document": document})).await;
    assert!(
        said.contains(r#""pressed":["Got it"]"#) && said.contains(r#""accepted_cookies":true"#) && said.contains(r#""gone":true"#),
        "{said}"
    );

    // The log says what was cleared, and that its cookies were taken, in
    // Claktory's words.
    let Body::ErrandLog(log) =
        apply_request(&mut client, &mut view, client_message::Body::ListErrandLog(v1::ListErrandLog::default())).await
    else {
        panic!("the log")
    };
    let actions: Vec<&str> = log.entries.iter().map(|entry| entry.action.as_str()).collect();
    assert!(actions.contains(&"Cleared a cookie notice"), "{actions:?}");
    assert!(actions.contains(&"Accepted the site's cookies: its notice offered no other choice"), "{actions:?}");
    assert!(log.entries.iter().all(|entry| !format!("{entry:?}").contains("Got it")), "{log:?}");
}

#[tokio::test]
async fn the_browser_waits_for_its_hidden_start_and_says_when_it_failed() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_with_turns_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    factoryd::errands::browser_unbooted_for_test(&factory, Some("a window showed"));
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert_eq!(said, "interrupted: Claktory's browser didn't start hidden (a window showed)");
    factoryd::errands::browser_unbooted_for_test(&factory, None);
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("a stranger's words"), "{said}");
}

/// A site's sign-in form on its own https origin, and the page it sends to.
const SHOP_SIGN_IN: &str = r#"<!doctype html><title>Sign in to Shop</title>
    <form action="/home"><label>Email <input type=email name=user autocomplete=username></label>
    <label>Password <input type=password name=pw autocomplete=current-password></label>
    <button>Sign in</button></form>"#;
const HOME: &str = "<!doctype html><title>Shop</title><p>Welcome back.</p>";

#[tokio::test]
async fn a_login_the_person_gives_on_its_card_is_typed_on_its_site_and_reaches_no_one_else() {
    use client_message::Body as Request;
    const PASSWORD: &str = "correct-horse-7";
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    if !factoryd::errands::local_chrome_for_test(&factory, &dir.join("profile")) {
        eprintln!("no Chrome here: skipped");
        return;
    }
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    runner_with_browser(&mut client, &mut view, &dir).await;
    factoryd::errands::open_browser_for_test(&factory, "runner").expect("its browser");
    select(&mut client, &mut view, "runner").await;
    // The site serves its home page only to the form sent with both values.
    let home = format!("https://shop.example/home?user=matt%40example.com&pw={PASSWORD}");
    factoryd::errands::serve_for_test("https://shop.example/login", Some(SHOP_SIGN_IN));
    factoryd::errands::serve_for_test(&home, Some(HOME));
    call(&mut client, &mut view, "runner", "browser_open", json!({"url": "https://shop.example/login"})).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    let (document, user) = reference(&said, "Email");
    let (_, password) = reference(&said, "Password");
    let sign_in = json!({"document": document, "username": user, "password": password, "reason": "To check your orders."});

    // No login yet: the person is asked, for exactly this site, and nothing is typed.
    let (said, _) = call(&mut client, &mut view, "runner", "browser_sign_in", sign_in.clone()).await;
    assert!(said.contains("Asked") && said.contains("shop.example"), "{said}");
    let request = factoryd::vault::logins::login_request_for_test(&factory, "runner", "https://shop.example").expect("the login request");
    assert_eq!((request.state(), request.login_site.as_str()), (v1::SecretRequestState::Pending, "shop.example"));
    let card = |view: &View| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::SecretRequest(shown)) if shown.id == request.id)).count()
    };
    assert_eq!(card(&view), 1, "the login request shows in the conversation");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_sign_in", sign_in.clone()).await;
    assert!(said.contains("waits for"), "while the card waits, nothing is typed: {said}");
    assert_eq!(card(&view), 1, "asked once");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains(r#""path":"/login""#), "nothing was sent: {said}");
    let (document, user) = reference(&said, "Email");
    let (_, password) = reference(&said, "Password");
    let sign_in = json!({"document": document, "username": user, "password": password});

    // Signed in on the card: the agent calls again, and Claktory types it.
    let entry = |variable: &str, value: &str| v1::SecretEntry { variable: variable.into(), value: value.into(), file: false };
    let decide = v1::DecideSecretRequest {
        agent_id: "runner".into(),
        request_id: request.id.clone(),
        approve: true,
        entries: vec![entry("USERNAME", "matt@example.com"), entry("PASSWORD", PASSWORD)],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide)).await;
    view.follow(&mut client, |view| told(view, "call browser_sign_in again") && idle(view, "runner")).await;
    let (said, _) = call(&mut client, &mut view, "runner", "browser_sign_in", sign_in).await;
    assert!(said.starts_with("Signed in to shop.example with Shop login"), "{said}");
    let (said, _) = call(&mut client, &mut view, "runner", "browser_read", json!({})).await;
    assert!(said.contains("Welcome back."), "the site got both: {said}");
    let now = factoryd::vault::logins::login_request_for_test(&factory, "runner", "https://shop.example").expect("the request");
    assert_eq!(now.login_outcome(), v1::SecretLoginOutcome::SignedIn);

    // Nowhere but the site: not the conversation, the Workspace or the browser log.
    assert!(!format!("{:?}", view.entries).contains(PASSWORD), "the password in the conversation");
    assert!(!format!("{:?}", view.workspace).contains(PASSWORD), "the password in the Workspace");
    let Body::ErrandLog(log) = apply_request(&mut client, &mut view, Request::ListErrandLog(v1::ListErrandLog::default())).await else {
        panic!("the log")
    };
    let actions: Vec<&str> = log.entries.iter().map(|entry| entry.action.as_str()).collect();
    assert!(actions.contains(&"Typed Shop login into the password field") && actions.contains(&"Signed in with Shop login"), "{actions:?}");
    assert!(!format!("{log:?}").contains(PASSWORD));
}
