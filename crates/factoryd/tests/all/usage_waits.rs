//! Usage findings: how long waits last and what they say, and
//! which accounts show in use.

use std::sync::Arc;
use std::sync::atomic::AtomicI64;
use std::time::Duration;

use crate::accounts::{account, registry_over, serve};
use crate::common::{apply_request, describe, send_message, temp_dir};
use crate::usage_switch::{
    Route, claktory, claude_reply, create, models_asked, openrouter_reply, providers, providers_on, serve_routes, until,
};
use factory_agent::clock::now_ms;
use factory_agent::providers::models::CREDIT_RETRY_MS;
use factory_protocol::v1::{self, client_message::Body as Request};
use factoryd::factory::Factory;
use serde_json::json;

/// U12: an agent waiting for a key out of credit isn't asleep for good. A
/// request tries the key again by itself an hour after it ran out, and the
/// wait ends then too, so credit added meanwhile is found without the
/// person doing anything more. The notice points to Try now for sooner.
#[tokio::test]
async fn an_agent_waiting_for_credit_carries_on_when_the_key_is_tried_again() {
    let dir = temp_dir();
    let (base_url, seen) = providers(Arc::new(AtomicI64::new(0))).await;
    let file = dir.join("auth.json");
    // Out of credit just now: a whole hour to wait, however long a busy
    // machine takes to start the agent's run (a few seconds left raced it).
    let out_of_credit = |since: u64| {
        let credentials = json!({"openrouter": {"type": "api_key", "key": "sk-or-test", "factoryOutOfCredit": since}});
        std::fs::write(&file, credentials.to_string()).expect("credentials");
    };
    out_of_credit(now_ms());
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file.clone(), &base_url))).await.expect("open");
    factory.set_usage_wait_margin(Duration::ZERO);
    let (mut client, mut view) = serve(&factory, &dir).await;
    create(&mut client, &mut view, "Builder", "openrouter/anthropic/claude-opus-4.8", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;

    until(&mut client, &mut view, "builder waiting for credit", |view| !view.agent("builder").usage_wait.is_empty()).await;
    let agent = view.agent("builder").clone();
    assert_eq!(agent.usage_wait, "Waiting for you to add credit or another OpenRouter account");
    assert_eq!(agent.usage_wait_until_ms, 0, "nothing says when credit comes");
    assert_eq!(models_asked(&seen, "/chat/completions"), Vec::<String>::new(), "the key isn't tried before its hour is up");
    view.follow(&mut client, |view| describe(&view.entries).len() == 2).await;
    let notice = describe(&view.entries)[1].clone();
    assert!(
        notice.ends_with("Builder will carry on once you add credit or another account. After adding credit, choose Try now."),
        "{notice}"
    );

    // Its hour nearly up: a change to the settings wakes the wait, which
    // finds the key still out of credit for a few seconds, waits them out
    // by itself and tries it again.
    let since = now_ms() - CREDIT_RETRY_MS + 3_000;
    out_of_credit(since);
    let nudge = v1::UpdateSettings { user_name: Some("Matt".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(nudge)).await;

    // The hour is up: it asks again by itself, and the key has credit now.
    until(&mut client, &mut view, "builder answered", |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
    assert!(now_ms() >= since + CREDIT_RETRY_MS, "not before the key is tried again");
    assert_eq!(models_asked(&seen, "/chat/completions"), vec!["anthropic/claude-opus-4.8".to_owned()]);
    assert!(describe(&view.entries).iter().any(|entry| entry == "assistant: hi from openrouter"), "{:?}", describe(&view.entries));
    let openrouter = view.workspace.providers.iter().find(|provider| provider.id == "openrouter").expect("OpenRouter").clone();
    assert!(!openrouter.accounts[0].out_of_credit, "{openrouter:?}");
}

fn in_use(view: &crate::common::View, provider: &str) -> bool {
    view.workspace.providers.iter().find(|known| known.id == provider).expect("the provider").accounts.iter().any(|account| account.in_use)
}

/// U1: an account shows in use only while an agent or chat has a model on
/// its provider (Fable's own pool aside), not because it has room.
#[tokio::test]
async fn only_a_provider_an_agent_runs_on_shows_in_use() {
    let reset = Arc::new(AtomicI64::new(i64::try_from(now_ms() / 1000).expect("now") - 1));
    let (_dir, _factory, mut client, mut view, _seen) = claktory(true, reset).await;
    // Rivet, on Claude.
    assert!(in_use(&view, "anthropic") && !in_use(&view, "openrouter"), "OpenRouter has room, but nobody runs on it");

    let set = |id: &str, model: &str| {
        Request::UpdateAgent(v1::UpdateAgent { agent_id: id.into(), model: Some(model.into()), ..Default::default() })
    };
    apply_request(&mut client, &mut view, set("rivet", "openrouter/anthropic/claude-opus-4.8")).await;
    assert!(!in_use(&view, "anthropic") && in_use(&view, "openrouter"));

    // Fable draws on a pool of its own: not what in_use is about.
    create(&mut client, &mut view, "Builder", "anthropic/claude-fable-5", "low").await;
    assert!(!in_use(&view, "anthropic"));
    apply_request(&mut client, &mut view, set("builder", "anthropic/claude-opus-4-8")).await;
    assert!(in_use(&view, "anthropic") && in_use(&view, "openrouter"));

    apply_request(&mut client, &mut view, set("builder", "openrouter/anthropic/claude-opus-4.8")).await;
    assert!(!in_use(&view, "anthropic"));

    // A chat counts as an agent does.
    let ask = Request::CreateChat(v1::CreateChat { text: "hello".into(), client_id: "c1".into() });
    apply_request(&mut client, &mut view, ask).await;
    let chat = view.workspace.chats.first().expect("the chat").model.clone();
    let on = chat.split_once('/').expect("a provider").0.to_owned();
    assert!(in_use(&view, &on), "{chat}");
}

fn provider(view: &crate::common::View, id: &str) -> v1::Provider {
    view.workspace.providers.iter().find(|known| known.id == id).expect("the provider").clone()
}

/// U10: Fable's own limit shows on the account, apart from the normal
/// pool's, and so does which account Fable agents use; each provider says
/// how many agents and chats run on it.
#[tokio::test]
async fn fables_own_limit_and_use_show_per_account() {
    let reset = Arc::new(AtomicI64::new(0));
    let (_dir, _factory, mut client, mut view, _seen) = claktory(true, reset).await;
    let (claude, openrouter) = (provider(&view, "anthropic"), provider(&view, "openrouter"));
    assert_eq!((claude.agents, openrouter.agents), (1, 0), "Rivet");
    assert!(claude.accounts[0].in_use && !claude.accounts[0].fable_in_use, "nobody on Fable");

    create(&mut client, &mut view, "Builder", "anthropic/claude-fable-5", "low").await;
    let claude = provider(&view, "anthropic");
    assert_eq!(claude.agents, 2);
    assert!(claude.accounts[0].in_use && claude.accounts[0].fable_in_use);

    // Fable's limit: its own pool's mark, the normal pool still has room.
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder waiting", |view| !view.agent("builder").usage_wait.is_empty()).await;
    until(&mut client, &mut view, "Fable's mark shown", |view| provider(view, "anthropic").accounts[0].fable_exhausted_until > 0).await;
    let account = provider(&view, "anthropic").accounts[0].clone();
    assert_eq!(account.fable_exhausted_until, view.agent("builder").usage_wait_until_ms);
    assert_eq!(account.exhausted_until, 0, "the normal pool has room");
    assert!(account.in_use && !account.fable_in_use, "{account:?}");
    assert_eq!(provider(&view, "openrouter").accounts[0].fable_exhausted_until, 0);
}

/// An OpenRouter stand-in whose first chat request finds the key's
/// in-flight budget full, free again in `seconds` (Retry-After); every
/// later one is answered.
async fn in_flight_once(seconds: u64) -> String {
    let chats = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let route: Route = Arc::new(move |path, _| {
        if !path.ends_with("/chat/completions") {
            (404, String::new(), "application/json", "{}".to_owned())
        } else if chats.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
            let body = json!({"error": {"code": 402, "message": "This request would exceed your available credits given your current in-flight requests.", "metadata": {"reason": "in_flight_budget_exhausted", "limit_source": "openrouter_in_flight_budget"}}});
            (402, format!("retry-after: {seconds}\r\n"), "application/json", body.to_string())
        } else {
            openrouter_reply()
        }
    });
    serve_routes(tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind"), route).0
}

/// A key's limit that lifts within moments (OpenRouter's in-flight budget,
/// whose Retry-After is seconds) is waited out quietly: the wait line says
/// so, but there's no notice, no Needs you item and no other provider
/// offered.
#[tokio::test]
async fn a_limit_that_lifts_in_moments_is_waited_out_quietly() {
    let dir = temp_dir();
    let base_url = in_flight_once(4).await;
    let file = dir.join("auth.json");
    std::fs::write(&file, json!({"openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file.clone(), &base_url))).await.expect("open");
    factory.set_usage_wait_margin(Duration::ZERO);
    let (mut client, mut view) = serve(&factory, &dir).await;
    create(&mut client, &mut view, "Builder", "openrouter/anthropic/claude-opus-4.8", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;

    until(&mut client, &mut view, "builder waiting", |view| !view.agent("builder").usage_wait.is_empty()).await;
    let agent = view.agent("builder").clone();
    assert!(agent.usage_wait.starts_with("Waiting a moment for OpenRouter, until "), "{}", agent.usage_wait);
    assert!(view.workspace.provider_waits.is_empty(), "{:?}", view.workspace.provider_waits);

    until(&mut client, &mut view, "builder answered", |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|entry| entry == "assistant: hi from openrouter")).await;
    assert_eq!(describe(&view.entries), ["user: hello", "assistant: hi from openrouter"], "no notice");
}

/// A run whose model calls can't connect doesn't fail after the
/// quick retries: it waits for the network, says so, and carries on by
/// itself once it's back.
#[tokio::test]
async fn an_agent_that_cant_connect_waits_for_the_network_and_carries_on() {
    let dir = temp_dir();
    // Nothing listens there yet: every request is refused.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let base_url = format!("http://{addr}");
    let file = dir.join("auth.json");
    std::fs::write(&file, json!({"openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file.clone(), &base_url))).await.expect("open");
    factory.set_network_wait(Duration::from_millis(100));
    factory.set_network_up(Some(false));
    let (mut client, mut view) = serve(&factory, &dir).await;
    create(&mut client, &mut view, "Builder", "openrouter/anthropic/claude-opus-4.8", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;

    until(&mut client, &mut view, "builder waiting for the network", |view| view.agent("builder").usage_wait == "Waiting for the network…")
        .await;
    let agent = view.agent("builder").clone();
    assert_eq!(agent.usage_wait_until_ms, 0);
    assert!(agent.usage_wait_alternative.is_none() && !agent.usage_wait_no_other_provider);
    assert_eq!(agent.status(), v1::AgentStatus::Working, "the run goes on");
    view.follow(&mut client, |view| {
        describe(&view.entries)
            .iter()
            .any(|entry| entry == "notice: Lost the connection to OpenRouter. Builder carries on by itself once it's back.")
    })
    .await;

    // The network looks back, but OpenRouter still can't be reached: it
    // waits again, each time, without the quick retries again (two more
    // notices on every look).
    factory.set_network_up(Some(true));
    tokio::time::sleep(Duration::from_millis(1500)).await;
    // A round trip, so the view has everything said meanwhile.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let retries = |view: &crate::common::View| {
        describe(&view.entries).iter().filter(|entry| entry.starts_with("notice: The model request failed. Retrying")).count()
    };
    assert_eq!(retries(&view), 2, "{:?}", describe(&view.entries));

    // The network is back, and OpenRouter with it.
    let listener = tokio::net::TcpListener::bind(addr).await.expect("the same address");
    let (_, seen) = providers_on(listener, Arc::new(AtomicI64::new(0)));
    factory.set_network_up(Some(true));
    until(&mut client, &mut view, "builder answered", |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
    assert!(describe(&view.entries).iter().any(|entry| entry == "assistant: hi from openrouter"), "{:?}", describe(&view.entries));
    assert_eq!(models_asked(&seen, "/chat/completions").len(), 1);
}

/// An address nothing listens on: every request to it is refused.
async fn refusing() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    listener.local_addr().expect("addr")
}

/// A Factory whose Builder runs on OpenRouter at `addr`, giving up on a
/// provider that can't be reached while the network works after `give_up`,
/// and has said hello: it waits for the network.
async fn cut_off(
    dir: &std::path::Path,
    addr: std::net::SocketAddr,
    up: bool,
    give_up: Duration,
) -> (Arc<Factory>, crate::common::Client, crate::common::View) {
    let file = dir.join("auth.json");
    std::fs::write(&file, json!({"openrouter": {"type": "api_key", "key": "sk-or-test"}}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(dir), Arc::new(registry_over(file, &format!("http://{addr}")))).await.expect("open");
    factory.set_network_wait(Duration::from_millis(100));
    factory.set_network_give_up(give_up);
    factory.set_network_up(Some(up));
    let (mut client, mut view) = serve(&factory, dir).await;
    create(&mut client, &mut view, "Builder", "openrouter/anthropic/claude-opus-4.8", "high").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    until(&mut client, &mut view, "builder waiting for the network", |view| view.agent("builder").usage_wait == "Waiting for the network…")
        .await;
    (factory, client, view)
}

/// An outage of any length is waited out (a night, say), and
/// the time it lasted doesn't count against the provider once the network
/// is back but the provider isn't yet.
#[tokio::test]
async fn an_outage_is_waited_out_however_long_it_lasts() {
    let dir = temp_dir();
    let addr = refusing().await;
    let (factory, mut client, mut view) = cut_off(&dir, addr, false, Duration::from_secs(5)).await;

    // Longer than it gives a provider that can't be reached.
    tokio::time::sleep(Duration::from_secs(6)).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    let agent = view.agent("builder").clone();
    assert_eq!((agent.status(), agent.usage_wait.as_str()), (v1::AgentStatus::Working, "Waiting for the network…"));

    // The network's back, OpenRouter not quite yet: a request fails, and
    // it waits on.
    factory.set_network_up(Some(true));
    tokio::time::sleep(Duration::from_millis(500)).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    assert_eq!(view.agent("builder").status(), v1::AgentStatus::Working, "{:?}", describe(&view.entries));

    // Then OpenRouter too: it carries on.
    let listener = tokio::net::TcpListener::bind(addr).await.expect("the same address");
    let (_, seen) = providers_on(listener, Arc::new(AtomicI64::new(0)));
    until(&mut client, &mut view, "builder answered", |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
    let said = describe(&view.entries);
    assert!(said.iter().any(|entry| entry == "assistant: hi from openrouter"), "{said:?}");
    assert!(!said.iter().any(|entry| entry.contains("stopped")), "{said:?}");
    assert_eq!(models_asked(&seen, "/chat/completions").len(), 1);
}

/// A provider that can't be reached while the network works is given an
/// hour, then the run ends, saying so; a message carries it on.
#[tokio::test]
async fn a_provider_that_cant_be_reached_while_the_network_works_ends_the_run_in_time() {
    let dir = temp_dir();
    let addr = refusing().await;
    let (_factory, mut client, mut view) = cut_off(&dir, addr, true, Duration::from_millis(500)).await;
    until(&mut client, &mut view, "builder stopped", |view| {
        let agent = view.agent("builder");
        agent.status() == v1::AgentStatus::Idle && agent.usage_wait.is_empty()
    })
    .await;
    view.follow(&mut client, |view| {
        describe(&view.entries).iter().any(|entry| {
            entry == "notice: Couldn't reach OpenRouter for an hour, though the network works. Builder stopped; send a message to carry on."
        })
    })
    .await;
    // One notice for the stop: it says why, in place of the last connection error.
    let notices: Vec<String> = describe(&view.entries).into_iter().filter(|entry| entry.starts_with("notice: ")).collect();
    assert_eq!(
        notices.last().map(String::as_str),
        Some("notice: Couldn't reach OpenRouter for an hour, though the network works. Builder stopped; send a message to carry on."),
        "{notices:?}"
    );
    assert!(!notices.iter().any(|notice| notice.contains("Connection error")), "{notices:?}");
}

/// U11: Claude's weekly limit of Opus models alone puts only the Opus
/// agents to wait. A Sonnet agent on the same account carries on, before
/// and after the usage read that shows the Opus limit at 100%, and the Opus
/// agent's notice says which limit it is.
#[tokio::test]
async fn an_opus_limit_puts_only_opus_agents_to_wait() {
    let dir = temp_dir();
    let resets = now_ms() / 1000 + 3 * 24 * 3600;
    let at = jiff::Timestamp::from_second(i64::try_from(resets).expect("seconds")).expect("a time").to_string();
    let route: Route = Arc::new(move |path, body| {
        let json = |body: serde_json::Value| (200, String::new(), "application/json", body.to_string());
        if path.starts_with("/api/oauth/usage") {
            json(json!({
                "five_hour": {"utilization": 3.0, "resets_at": at},
                "seven_day": {"utilization": 40.0, "resets_at": at},
                "seven_day_opus": {"utilization": 100.0, "resets_at": at},
            }))
        } else if path.ends_with("/api/oauth/profile") {
            json(json!({"account": {"uuid": "uuid-a", "email": "a@x.com"}, "organization": {"uuid": "org-a"}}))
        } else if body["model"].as_str().is_some_and(|model| model.contains("opus")) {
            let headers = format!(
                "anthropic-ratelimit-unified-status: rejected\r\nanthropic-ratelimit-unified-reset: {resets}\r\nanthropic-ratelimit-unified-representative-claim: seven_day_opus\r\n"
            );
            (
                429,
                headers,
                "application/json",
                json!({"type": "error", "error": {"type": "rate_limit_error", "message": "Error"}}).to_string(),
            )
        } else {
            claude_reply()
        }
    });
    let (base_url, seen) = serve_routes(tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind"), route);
    let file = dir.join("auth.json");
    let mut claude = account("a");
    claude["factoryLabel"] = json!("a@x.com");
    std::fs::write(&file, json!({"anthropic": claude}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file.clone(), &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;
    let sonnet = factory_agent::providers::catalog::models("anthropic")
        .into_iter()
        .find(|model| model.id.contains("sonnet"))
        .expect("a Sonnet model")
        .id;
    create(&mut client, &mut view, "Builder", "anthropic/claude-opus-4-8", "low").await;
    create(&mut client, &mut view, "Helper", &format!("anthropic/{sonnet}"), "low").await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "builder".into() })).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;

    until(&mut client, &mut view, "builder waiting", |view| !view.agent("builder").usage_wait.is_empty()).await;
    let wait = view.agent("builder").usage_wait.clone();
    assert!(wait.starts_with("Waiting for your Claude usage to reset at "), "{wait}");
    view.follow(&mut client, |view| describe(&view.entries).len() == 2).await;
    let notice = describe(&view.entries)[1].clone();
    assert!(notice.starts_with("notice: Your Claude account reached its weekly Opus limit; it resets "), "{notice}");
    let answered = |view: &crate::common::View| {
        let helper = view.agent("helper");
        helper.status() == v1::AgentStatus::Idle && helper.usage_wait.is_empty()
    };
    until(&mut client, &mut view, "helper answered", answered).await;

    // The usage read says the Opus limit is full: the Sonnet agent still
    // carries on, the Opus one still waits.
    until(&mut client, &mut view, "the Opus limit read", |view| {
        view.workspace.providers.iter().filter(|provider| provider.id == "anthropic").flat_map(|provider| &provider.accounts).any(
            |account| {
                account
                    .usage
                    .as_ref()
                    .is_some_and(|usage| usage.windows.iter().any(|window| window.id == "weekly_opus" && window.used_percent >= 100.0))
            },
        )
    })
    .await;
    let sonnet_asked = || models_asked(&seen, "/v1/messages").iter().filter(|model| model.contains("sonnet")).count();
    assert_eq!(sonnet_asked(), 1);
    apply_request(&mut client, &mut view, send_message("helper", "again")).await;
    until(&mut client, &mut view, "helper answered again", |view| answered(view) && sonnet_asked() == 2).await;
    assert!(!view.agent("builder").usage_wait.is_empty(), "Opus still waits");
}
