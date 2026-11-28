//! Start over: an agent clears its own conversation when it finishes a task.
//! The whole conversation is kept as another version, what came meanwhile
//! reaches the new one, and the person sees it in History.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind, server_message::Body};
use factoryd::fork;

fn idle(view: &View) -> bool {
    let agent = view.agent("consultant");
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn shown(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::User(user)) => Some(user.text.clone()),
            Some(Kind::Assistant(reply)) => Some(reply.text.clone()),
            Some(Kind::Notice(notice)) => Some(format!("notice: {}", notice.text)),
            _ => None,
        })
        .collect()
}

async fn say(client: &mut Client, view: &mut View, text: &str) {
    let before = view.entries.len();
    apply_request(client, view, send_message("consultant", text)).await;
    view.follow(client, |view| view.entries.len() > before + 1 && idle(view)).await;
}

fn turn_on(on: bool) -> client_message::Body {
    let change = v1::ExtensionChange { id: "start_over".into(), enabled: Some(on), ..Default::default() };
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: "consultant".into(), extension: Some(change), ..Default::default() })
}

#[tokio::test]
async fn an_agent_starts_over_keeping_the_old_conversation_and_what_came_meanwhile() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Consultant", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "consultant".into() })).await;

    // Off unless turned on: the tool isn't there.
    say(&mut client, &mut view, r#"call start_over {"reason": "Done."}"#).await;
    assert!(fork::branches_for_test(&factory, "consultant").is_empty());
    assert!(matches!(apply_request(&mut client, &mut view, turn_on(true)).await, Body::Ack(_)));
    say(&mut client, &mut view, "first consult").await;
    say(&mut client, &mut view, r#"call secrets_request {"name": "Key", "reason": "To ship."}"#).await;

    // Turned on mid-conversation, it calls it through extension_call. A
    // message sent while that step is still streaming waits for the new
    // conversation.
    let start_over = r#"slowcall extension_call {"tool": "start_over", "args": {"reason": "Consult answered!"}}"#;
    apply_request(&mut client, &mut view, send_message("consultant", start_over)).await;
    view.follow(&mut client, |view| view.agent("consultant").status() == AgentStatus::Working).await;
    apply_request(&mut client, &mut view, send_message("consultant", "second consult")).await;
    view.follow(&mut client, |view| shown(view).iter().any(|text| text == "echo: second consult") && idle(view)).await;
    assert_eq!(
        shown(&view),
        [
            "notice: Consultant started over: Consult answered! Its earlier conversation is kept in its other versions.",
            "second consult",
            "echo: second consult",
        ]
    );

    // The conversation before is a version, made by the agent itself.
    let versions = fork::branches_for_test(&factory, "consultant");
    assert_eq!(versions.len(), 1, "{versions:?}");
    assert_eq!(versions[0].made_by, "consultant");
    assert!(versions[0].messages >= 4, "{:?}", versions[0]);

    // The person sees it in History, with its reason.
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(&mut client, &mut view, client_message::Body::ListNeedsYouHistory(list)).await
    else {
        panic!("history")
    };
    let record = history.records.iter().find(|record| record.kind() == v1::NeedsYouRecordKind::Conversation).expect("in History");
    assert_eq!(record.title, "Consultant started over with a new conversation");
    assert_eq!(record.agent_reason, "Consult answered!");

    // And can switch back to it.
    fork::switch_for_test(&factory, "consultant", &versions[0].id).await.expect("switched");
    view.follow(&mut client, |view| shown(view).iter().any(|text| text == "first consult")).await;
    assert!(!shown(&view).iter().any(|text| text == "second consult"), "{:?}", shown(&view));

    // Its request to the person was withdrawn, saying why.
    let request = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) => Some(request.clone()),
            _ => None,
        })
        .expect("the request");
    assert_eq!(
        (request.state(), request.comment.as_str()),
        (v1::SecretRequestState::Withdrawn, "Consultant started over, so this is no longer needed.")
    );
}

/// What it read from outside still limits it after it starts over: the new
/// conversation forgets the page, not that it was read. Only the person's
/// /clear lifts it.
#[tokio::test]
async fn starting_over_leaves_what_it_read_from_outside_limiting_it() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Consultant", "")).await;
    apply_request(&mut client, &mut view, client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "consultant".into() })).await;
    apply_request(&mut client, &mut view, turn_on(true)).await;
    let secret = v1::CreateSecret {
        name: "Deploy".into(),
        description: "Given.".into(),
        entries: vec![v1::SecretEntry { variable: "DEPLOY_TOKEN".into(), value: "deploy-token-0123456789".into(), file: false }],
        ask_each_time: false,
        sites: Vec::new(),
    };
    apply_request(&mut client, &mut view, client_message::Body::CreateSecret(secret)).await;
    let id = view.workspace.vault.clone().expect("vault").secrets.into_iter().find(|secret| secret.name == "Deploy").expect("added").id;
    let grant = v1::GrantSecret { id, agent_id: "consultant".into(), proof: None, fill_only: false };
    apply_request(&mut client, &mut view, client_message::Body::GrantSecret(grant)).await;
    say(&mut client, &mut view, "hello").await;

    factoryd::pipes::taint_for_test(&factory, "consultant", "browser:https://example.com");
    factoryd::errands::read_outside_for_test(&factory, "consultant", "a web page");
    let taint = factoryd::pipes::taint_of_for_test(&factory, "consultant");
    assert_ne!(taint, "{}");
    apply_request(&mut client, &mut view, send_message("consultant", r#"call start_over {"reason": "Done."}"#)).await;
    view.follow(&mut client, |view| shown(view).first().is_some_and(|text| text.starts_with("notice: Consultant started over")) && idle(view))
        .await;

    assert_eq!(factoryd::pipes::taint_of_for_test(&factory, "consultant"), taint);
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let read: String =
        db.query_row("SELECT what FROM errand_outside_read WHERE agent = 'consultant'", [], |row| row.get(0)).expect("still marked");
    assert_eq!(read, "a web page");
    // Its secrets still wait for the person.
    let ran = dir.join("ran");
    let args = serde_json::json!({"command": format!("touch {}", ran.display()), "secrets": ["Deploy"]});
    let said = crate::pipes::call(&mut client, &mut view, "consultant", "bash", &args.to_string()).await;
    assert!(said.starts_with("Not run"), "{said}");
    assert!(!ran.exists(), "it ran");
}
