//! Agents waiting on a provider's usage limit, grouped by provider: one
//! Needs you item (Workspace.provider_waits) and one notification, not one
//! per agent, and not again when they wake for a moment and wait again, nor
//! after a restart.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::accounts::{account, anthropic_with, registry_over, serve};
use crate::common::{Client, View, apply_request, send_message, temp_dir};
use factory_agent::clock::now_ms;
use factory_protocol::v1::{self, NotifyKind, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use serde_json::json;

/// Follows the Workspace until `done`, keeping the notifications on the way.
async fn follow(client: &mut Client, view: &mut View, notes: &mut Vec<v1::Notify>, what: &str, mut done: impl FnMut(&View) -> bool) {
    let waited = tokio::time::timeout(std::time::Duration::from_secs(45), async {
        while !done(view) {
            let message = client.receive().await;
            let body = message.body.as_ref().expect("body");
            if let Body::Notify(notify) = body {
                notes.push(notify.clone());
            }
            view.apply(body);
        }
    })
    .await;
    assert!(waited.is_ok(), "still waiting for {what}: {:?}", view.workspace.provider_waits);
}

async fn create(client: &mut Client, view: &mut View, name: &str) {
    // A model of the normal pool, as the account tests use.
    let model = factory_agent::providers::catalog::models("anthropic")
        .into_iter()
        .find(|model| !model.id.starts_with("claude-fable"))
        .expect("a Claude model")
        .id;
    let create = Request::CreateAgent(v1::CreateAgent {
        name: name.into(),
        model: format!("anthropic/{model}"),
        effort: "off".into(),
        ..Default::default()
    });
    assert!(matches!(apply_request(client, view, create).await, Body::Ack(_)));
}

fn waiting(view: &View, id: &str) -> bool {
    view.workspace.agents.iter().any(|agent| agent.id == id && !agent.usage_wait.is_empty())
}

fn provider_notes(notes: &[v1::Notify]) -> Vec<&v1::Notify> {
    notes.iter().filter(|note| note.thread.starts_with("provider:")).collect()
}

#[tokio::test]
async fn agents_waiting_on_a_limit_make_one_item_and_one_notification() {
    let dir = temp_dir();
    let resets = now_ms() / 1000 + 3 * 3600;
    let limited = Arc::new(AtomicBool::new(true));
    let (base_url, _seen) = anthropic_with(resets, limited.clone()).await;
    let file = dir.join("auth.json");
    std::fs::write(&file, json!({"anthropic": account("a")}).to_string()).expect("credentials");
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file.clone(), &base_url))).await.expect("open");
    let (mut client, mut view) = serve(&factory, &dir).await;
    let mut notes = Vec::new();
    assert!(view.workspace.provider_waits.is_empty());

    create(&mut client, &mut view, "Builder").await;
    create(&mut client, &mut view, "Helper").await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    follow(&mut client, &mut view, &mut notes, "builder waiting", |view| waiting(view, "builder")).await;
    apply_request(&mut client, &mut view, send_message("helper", "hello")).await;
    follow(&mut client, &mut view, &mut notes, "both in one item", |view| {
        view.workspace.provider_waits.first().is_some_and(|wait| wait.agent_ids.len() == 2)
    })
    .await;

    let [wait] = view.workspace.provider_waits.as_slice() else { panic!("{:?}", view.workspace.provider_waits) };
    let at = factory_agent::accounts::local_time(resets * 1000).expect("a local time");
    assert_eq!(
        (wait.provider_id.as_str(), wait.provider_name.as_str(), wait.credit, wait.until_ms),
        ("anthropic", "Claude", false, i64::try_from(resets * 1000).expect("ms"))
    );
    assert_eq!((wait.agent_ids.clone(), wait.chat_ids.len()), (vec!["builder".to_owned(), "helper".to_owned()], 0));
    assert_eq!(wait.title, "Claude usage limit reached");
    assert_eq!(wait.text, format!("Builder and Helper wait until it resets at {at}."));

    // One notification, when Builder started waiting; Helper joining it adds none.
    let name = view.workspace.settings.clone().expect("settings").name;
    let [note] = provider_notes(&notes)[..] else { panic!("{notes:?}") };
    assert_eq!(note.kind(), NotifyKind::NeedsYou);
    assert_eq!(note.title, name);
    assert_eq!(note.body, format!("Claude usage limit reached. Builder waits until it resets at {at}."));
    assert_eq!((note.agent_id.as_str(), note.thread.as_str()), ("builder", "provider:anthropic"));

    // A message wakes them; they ask, find the limit, and wait again: the
    // item comes back, with no second notification.
    apply_request(&mut client, &mut view, send_message("builder", "still there?")).await;
    follow(&mut client, &mut view, &mut notes, "the item gone a moment", |view| view.workspace.provider_waits.is_empty()).await;
    follow(&mut client, &mut view, &mut notes, "the item back", |view| {
        view.workspace.provider_waits.first().is_some_and(|wait| wait.agent_ids.len() == 2)
    })
    .await;
    assert_eq!(provider_notes(&notes).len(), 1, "{notes:?}");

    // Restarted, the runs resume and wait again: the same wait, so nothing
    // notifies. (The first Claktory runs on in this process, waiting too.)
    let factory = Factory::open(crate::common::home(&dir), Arc::new(registry_over(file, &base_url))).await.expect("open again");
    let (mut client, mut view) = serve(&factory, &dir).await;
    let mut notes = Vec::new();
    follow(&mut client, &mut view, &mut notes, "the item after the restart", |view| {
        view.workspace.provider_waits.first().is_some_and(|wait| wait.agent_ids.len() == 2)
    })
    .await;
    assert!(provider_notes(&notes).is_empty(), "{notes:?}");

    // The limit lifts: the item goes.
    limited.store(false, Ordering::SeqCst);
    let try_now = Request::TryAgainNow(v1::TryAgainNow { agent_id: "helper".into() });
    assert!(matches!(apply_request(&mut client, &mut view, try_now).await, Body::Ack(_)));
    follow(&mut client, &mut view, &mut notes, "nobody waiting", |view| {
        view.workspace.provider_waits.is_empty() && !waiting(view, "builder") && !waiting(view, "helper")
    })
    .await;

    // Moved to another provider's model while it waits, Builder waits on
    // nothing there: no OpenRouter item, no notification.
    limited.store(true, Ordering::SeqCst);
    apply_request(&mut client, &mut view, send_message("builder", "once more")).await;
    follow(&mut client, &mut view, &mut notes, "builder waiting again", |view| {
        view.workspace.provider_waits.first().is_some_and(|wait| wait.agent_ids.iter().any(|agent| agent == "builder"))
    })
    .await;
    let mut elsewhere = Vec::new();
    // The mock answers every provider: OpenRouter has room, so a wait shown
    // under it could only be the Claude one carried over.
    limited.store(false, Ordering::SeqCst);
    let moved = Request::UpdateAgent(v1::UpdateAgent {
        agent_id: "builder".into(),
        model: Some("openrouter/openai/gpt-4o-mini".into()),
        ..Default::default()
    });
    // (What comes before its Ack counts too.)
    let reply = client.request(moved).await;
    for body in reply.events.iter().filter_map(|event| event.body.as_ref()) {
        if let Body::Notify(notify) = body {
            notes.push(notify.clone());
        }
        view.apply(body);
        elsewhere.extend(view.workspace.provider_waits.iter().filter(|wait| wait.provider_id != "anthropic").cloned());
    }
    assert!(matches!(reply.body, Body::Ack(_)), "{:?}", reply.body);
    follow(&mut client, &mut view, &mut notes, "builder out of the Claude item", |view| {
        elsewhere.extend(view.workspace.provider_waits.iter().filter(|wait| wait.provider_id != "anthropic").cloned());
        !view.workspace.provider_waits.iter().any(|wait| wait.agent_ids.iter().any(|agent| agent == "builder"))
    })
    .await;
    assert!(elsewhere.is_empty(), "{elsewhere:?}");
    assert!(provider_notes(&notes).iter().all(|note| note.thread == "provider:anthropic"), "{notes:?}");
}
