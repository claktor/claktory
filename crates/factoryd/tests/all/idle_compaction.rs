//! Idle compaction: an agent left idle with a long conversation compacts it
//! just before its prompt cache expires (after 58 minutes for an hour's
//! cache, the default; after 4 for a 5-minute one, only when asked), once,
//! quietly, and never over new work.

use std::time::Duration;

use crate::common::{Client, LONG_MODEL, Scripted, View, apply_request, create_agent, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};

const MINUTE: Duration = Duration::from_secs(60);

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn settings(settings: v1::UpdateSettings) -> client_message::Body {
    client_message::Body::UpdateSettings(settings)
}

fn hour_cache() -> client_message::Body {
    settings(v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: "anthropic".into(), ttl: "1h".into() }), ..Default::default() })
}

fn five_minute_cache() -> client_message::Body {
    settings(v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: "anthropic".into(), ttl: "5m".into() }), ..Default::default() })
}

fn compactions(view: &View) -> Vec<v1::Compaction> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Compaction(compaction)) => Some(compaction.clone()),
            _ => None,
        })
        .collect()
}

/// A Factory whose agent Builder works on `LONG_MODEL` and has answered
/// `first`; the idle check runs every 20 ms.
async fn setup() -> (std::sync::Arc<factoryd::Factory>, Client, View, std::sync::Arc<std::sync::Mutex<Vec<crate::common::Request>>>) {
    let dir = temp_dir();
    let models = Scripted::signed_in().with_long_model();
    let requests = models.requests.clone();
    let (addr, factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    let model =
        v1::UpdateAgent { agent_id: "builder".into(), model: Some(LONG_MODEL.into()), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(model)).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    (factory, client, view, requests)
}

/// Builder answers `text`, with the idle clock set to the real one, so
/// its response is stamped now.
async fn talk(factory: &factoryd::Factory, client: &mut Client, view: &mut View, text: &str) {
    factory.set_idle_clock_ahead(Duration::ZERO);
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", text)).await;
    view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, "builder")).await;
}

/// Lets the idle check run a few times.
async fn settle(factory: &factoryd::Factory, client: &mut Client, view: &mut View) {
    factory.set_idle_check_every(Duration::from_millis(20));
    drain(client, view, Duration::from_millis(300)).await;
}

/// Applies what the server sends for `time`.
async fn drain(client: &mut Client, view: &mut View, time: Duration) {
    let until = tokio::time::Instant::now() + time;
    while tokio::time::Instant::now() < until {
        tokio::select! {
            message = client.receive() => if let Some(body) = message.body { view.apply(&body) },
            () = tokio::time::sleep_until(until) => {}
        }
    }
}

/// Waits until `done`, which may look beyond what the server sends.
async fn until(client: &mut Client, view: &mut View, mut done: impl FnMut(&View) -> bool) {
    for _ in 0..500 {
        if done(view) {
            return;
        }
        drain(client, view, Duration::from_millis(20)).await;
    }
    panic!("it never happened: {:?}", crate::common::describe(&view.entries));
}

#[tokio::test]
async fn an_hour_s_cache_compacts_after_58_idle_minutes_once_and_quietly() {
    let (factory, mut client, mut view, requests) = setup().await;
    // An hour unless set otherwise: nothing in the settings says so.
    let ttls = view.workspace.settings.as_ref().map(|settings| settings.cache_ttls.clone()).unwrap_or_default();
    assert!(ttls.is_empty(), "{ttls:?}");
    apply_request(&mut client, &mut view, hour_cache()).await;
    let provider = view.workspace.providers.iter().find(|provider| provider.id == "scripted").map(|provider| provider.cache_ttl.clone());
    assert_eq!(provider.as_deref(), Some(""), "a provider whose cache lifetime Claktory can't choose");
    let ttls = view.workspace.settings.as_ref().map(|settings| settings.cache_ttls.clone()).unwrap_or_default();
    assert_eq!(ttls, [v1::CacheTtl { key: "anthropic".into(), ttl: "1h".into() }]);
    // Each provider's own lifetimes only; none for one whose requests can't choose.
    for (key, ttl) in [("openai", "1h"), ("anthropic", "2h"), ("anthropic", "30m"), ("openai-codex", "30m")] {
        let wrong = v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: key.into(), ttl: ttl.into() }), ..Default::default() };
        let refused = apply_request(&mut client, &mut view, settings(wrong)).await;
        assert!(matches!(&refused, Body::Error(_)), "{key} {ttl}: {refused:?}");
    }
    // OpenAI's own aren't written yet either: the backend a rollback goes
    // back to refuses a config.json that has them.
    for (key, ttl) in [("openai", "12h"), ("openai", "30m"), ("openai/gpt-5.6-sol", "12h")] {
        let openai = v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: key.into(), ttl: ttl.into() }), ..Default::default() };
        let refused = apply_request(&mut client, &mut view, settings(openai)).await;
        assert!(matches!(&refused, Body::Error(error) if error.message.contains("can't be chosen yet")), "{key} {ttl}: {refused:?}");
    }
    let unset = v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: "openai".into(), ttl: String::new() }), ..Default::default() };
    apply_request(&mut client, &mut view, settings(unset)).await;
    let ttls = view.workspace.settings.as_ref().map(|settings| settings.cache_ttls.clone()).unwrap_or_default();
    assert_eq!(ttls, [v1::CacheTtl { key: "anthropic".into(), ttl: "1h".into() }], "nothing of OpenAI's written");
    // Back to the default: still an hour.
    let unset = v1::UpdateSettings { cache_ttl: Some(v1::CacheTtl { key: "anthropic".into(), ttl: String::new() }), ..Default::default() };
    apply_request(&mut client, &mut view, settings(unset)).await;
    assert!(view.workspace.settings.as_ref().is_some_and(|settings| settings.cache_ttls.is_empty()));
    talk(&factory, &mut client, &mut view, "hello").await;
    assert_eq!(requests.lock().expect("lock")[0].cache_ttl, "1h", "requests ask for an hour's cache");

    // 57 minutes: not yet.
    factory.set_idle_clock_ahead(57 * MINUTE);
    settle(&factory, &mut client, &mut view).await;
    assert!(compactions(&view).is_empty());

    // 58: summarized while idle, and that's all it does: no news of a run
    // for the person while they look elsewhere.
    let select = |agent: &str| client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() });
    apply_request(&mut client, &mut view, select("rivet")).await;
    factory.set_idle_clock_ahead(58 * MINUTE);
    factory.set_idle_check_every(Duration::from_millis(20));
    until(&mut client, &mut view, |view| requests.lock().expect("lock").len() == 2 && idle(view, "builder")).await;
    settle(&factory, &mut client, &mut view).await;
    assert!(!view.agent("builder").unseen, "nothing new to see");
    apply_request(&mut client, &mut view, select("builder")).await;
    let [compaction] = compactions(&view).try_into().expect("one compaction");
    assert!(compaction.idle, "{compaction:?}");
    assert_eq!(compaction.tokens_before, 160_200);
    assert_eq!(requests.lock().expect("lock").len(), 2, "the answer, then the summary; no run after it");
    let last = view.entries.last().expect("an entry");
    assert!(matches!(last.kind, Some(Kind::Compaction(_))), "nothing after the divider: {last:?}");

    // Once: the summary isn't compacted again.
    factory.set_idle_clock_ahead(116 * MINUTE);
    settle(&factory, &mut client, &mut view).await;
    assert_eq!(compactions(&view).len(), 1);

    // The next message carries on from the summary, told it was summarized while idle.
    talk(&factory, &mut client, &mut view, "back again").await;
    let compacted = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Assistant(reply)) => Some(reply.text.clone()),
        _ => None,
    });
    assert_eq!(compacted.as_deref(), Some("echo: back again"));
}

#[tokio::test]
async fn a_five_minute_cache_compacts_only_when_asked_and_never_late_or_when_off() {
    let (factory, mut client, mut view, _) = setup().await;
    apply_request(&mut client, &mut view, five_minute_cache()).await;
    talk(&factory, &mut client, &mut view, "hello").await;
    // The provider's own 5 minutes, chosen: not unless asked.
    factory.set_idle_clock_ahead(4 * MINUTE);
    settle(&factory, &mut client, &mut view).await;
    assert!(compactions(&view).is_empty());

    // Asked, after 4 minutes.
    let short = v1::UpdateSettings { idle_compaction_short_cache: Some(true), ..Default::default() };
    apply_request(&mut client, &mut view, settings(short)).await;
    assert_eq!(view.workspace.settings.as_ref().map(|settings| settings.idle_compaction_short_cache), Some(true));
    talk(&factory, &mut client, &mut view, "again").await;
    factory.set_idle_clock_ahead(4 * MINUTE);
    factory.set_idle_check_every(Duration::from_millis(20));
    view.follow(&mut client, |view| !compactions(view).is_empty() && idle(view, "builder")).await;

    // Late (the machine slept): left as it is.
    talk(&factory, &mut client, &mut view, "and again").await;
    factory.set_idle_clock_ahead(10 * MINUTE);
    settle(&factory, &mut client, &mut view).await;
    assert_eq!(compactions(&view).len(), 1);

    // Turned off: never.
    let off = v1::UpdateSettings { idle_compaction: Some(false), ..Default::default() };
    apply_request(&mut client, &mut view, settings(off)).await;
    talk(&factory, &mut client, &mut view, "once more").await;
    factory.set_idle_clock_ahead(4 * MINUTE);
    settle(&factory, &mut client, &mut view).await;
    assert_eq!(compactions(&view).len(), 1);
}

#[tokio::test]
async fn new_work_or_stop_cancels_it_and_the_conversation_stays() {
    let (factory, mut client, mut view, requests) = setup().await;
    apply_request(&mut client, &mut view, hour_cache()).await;
    // Its summary never finishes, so it's still compacting when work comes.
    talk(&factory, &mut client, &mut view, "a slow summary, please").await;
    factory.set_idle_clock_ahead(58 * MINUTE);
    factory.set_idle_check_every(Duration::from_millis(20));
    until(&mut client, &mut view, |view| {
        requests.lock().expect("lock").len() == 2 && view.agent("builder").status() == AgentStatus::Working
    })
    .await;
    talk(&factory, &mut client, &mut view, "hello").await;
    assert!(compactions(&view).is_empty(), "the conversation stays as it was");
    let answered = view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Assistant(reply)) if reply.text == "echo: hello"));
    assert!(answered, "the message got its answer");

    // Stopped while compacting: nothing changes, and it isn't tried again.
    factory.set_idle_clock_ahead(58 * MINUTE);
    until(&mut client, &mut view, |_| requests.lock().expect("lock").len() == 4).await;
    let reply = apply_request(&mut client, &mut view, client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() })).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| idle(view, "builder")).await;
    settle(&factory, &mut client, &mut view).await;
    assert!(compactions(&view).is_empty());
    assert_eq!(requests.lock().expect("lock").len(), 4);
}
