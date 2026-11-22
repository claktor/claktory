//! Report this reply (store launch): the app names a reply, an agent's or a
//! chat's, by its entry, and the backend reads it itself: only a finished
//! assistant reply, its vault values hidden and cut at 16 KB, with the
//! model's name, the version and the person's cleaned note. A preview sends
//! nothing; a send goes to the account service, signed in or not, and is
//! kept there 30 days.

use factory_protocol::v1::{self, AgentStatus, ErrorCode, client_message::Body as Request, entry::Kind, server_message::Body};

use crate::claktory_account::{Instance, open, sign_in};
use crate::common::{Client, MODEL, View, apply_request, create_agent, create_project, last_workspace, send_message, start, temp_dir};

fn select(agent: &str) -> Request {
    Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    view.workspace.agents.iter().any(|candidate| candidate.id == agent && candidate.status() == AgentStatus::Idle)
}

fn report(agent: &str, seq: u64, note: &str, send: bool) -> Request {
    Request::ReportReply(v1::ReportReply { agent_id: agent.into(), chat_id: String::new(), seq, note: note.into(), send })
}

fn answer(body: Body) -> v1::ReplyReport {
    match body {
        Body::ReplyReport(report) => report,
        other => panic!("not a ReplyReport: {other:?}"),
    }
}

fn failure(body: &Body) -> (ErrorCode, String) {
    match body {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("not an error: {other:?}"),
    }
}

/// An agent "builder" with a conversation, in a Claktory at `addr`.
async fn builder(client: &mut Client, view: &mut View, dir: &std::path::Path) {
    apply_request(client, view, create_project("Site", &dir.join("site"))).await;
    apply_request(client, view, create_agent("site", "Builder", "")).await;
    apply_request(client, view, select("builder")).await;
}

/// Says `text` to the builder and waits for its answer: the seq of the
/// reply's text.
async fn ask(client: &mut Client, view: &mut View, text: &str) -> u64 {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", text)).await;
    view.follow(client, |view| idle(view, "builder") && view.entries.len() > before + 1).await;
    view.entries.iter().rev().find(|entry| matches!(entry.kind, Some(Kind::Assistant(_)))).expect("a reply").seq
}

#[tokio::test]
async fn only_a_finished_reply_is_previewed_as_it_would_be_sent_its_secrets_hidden_and_cut_at_16_kb() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    builder(&mut client, &mut view, &dir).await;
    let secret = v1::CreateSecret {
        name: "Stripe".into(),
        entries: vec![v1::SecretEntry { variable: "STRIPE_KEY".into(), value: "sk_live_report_test_51Hx".into(), file: false }],
        ..Default::default()
    };
    client.ok(Request::CreateSecret(secret)).await;

    let reply = ask(&mut client, &mut view, "the key is sk_live_report_test_51Hx").await;
    let shown = answer(client.request(report("builder", reply, "  Rude.\r\nVery\u{202E}.\u{7}  ", false)).await.body);
    assert!(shown.text.starts_with("echo:") && shown.text.contains("[secret Stripe/STRIPE_KEY]"), "{}", shown.text);
    assert!(!shown.text.contains("sk_live_report_test_51Hx"));
    assert_eq!((shown.model.as_str(), shown.note.as_str(), shown.truncated), ("Echo", "Rude.\nVery.", false));
    assert_eq!(shown.version, factoryd::update::VERSION);

    // Neither the person's message, nor a tool call, nor what isn't there.
    let mine = view.entries.iter().find(|entry| matches!(entry.kind, Some(Kind::User(_)))).expect("the person's message").seq;
    let refused = client.request(report("builder", mine, "", false)).await;
    assert_eq!(failure(&refused.body), (ErrorCode::InvalidRequest, "Only replies can be reported.".into()));
    let reply_after_call = ask(&mut client, &mut view, r#"call bash {"command":"true"}"#).await;
    let call = view.entries.iter().find(|entry| matches!(entry.kind, Some(Kind::ToolCall(_)))).expect("a tool call").seq;
    assert!(call < reply_after_call);
    let refused = client.request(report("builder", call, "", false)).await;
    assert_eq!(failure(&refused.body).0, ErrorCode::InvalidRequest);
    let missing = client.request(report("builder", 999, "", false)).await;
    assert_eq!(failure(&missing.body).0, ErrorCode::NotFound);
    let nobody = client.request(report("nobody", 0, "", false)).await;
    assert_eq!(failure(&nobody.body).0, ErrorCode::NotFound);
    let neither = client.request(report("", 0, "", false)).await;
    assert_eq!(failure(&neither.body).0, ErrorCode::InvalidRequest);
    let long_note = client.request(report("builder", reply, &"x".repeat(1001), false)).await;
    assert_eq!(failure(&long_note.body), (ErrorCode::InvalidRequest, "Keep your note under 1,000 characters.".into()));

    // A long reply is cut at 16 KB, on a character's boundary.
    let long = ask(&mut client, &mut view, &"é".repeat(10_000)).await;
    let shown = answer(client.request(report("builder", long, "", false)).await.body);
    assert!(shown.truncated && shown.text.len() <= 16 * 1024 && shown.text.len() > 16 * 1024 - 4, "{}", shown.text.len());
}

#[tokio::test]
async fn a_chats_reply_can_be_reported_too() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    client.ok(Request::UpdateSettings(settings)).await;
    let events = client.ok(Request::CreateChat(v1::CreateChat { text: "hello there".into(), client_id: "c1".into() })).await;
    let chat = last_workspace(&events).and_then(|workspace| workspace.chats.first()).expect("the chat").id.clone();
    // Its reply, once the model has answered, after its message (seq 0).
    let mut tries = 0;
    let shown = 'found: loop {
        for seq in 1..6 {
            let ask = Request::ReportReply(v1::ReportReply { chat_id: chat.clone(), seq, ..Default::default() });
            if let Body::ReplyReport(shown) = client.request(ask).await.body {
                break 'found shown;
            }
        }
        tries += 1;
        assert!(tries < 200, "no reply to report");
        tokio::time::sleep(std::time::Duration::from_millis(50)).await
    };
    assert!(shown.text.contains("hello there") && shown.model == "Echo", "{shown:?}");
    let unknown = client.request(Request::ReportReply(v1::ReportReply { chat_id: "chat:999".into(), ..Default::default() })).await;
    assert_eq!(failure(&unknown.body).0, ErrorCode::NotFound);
}

/// The reports the local service keeps: (text, model, note).
fn kept(dir: &std::path::Path) -> Vec<(String, String, String)> {
    let conn = rusqlite::Connection::open(dir.join("account.db")).expect("the service's database");
    let mut statement = conn.prepare("SELECT text, model, note FROM ai_reports ORDER BY id").expect("sql");
    statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).expect("sql").collect::<rusqlite::Result<_>>().expect("rows")
}

#[tokio::test]
async fn a_report_reaches_the_service_signed_in_or_not_and_ten_an_hour_at_most() {
    let dir = temp_dir();
    let instance = Instance::start(&dir).await;
    let (addr, _factory) = open(&dir, &instance.url).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    builder(&mut client, &mut view, &dir).await;
    let reply = ask(&mut client, &mut view, "something rude").await;

    // Signed out: unsigned, as the preview showed it.
    let preview = answer(client.request(report("builder", reply, "Rude.", false)).await.body);
    assert!(kept(&dir).is_empty(), "a preview sends nothing");
    let sent = answer(client.request(report("builder", reply, "Rude.", true)).await.body);
    assert_eq!(sent, preview);
    assert_eq!(kept(&dir), [("echo: something rude".to_owned(), "Echo".to_owned(), "Rude.".to_owned())]);

    // Signed in: signed, and kept just the same.
    sign_in(&mut client, &instance, "matt@example.com").await;
    answer(client.request(report("builder", reply, "", true)).await.body);
    assert_eq!(kept(&dir)[1], ("echo: something rude".to_owned(), "Echo".to_owned(), String::new()));

    for number in 2..10 {
        answer(client.request(report("builder", reply, &format!("Again, {number}."), true)).await.body);
    }
    let limited = client.request(report("builder", reply, "", true)).await;
    let (code, message) = failure(&limited.body);
    assert_eq!(code, ErrorCode::RateLimited);
    assert!(message.starts_with("You've sent a lot of reports. Try again in "), "{message}");
    assert_eq!(kept(&dir).len(), 10);
    answer(client.request(report("builder", reply, "", false)).await.body);
}

#[tokio::test]
async fn a_report_says_when_the_service_cant_be_reached_or_isnt_to_be_contacted() {
    let dir = temp_dir();
    let (addr, factory) = open(&dir, "http://127.0.0.1:9").await;
    let (mut client, _, _) = Client::connect(addr).await;
    let mut view = View::default();
    builder(&mut client, &mut view, &dir).await;
    let reply = ask(&mut client, &mut view, "something rude").await;
    let unreachable = client.request(report("builder", reply, "", true)).await;
    assert_eq!(
        failure(&unreachable.body),
        (ErrorCode::Unreachable, "Can't reach 127.0.0.1. Check your internet connection and try again.".into())
    );

    drop(client);
    drop(factory);
    let path = dir.join("home/config.json");
    let mut config: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).expect("config")).expect("json");
    config["account"]["enabled"] = serde_json::Value::Bool(false);
    std::fs::write(&path, serde_json::to_vec(&config).expect("json")).expect("config");
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let off = client.request(report("builder", reply, "", true)).await;
    assert_eq!(
        failure(&off.body),
        (ErrorCode::InvalidRequest, "Reports go to claktory.com, and this Claktory is set not to contact it.".into())
    );
    answer(client.request(report("builder", reply, "", false)).await.body);
}

#[tokio::test]
async fn while_secrets_are_locked_nothing_is_reported_since_none_could_be_hidden() {
    let dir = temp_dir();
    let reply;
    {
        let (addr, _factory) = start(&dir).await;
        let (mut client, _, _) = Client::connect(addr).await;
        let mut view = View::default();
        builder(&mut client, &mut view, &dir).await;
        reply = ask(&mut client, &mut view, "something rude").await;
        client.request(Request::NewRecoveryKey(v1::NewRecoveryKey { proof: None })).await;
        client.request(Request::SetVaultLockedMode(v1::SetVaultLockedMode { locked_mode: true, proof: None })).await;
    }
    let (addr, _factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    let locked = client.request(report("builder", reply, "", false)).await;
    assert_eq!(
        failure(&locked.body),
        (ErrorCode::InvalidRequest, "Unlock your secrets first, so any secrets in this reply can be hidden before it's sent.".into())
    );
}
