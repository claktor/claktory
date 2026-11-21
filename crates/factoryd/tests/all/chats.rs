//! Chats (#213): the person's one-off conversations beside their agents,
//! with web search and a way to propose to Rivet, and nothing else.

use crate::common::{Client, MODEL, Scripted, apply_request, start, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, ProposalState, client_message, entry::Kind, server_message::Body};

/// What a client shows of the chats: the Workspace, and the shown chat's
/// conversation as ChatTranscript, ChatEntryUpdate and ChatEntryDelta build it.
#[derive(Default)]
struct Chats {
    workspace: v1::Workspace,
    chat: String,
    entries: Vec<v1::Entry>,
    queued: Option<v1::ChatQueuedMessages>,
}

impl Chats {
    fn apply(&mut self, body: &Body) {
        match body {
            Body::Workspace(workspace) => self.workspace = workspace.clone(),
            Body::ChatTranscript(transcript) => {
                self.chat = transcript.chat_id.clone();
                self.entries = transcript.entries.clone();
            }
            Body::ChatEntry(update) => {
                assert_eq!(update.chat_id, self.chat, "only the shown chat's entries are sent");
                let entry = update.entry.clone().expect("an entry");
                let first = self.entries.first().map_or(0, |entry| entry.seq) as usize;
                let at = entry.seq as usize - first;
                if at < self.entries.len() {
                    self.entries[at] = entry;
                } else {
                    assert_eq!(at, self.entries.len(), "entries arrive in order");
                    self.entries.push(entry);
                }
            }
            Body::ChatDelta(delta) => {
                assert_eq!(delta.chat_id, self.chat);
                let first = self.entries.first().map_or(0, |entry| entry.seq) as usize;
                match self.entries[delta.seq as usize - first].kind.as_mut() {
                    Some(Kind::Assistant(v1::AssistantText { text })) | Some(Kind::Thinking(v1::Thinking { text })) => {
                        text.push_str(&delta.text)
                    }
                    other => panic!("delta for {other:?}"),
                }
            }
            Body::ChatQueuedMessages(queued) => self.queued = Some(queued.clone()),
            Body::Entry(_) | Body::Delta(_) => {}
            _ => {}
        }
    }

    async fn request(&mut self, client: &mut Client, body: client_message::Body) -> Body {
        let reply = client.request(body).await;
        for event in &reply.events {
            if let Some(body) = &event.body {
                self.apply(body);
            }
        }
        reply.body
    }

    async fn ok(&mut self, client: &mut Client, body: client_message::Body) {
        let reply = self.request(client, body).await;
        assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    }

    async fn follow(&mut self, client: &mut Client, mut done: impl FnMut(&Chats) -> bool) {
        while !done(self) {
            if let Some(body) = client.receive().await.body {
                self.apply(&body);
            }
        }
    }

    fn get(&self, id: &str) -> Option<&v1::Chat> {
        self.workspace.chats.iter().find(|chat| chat.id == id)
    }

    fn only(&self) -> &v1::Chat {
        assert_eq!(self.workspace.chats.len(), 1, "{:?}", self.workspace.chats);
        &self.workspace.chats[0]
    }

    /// Idle with nothing queued.
    fn settled(&self, id: &str) -> bool {
        self.get(id).is_some_and(|chat| chat.status() == AgentStatus::Idle && chat.queued_messages == 0)
    }

    fn texts(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(Kind::User(user)) => Some(format!("user {}", user.text)),
                Some(Kind::Assistant(reply)) => Some(format!("assistant {}", reply.text)),
                _ => None,
            })
            .collect()
    }
}

fn create(text: &str) -> client_message::Body {
    client_message::Body::CreateChat(v1::CreateChat { text: text.into(), client_id: "c1".into() })
}

fn send(chat: &str, text: &str) -> client_message::Body {
    client_message::Body::SendChatMessage(v1::SendChatMessage { chat_id: chat.into(), text: text.into(), client_id: String::new() })
}

fn select(chat: &str) -> client_message::Body {
    client_message::Body::SelectChat(v1::SelectChat { chat_id: chat.into() })
}

fn rename(chat: &str, title: &str) -> client_message::Body {
    client_message::Body::RenameChat(v1::RenameChat { chat_id: chat.into(), title: title.into() })
}

fn select_agent(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

/// The scripted model is the default one, which chats run on.
async fn connect(addr: std::net::SocketAddr) -> (Client, Chats) {
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut chats = Chats { workspace, ..Chats::default() };
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    chats.ok(&mut client, client_message::Body::UpdateSettings(settings)).await;
    (client, chats)
}

const ASKED: &str = "what's the history of fried chicken?";

#[tokio::test]
async fn a_chat_answers_with_three_tools_and_titles_itself_from_its_first_exchange() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, mut chats) = connect(addr).await;
    let agents = chats.workspace.agents.len();

    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    assert!(id.starts_with("chat:"), "{id}");
    assert_eq!(chats.workspace.selected_chat_id, id, "a new chat is shown at once");
    assert_eq!(chats.only().model, MODEL);
    assert!(chats.only().model_available);
    // Until the model titles it: its opening words.
    chats.follow(&mut client, |chats| chats.chat == id).await;

    chats.follow(&mut client, |chats| chats.settled(&id) && chats.get(&id).is_some_and(|chat| chat.title.starts_with("echo"))).await;
    assert_eq!(chats.texts(), [format!("user {ASKED}"), format!("assistant echo: {ASKED}")]);
    // Titled by a model call of its own, apart from the conversation.
    assert_eq!(chats.only().title, "echo: The person asked");
    assert!(!chats.only().unseen, "shown while it replied");
    assert_eq!(chats.workspace.agents.len(), agents, "a chat is no agent");

    let requests = requests.lock().expect("lock").clone();
    let reply = requests
        .iter()
        .find(|request| request.prompt.contains(ASKED) && !request.prompt.contains("The person asked"))
        .expect("the reply's request");
    assert_eq!(reply.tools, ["web_search", "web_fetch", "propose_to_rivet"]);
    assert!(reply.system_prompt.starts_with("You're a chat in Claktory"), "{}", reply.system_prompt);
    let titled = requests.iter().find(|request| request.prompt.contains("The person asked")).expect("the title's request");
    assert!(titled.tools.is_empty());
    assert!(titled.system_prompt.starts_with("Write a title"));

    // Another message goes on in the same conversation.
    chats.ok(&mut client, send(&id, "and in Korea?")).await;
    chats.follow(&mut client, |chats| chats.texts().len() == 4 && chats.settled(&id)).await;
    assert_eq!(chats.texts()[3], "assistant echo: and in Korea?");
    assert_eq!(chats.only().title, "echo: The person asked", "titled once");
}

#[tokio::test]
async fn a_renamed_chat_keeps_its_name_and_an_empty_one_goes_back_to_the_automatic_title() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().title.starts_with("echo")).await;

    chats.ok(&mut client, rename(&id, "  Fried   chicken ")).await;
    assert_eq!(chats.only().title, "Fried chicken");
    chats.ok(&mut client, rename(&id, "")).await;
    assert_eq!(chats.only().title, "echo: The person asked");

    let unknown = chats.request(&mut client, rename("chat:99", "Nope")).await;
    assert!(matches!(unknown, Body::Error(_)), "{unknown:?}");
}

#[tokio::test]
async fn an_empty_chat_is_deleted_once_left_and_one_with_messages_is_kept() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;

    // Two empty chats in a row: the first is gone.
    chats.ok(&mut client, create("")).await;
    let first = chats.only().id.clone();
    assert_eq!(chats.only().title, "");
    chats.ok(&mut client, create("")).await;
    let second = chats.only().id.clone();
    assert_ne!(first, second);

    // Opening an agent leaves it: gone too.
    chats.ok(&mut client, select_agent("rivet")).await;
    assert!(chats.workspace.chats.is_empty(), "{:?}", chats.workspace.chats);
    assert_eq!(chats.workspace.selected_chat_id, "");

    // One that was written in stays when left, and opens again.
    chats.ok(&mut client, create("")).await;
    let kept = chats.only().id.clone();
    chats.ok(&mut client, send(&kept, ASKED)).await;
    chats.follow(&mut client, |chats| chats.settled(&kept) && chats.texts().len() == 2).await;
    chats.ok(&mut client, select_agent("rivet")).await;
    assert_eq!(chats.only().id, kept);
    assert_eq!(chats.workspace.selected_chat_id, "");
    chats.ok(&mut client, select(&kept)).await;
    assert_eq!(chats.workspace.selected_chat_id, kept);
    chats.follow(&mut client, |chats| chats.chat == kept && chats.texts().len() == 2).await;
}

#[tokio::test]
async fn a_chat_that_replies_while_not_shown_is_unseen_until_opened() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().title.starts_with("echo")).await;

    chats.ok(&mut client, select_agent("rivet")).await;
    chats.ok(&mut client, send(&id, "one more thing")).await;
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().unseen).await;
    chats.ok(&mut client, select(&id)).await;
    assert!(!chats.only().unseen);
}

#[tokio::test]
async fn a_deleted_chat_is_gone_with_its_conversation() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().title.starts_with("echo")).await;
    let folder = dir.join("home/chats").join(id.strip_prefix("chat:").expect("a chat's id"));
    assert!(folder.exists(), "{}", folder.display());

    chats.ok(&mut client, client_message::Body::DeleteChat(v1::DeleteChat { chat_id: id.clone() })).await;
    assert!(chats.workspace.chats.is_empty());
    assert_eq!(chats.workspace.selected_chat_id, "");
    assert!(!folder.exists());
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let left: i64 = db.query_row("SELECT count(*) FROM messages WHERE agent = ?1", [&id], |row| row.get(0)).expect("count");
    assert_eq!(left, 0);
    let again = chats.request(&mut client, select(&id)).await;
    assert!(matches!(again, Body::Error(_)), "{again:?}");

    // And it stays gone.
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert!(workspace.chats.is_empty());
}

#[tokio::test]
async fn chats_and_the_one_shown_survive_a_restart() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().title.starts_with("echo")).await;
    chats.ok(&mut client, rename(&id, "Fried chicken")).await;
    drop(client);
    drop(factory);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut chats = Chats { workspace, ..Chats::default() };
    assert_eq!(chats.only().title, "Fried chicken");
    assert_eq!(chats.workspace.selected_chat_id, id);
    // The shown chat's conversation comes with the state.
    chats.follow(&mut client, |chats| chats.chat == id).await;
    assert_eq!(chats.texts(), [format!("user {ASKED}"), format!("assistant echo: {ASKED}")]);
}

#[tokio::test]
async fn a_chat_proposes_to_rivet_who_gets_it_signed_with_the_chats_title() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
    chats.ok(&mut client, client_message::Body::UpdateAgent(rivet)).await;

    chats.ok(&mut client, create(ASKED)).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.settled(&id) && chats.only().title.starts_with("echo")).await;
    chats.ok(&mut client, rename(&id, "Fried chicken")).await;
    let wanted = "Matt wants a YouTube video on the history of fried chicken.";
    let args = serde_json::json!({"message": wanted}).to_string();
    chats.ok(&mut client, send(&id, &format!("call propose_to_rivet {args}"))).await;
    let proposal = |chats: &Chats| {
        chats.entries.iter().find_map(|entry| match &entry.kind {
            Some(Kind::Proposal(proposal)) => Some(proposal.clone()),
            _ => None,
        })
    };
    chats.follow(&mut client, |chats| proposal(chats).is_some() && chats.settled(&id)).await;
    let pending = proposal(&chats).expect("the proposal");
    assert_eq!((pending.text.as_str(), pending.state()), (wanted, ProposalState::Pending));
    assert!(!chats.only().waiting_seqs.is_empty(), "it waits for the person");

    let decide = v1::DecideProposal {
        chat_id: id.clone(),
        proposal_id: pending.id.clone(),
        approve: true,
        comment: "Yes please.".into(),
        ..Default::default()
    };
    chats.ok(&mut client, client_message::Body::DecideProposal(decide)).await;
    chats.follow(&mut client, |chats| proposal(chats).is_some_and(|proposal| proposal.state() == ProposalState::Approved)).await;
    // The chat is told, and carries on.
    chats
        .follow(&mut client, |chats| {
            chats.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.summary.contains("approved")))
                && chats.settled(&id)
        })
        .await;
    assert!(chats.only().waiting_seqs.is_empty());

    // Rivet got it as the person's message, signed with the chat's title.
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let text: String = db.query_row("SELECT text FROM messages WHERE agent = 'rivet'", [], |row| row.get(0)).expect("Rivet's message");
    assert_eq!(text, format!("Yes please.\n\nA chat (\"Fried chicken\") proposes:\n\n{wanted}"));
    let mut view = crate::common::View { workspace: chats.workspace.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, select_agent("rivet")).await;
    assert_eq!(view.workspace.selected_chat_id, "");
}

#[tokio::test]
async fn stop_ends_a_chats_reply() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create("think about it and hang")).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.get(&id).is_some_and(|chat| chat.status() == AgentStatus::Working)).await;
    chats.ok(&mut client, client_message::Body::StopChat(v1::StopChat { chat_id: id.clone() })).await;
    chats.follow(&mut client, |chats| chats.settled(&id)).await;

    // It answers the next message as usual.
    chats.ok(&mut client, send(&id, "never mind")).await;
    chats
        .follow(&mut client, |chats| chats.settled(&id) && chats.texts().last().is_some_and(|text| text == "assistant echo: never mind"))
        .await;
}

#[tokio::test]
async fn a_reply_cut_off_by_a_restart_carries_on_after_it() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, mut chats) = connect(addr).await;
    chats.ok(&mut client, create("look it up and hang")).await;
    let id = chats.only().id.clone();
    chats.follow(&mut client, |chats| chats.get(&id).is_some_and(|chat| chat.status() == AgentStatus::Working)).await;
    drop(client);
    drop(factory);

    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut chats = Chats { workspace, ..Chats::default() };
    chats
        .follow(&mut client, |chats| {
            chats.chat == id
                && chats.settled(&id)
                && chats
                    .entries
                    .iter()
                    .any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.summary == "Claktory restarted"))
                && chats.texts().last().is_some_and(|text| text.starts_with("assistant echo:"))
        })
        .await;
}
