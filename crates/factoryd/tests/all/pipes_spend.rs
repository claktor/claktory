//! A liaison's daily spend cap and contained agents' card limit
//! (pipes-automation H4): past its cap, a liaison's pipe messages wait
//! instead of waking it, counted for the person, until midnight or a higher
//! cap; an agent limited to a pipe shows the person at most 3 questions an
//! hour, the rest held back as a count.

use std::sync::Arc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, send_message, start_with_models, temp_dir};
use crate::pipes::{events, idle, incoming, select, shown};
use factory_protocol::v1::{self, client_message, entry::Kind, server_message::Body};
use factoryd::factory::Factory;
use factoryd::pipe_link::Liaison;
use factoryd::pipes;

/// A project "Site" with Lea, an open pipe, and its liaison "Liaison" (id
/// "liaison") as its lead.
async fn with_liaison() -> (Arc<Factory>, Client, View, String) {
    let dir = temp_dir();
    let models = Arc::new(Scripted::signed_in());
    let (addr, factory) = start_with_models(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Lea", "")).await;
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["lea"], "lea", false);
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.lead_id == "liaison")).await;
    (factory, client, view, pipe)
}

fn cap(agent: &str, usd: f64, tokens: u64) -> client_message::Body {
    client_message::Body::SetSpendCap(v1::SetSpendCap { agent_id: agent.into(), cap_usd: usd, cap_tokens: tokens })
}

fn spend(view: &View, agent: &str) -> v1::ContainedSpend {
    view.agent(agent).spend.expect("a liaison's spend")
}

/// The person's message to `agent`, and its run to the end.
async fn run(client: &mut Client, view: &mut View, agent: &str, text: &str) {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, agent)).await;
}

#[tokio::test]
async fn a_liaison_past_its_cap_waits_until_it_has_more() {
    let (factory, mut client, mut view, pipe) = with_liaison().await;
    // Only a liaison has a spend; the scripted model has no price, so it
    // counts in tokens, against the defaults.
    assert!(view.agent("lea").spend.is_none());
    let fresh = spend(&view, "liaison");
    assert_eq!((fresh.cap_usd, fresh.cap_tokens, fresh.spent_tokens, fresh.over), (10.0, 1_000_000, 0, false));
    assert!(fresh.resumes_ms > factoryd::transcript::now_ms());

    // A cap of 1,000 tokens: one model call (1,000 in, 200 out) is past it.
    apply_request(&mut client, &mut view, cap("liaison", 0.0, 1_000)).await;
    run(&mut client, &mut view, "liaison", "hello").await;
    view.follow(&mut client, |view| spend(view, "liaison").over).await;
    assert_eq!(spend(&view, "liaison").spent_tokens, 1_200);
    assert_eq!(spend(&view, "liaison").cap_usd, 10.0, "0 keeps the default");

    // The other side hears it waits until midnight, rounded up to a quarter
    // of an hour as any wait, and nothing of why.
    factoryd::pipe_link::set_liaison_every(std::time::Duration::from_millis(600));
    let quarter = 15 * 60 * 1000;
    let midnight = spend(&view, "liaison").resumes_ms;
    let told = Liaison::Waiting { until_ms: (midnight + quarter - 1) / quarter * quarter };
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), Some(told));
    pipes::liaison_told_for_test(&factory, &pipe, told);
    // A usage limit with no known end meanwhile: no time, the later.
    pipes::set_usage_wait_for_test(&factory, "liaison", Some(0));
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), Some(Liaison::Waiting { until_ms: 0 }));
    pipes::set_usage_wait_for_test(&factory, "liaison", None);
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), None, "midnight again, as told");

    // The other side's message waits, counted, and doesn't wake it.
    let before = view.entries.len();
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Can we talk about the launch?")).expect("received");
    view.follow(&mut client, |view| spend(view, "liaison").held_messages == 1).await;
    assert!(idle(&view, "liaison"));
    assert!(events(&view).is_empty(), "{:?}", events(&view));
    assert_eq!(view.entries.len(), before);

    // The person's own message still reaches it.
    run(&mut client, &mut view, "liaison", "are you there?").await;
    assert!(events(&view).is_empty());

    // A higher cap: the waiting message goes to it now.
    apply_request(&mut client, &mut view, cap("liaison", 0.0, 1_000_000)).await;
    view.follow(&mut client, |view| !events(view).is_empty() && idle(view, "liaison")).await;
    assert!(events(&view)[0].text.contains("Can we talk about the launch?"), "{:?}", events(&view));
    view.follow(&mut client, |view| spend(view, "liaison").held_messages == 0 && !spend(view, "liaison").over).await;
    // And the other side hears it's awake.
    tokio::time::sleep(std::time::Duration::from_millis(700)).await;
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), Some(Liaison::Awake));
}

#[tokio::test]
async fn only_an_agent_in_the_workspace_has_a_cap_set() {
    let (_factory, mut client, mut view, _pipe) = with_liaison().await;
    for refused in [cap("nobody", 5.0, 0), cap("liaison", -1.0, 0), cap("liaison", f64::NAN, 0)] {
        let reply = client.request(refused).await.body;
        assert!(matches!(reply, Body::Error(_)), "{reply:?}");
    }
    apply_request(&mut client, &mut view, cap("liaison", 2.5, 0)).await;
    view.follow(&mut client, |view| spend(view, "liaison").cap_usd == 2.5).await;
}

/// `agent` asks; what the tool said.
async fn ask(client: &mut Client, view: &mut View, agent: &str, title: &str) -> String {
    apply_request(client, view, select(agent)).await;
    let before = view.entries.len();
    let args = serde_json::json!({"title": title, "body": "Only you can say."});
    apply_request(client, view, send_message(agent, &format!("call ask_person {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// Dismisses every card `agent` shows.
async fn dismiss_all(client: &mut Client, view: &mut View, agent: &str) {
    let seqs: Vec<u64> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(card)) if card.state() == v1::AskPersonState::Pending => Some(entry.seq),
            _ => None,
        })
        .collect();
    for seq in seqs {
        let dismiss = v1::AnswerAsk { agent_id: agent.into(), seq, dismiss: true, ..Default::default() };
        apply_request(client, view, client_message::Body::AnswerAsk(dismiss)).await;
    }
    view.follow(client, |view| view.agent(agent).waiting_seqs.is_empty() && idle(view, agent)).await;
}

#[tokio::test]
async fn a_contained_agent_shows_at_most_three_questions_an_hour() {
    let (factory, mut client, mut view, _pipe) = with_liaison().await;
    for n in 1..=3 {
        let said = ask(&mut client, &mut view, "liaison", &format!("Question {n}")).await;
        assert!(said.starts_with("Asked"), "{said}");
    }
    dismiss_all(&mut client, &mut view, "liaison").await;
    assert_eq!(view.agent("liaison").held_asks, 0);

    // A fourth within the hour: no card, counted for the person.
    let said = ask(&mut client, &mut view, "liaison", "Question 4").await;
    assert!(said.contains("3 questions in the past hour") && said.contains("Nothing was asked"), "{said}");
    assert!(!said.contains("Question 4"), "{said}");
    view.follow(&mut client, |view| view.agent("liaison").held_asks == 1).await;
    assert!(view.agent("liaison").waiting_seqs.is_empty());

    // An agent that isn't contained has no hourly limit.
    for n in 1..=3 {
        ask(&mut client, &mut view, "lea", &format!("Lea's {n}")).await;
    }
    dismiss_all(&mut client, &mut view, "lea").await;
    let said = ask(&mut client, &mut view, "lea", "Lea's 4").await;
    assert!(said.starts_with("Asked"), "{said}");
    assert_eq!(view.agent("lea").held_asks, 0);

    // A curator reads what friends wrote: limited the same way.
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Ingrid", "")).await;
    pipes::make_curator_for_test(&factory, "ingrid", false).await.expect("a curator");
    for n in 1..=3 {
        ask(&mut client, &mut view, "ingrid", &format!("Ingrid's {n}")).await;
    }
    dismiss_all(&mut client, &mut view, "ingrid").await;
    let said = ask(&mut client, &mut view, "ingrid", "Ingrid's 4").await;
    assert!(said.contains("Nothing was asked"), "{said}");
    view.follow(&mut client, |view| view.agent("ingrid").held_asks == 1).await;
}

#[tokio::test]
async fn what_waited_goes_after_a_restart_once_it_may() {
    let dir = temp_dir();
    let models = Arc::new(Scripted::signed_in());
    let (addr, factory) = start_with_models(&dir, models.clone()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Its first pipe wake comes at once (the pipe never woke it before).
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, crate::common::create_agent("site", "Lea", "")).await;
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    let pipe = pipes::open_for_test(&factory, "site", "Website ⇄ Marketing", &["lea"], "lea", false);
    let add = v1::AddPipeLiaison { pipe_id: pipe.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, client_message::Body::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.lead_id == "liaison")).await;
    // Past its cap. (Had the pipe woken it in the minute before the restart,
    // the release would wait out the rest of that minute: a pipe wakes its
    // lead at most once a minute, restart or not.)
    run(&mut client, &mut view, "liaison", "hello").await;
    apply_request(&mut client, &mut view, cap("liaison", 0.0, 1)).await;
    view.follow(&mut client, |view| spend(view, "liaison").over).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Held through a restart.")).expect("received");
    view.follow(&mut client, |view| spend(view, "liaison").held_messages == 1).await;
    // Another agent shown, so the restarted Claktory streams none of its entries.
    apply_request(&mut client, &mut view, select("lea")).await;
    drop(client);
    drop(factory);

    // Its cap raised while Claktory was stopped.
    let db = rusqlite::Connection::open(crate::common::home(&dir).root().join("factory.db")).expect("db");
    db.execute("UPDATE pipe_spend_caps SET tokens = 100000000 WHERE agent = 'liaison'", []).expect("raised");
    drop(db);
    let (addr, _factory) = start_with_models(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Woken at start, before anything selects it: its run ends unseen.
    view.follow(&mut client, |view| view.agent("liaison").unseen && idle(view, "liaison")).await;
    apply_request(&mut client, &mut view, select("liaison")).await;
    assert!(events(&view).iter().any(|event| event.text.contains("Held through a restart.")), "{:?}", events(&view));
    assert!(!spend(&view, "liaison").over);
}
