//! A provider that keeps answering with an error that passes
//! (overloaded, a server error, a rate limit) is waited out, past the quick
//! retries, and the run carries on once it answers; an error that won't
//! pass still fails at once.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use crate::accounts::{registry_over, serve};
use crate::common::{Client, View, apply_request, describe, send_message, temp_dir};
use crate::usage_switch::{Answer, Route, create, openrouter_reply, serve_routes, until};
use factory_agent::clock::now_ms;
use factory_protocol::v1::{self, client_message::Body as Request};
use factoryd::factory::Factory;
use serde_json::json;

const WAITING: &str = "Waiting for OpenRouter to answer again…";
const TROUBLE: &str = "notice: OpenRouter is having trouble answering. Builder keeps trying by itself and carries on once it answers.";

/// OpenRouter answering `failing` while `down`, then as usual; with how
/// many chat requests it was sent.
struct Outage {
    down: Arc<AtomicBool>,
    asked: Arc<AtomicUsize>,
}

fn overloaded() -> Answer {
    (529, String::new(), "application/json", json!({"error": {"code": 529, "message": "Overloaded"}}).to_string())
}

/// A Factory whose Builder runs on OpenRouter, answered by `failing` while
/// the outage lasts, waiting `least` at first for it, and has said hello.
async fn outage(
    dir: &std::path::Path,
    failing: fn() -> Answer,
    least: Duration,
    give_up: Duration,
) -> (Arc<Factory>, Client, View, Outage) {
    let (down, asked) = (Arc::new(AtomicBool::new(true)), Arc::new(AtomicUsize::new(0)));
    let route: Route = {
        let (down, asked) = (down.clone(), asked.clone());
        Arc::new(move |path, _| {
            if !path.ends_with("/chat/completions") {
                return (404, String::new(), "application/json", "{}".to_owned());
            }
            asked.fetch_add(1, Ordering::SeqCst);
            if down.load(Ordering::SeqCst) { failing() } else { openrouter_reply() }
        })
    };
    let (base_url, _) = serve_routes(tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind"), route);
    let file = dir.join("auth.json");
    std::fs::write(&file, json!({"openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(dir), Arc::new(registry_over(file, &base_url))).await.expect("open");
    factory.set_provider_wait(least);
    factory.set_provider_give_up(give_up);
    let (mut client, mut view) = serve(&factory, dir).await;
    create(&mut client, &mut view, "Builder", "openrouter/anthropic/claude-opus-4.8", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    (factory, client, view, Outage { down, asked })
}

async fn waiting(client: &mut Client, view: &mut View) {
    until(client, view, "builder waiting for OpenRouter", |view| view.agent("builder").usage_wait == WAITING).await;
}

async fn idle(client: &mut Client, view: &mut View, what: &str) {
    until(client, view, what, |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
}

fn notices(view: &View) -> Vec<String> {
    describe(&view.entries).into_iter().filter(|entry| entry.starts_with("notice: ")).collect()
}

/// Overloaded for a while (529s, as Anthropic answers): after the quick
/// retries it waits, longer each time, says so once, and carries on by
/// itself once the provider answers; the run never fails.
#[tokio::test]
async fn an_overloaded_provider_is_waited_out_and_the_run_completes() {
    let dir = temp_dir();
    let (_factory, mut client, mut view, outage) = outage(&dir, overloaded, Duration::from_millis(100), Duration::from_secs(600)).await;
    waiting(&mut client, &mut view).await;
    let agent = view.agent("builder").clone();
    assert_eq!(agent.status(), v1::AgentStatus::Working, "the run goes on");
    assert!(agent.usage_wait_until_ms > 0 && agent.usage_wait_until_ms <= i64::try_from(now_ms()).expect("now") + 1_000, "{agent:?}");
    assert!(agent.usage_wait_alternative.is_none() && !agent.usage_wait_no_other_provider);

    // Still overloaded through several waits (100, 200, 400, 800 ms).
    tokio::time::sleep(Duration::from_millis(1600)).await;
    assert!(outage.asked.load(Ordering::SeqCst) >= 5, "asked again after each wait: {}", outage.asked.load(Ordering::SeqCst));
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert_eq!(view.agent("builder").status(), v1::AgentStatus::Working, "{:?}", describe(&view.entries));

    outage.down.store(false, Ordering::SeqCst);
    idle(&mut client, &mut view, "builder answered").await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi from openrouter")).await;
    let notices = notices(&view);
    assert_eq!(notices.iter().filter(|notice| *notice == TROUBLE).count(), 1, "said once: {notices:?}");
    let retries = notices.iter().filter(|notice| notice.starts_with("notice: The model request failed. Retrying")).count();
    assert_eq!(retries, 2, "the quick retries only once: {notices:?}");
    assert!(!notices.iter().any(|notice| notice.contains("stopped") || notice.contains("Overloaded")), "{notices:?}");
}

/// A request the provider refuses as it is (400) fails at once, as before:
/// no waiting, no asking again.
#[tokio::test]
async fn a_refused_request_still_fails_at_once() {
    let dir = temp_dir();
    let invalid = || {
        let body = json!({"error": {"code": 400, "message": "Invalid request: tools must be an array"}});
        (400, String::new(), "application/json", body.to_string())
    };
    let (_factory, mut client, mut view, outage) = outage(&dir, invalid, Duration::from_millis(100), Duration::from_secs(600)).await;
    let mut waited = false;
    until(&mut client, &mut view, "builder failed", |view| {
        let agent = view.agent("builder");
        waited |= !agent.usage_wait.is_empty();
        agent.status() == v1::AgentStatus::Idle && describe(&view.entries).iter().any(|entry| entry.contains("tools must be an array"))
    })
    .await;
    assert!(!waited);
    assert_eq!(outage.asked.load(Ordering::SeqCst), 1);
    let notices = notices(&view);
    assert!(!notices.iter().any(|notice| notice == TROUBLE || notice.contains("Retrying")), "{notices:?}");
}

/// The person's message ends a long wait at once: it asks again then,
/// with the message.
#[tokio::test]
async fn a_message_cuts_the_wait_short() {
    let dir = temp_dir();
    let (_factory, mut client, mut view, outage) = outage(&dir, overloaded, Duration::from_secs(600), Duration::from_secs(3600)).await;
    waiting(&mut client, &mut view).await;
    outage.down.store(false, Ordering::SeqCst);
    apply_request(&mut client, &mut view, send_message("builder", "still there?")).await;
    idle(&mut client, &mut view, "builder answered").await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi from openrouter")).await;
    let said = describe(&view.entries);
    assert!(said.iter().any(|entry| entry == "user: still there?"), "{said:?}");
}

/// Stop ends the wait, and the run, at once.
#[tokio::test]
async fn stop_ends_the_wait() {
    let dir = temp_dir();
    let (_factory, mut client, mut view, outage) = outage(&dir, overloaded, Duration::from_secs(600), Duration::from_secs(3600)).await;
    waiting(&mut client, &mut view).await;
    let asked = outage.asked.load(Ordering::SeqCst);
    apply_request(&mut client, &mut view, Request::StopAgent(v1::StopAgent { agent_id: "builder".into() })).await;
    idle(&mut client, &mut view, "builder stopped").await;
    assert_eq!(outage.asked.load(Ordering::SeqCst), asked, "not asked again");
}

/// A provider that keeps failing is given 12 hours, then the run ends,
/// saying so in place of its last error.
#[tokio::test]
async fn a_provider_that_keeps_failing_ends_the_run_in_time() {
    let dir = temp_dir();
    let (_factory, mut client, mut view, _outage) = outage(&dir, overloaded, Duration::from_millis(100), Duration::from_millis(500)).await;
    let stopped = "notice: OpenRouter kept failing to answer for 12 hours. Builder stopped; send a message to carry on.";
    idle(&mut client, &mut view, "builder stopped").await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == stopped)).await;
    let notices = notices(&view);
    assert_eq!(notices.last().map(String::as_str), Some(stopped), "{notices:?}");
    assert!(!notices.iter().any(|notice| notice.contains("Overloaded")), "{notices:?}");
}

fn too_many() -> Answer {
    (429, "retry-after: 2\r\n".to_owned(), "application/json", json!({"error": {"code": 429, "message": "Rate limited"}}).to_string())
}

/// A provider that says how long to wait (Retry-After) isn't asked again
/// sooner: neither by the quick retries nor after a wait, whose time says
/// when it really asks.
#[tokio::test]
async fn retry_after_is_honoured() {
    let dir = temp_dir();
    let started = std::time::Instant::now();
    let (_factory, mut client, mut view, outage) = outage(&dir, too_many, Duration::from_millis(100), Duration::from_secs(600)).await;
    waiting(&mut client, &mut view).await;
    assert!(started.elapsed() >= Duration::from_secs(4), "two quick retries, 2 s each: {:?}", started.elapsed());
    assert_eq!(outage.asked.load(Ordering::SeqCst), 3);
    let left = view.agent("builder").usage_wait_until_ms - i64::try_from(now_ms()).expect("now");
    assert!(left > 1_000, "until when the provider asked, not the 100 ms wait: {left} ms");
    outage.down.store(false, Ordering::SeqCst);
    idle(&mut client, &mut view, "builder answered").await;
    assert_eq!(outage.asked.load(Ordering::SeqCst), 4, "asked once after the wait");
}
