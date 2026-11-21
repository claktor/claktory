//! Pipes (#196) end to end: two Claktories pair through an invite, both
//! people approve, and their agents talk through the store and Sam's
//! transport together. A file of its own: the transport's store is
//! process-wide, and the transport's own tests register a test store.

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{Client, View, apply_request, create_agent, create_project, send_message, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::factory::Factory;
use factoryd::pipes;

struct Side {
    factory: Arc<Factory>,
    client: Client,
    view: View,
}

/// A Claktory with a project and one agent, its tunnel quick and its relay
/// dead, so nothing uses the network.
async fn claktory(dir: &Path, project: &str, agent: &str) -> Side {
    let (addr, factory) = common::start(dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    pipes::set_wake_every(&factory, Duration::ZERO);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let relays = v1::SetRemoteAccess { relays: vec!["http://127.0.0.1:9/".into()], set_relays: true, ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetRemoteAccess(relays)).await;
    apply_request(&mut client, &mut view, create_project(project, &dir.join("work"))).await;
    apply_request(&mut client, &mut view, create_agent(&project.to_lowercase(), agent, "")).await;
    Side { factory, client, view }
}

impl Side {
    async fn request(&mut self, body: Request) -> Body {
        apply_request(&mut self.client, &mut self.view, body).await
    }

    fn pipe(&self) -> Option<v1::Pipe> {
        self.view.workspace.pipes.first().cloned()
    }

    async fn until(&mut self, done: impl Fn(&v1::Pipe) -> bool) -> v1::Pipe {
        let wait = self.view.follow(&mut self.client, |view| view.workspace.pipes.first().is_some_and(&done));
        tokio::time::timeout(Duration::from_secs(20), wait).await.expect("the pipe got there");
        self.pipe().expect("the pipe")
    }

    /// Calls `tool` as `agent` and returns what it answered.
    async fn call(&mut self, agent: &str, tool: &str, args: &str) -> String {
        self.request(Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
        let before = self.view.entries.len();
        self.request(send_message(agent, &format!("call {tool} {args}"))).await;
        self.view.follow(&mut self.client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
        self.view.entries[before..]
            .iter()
            .find_map(|entry| match &entry.kind {
                Some(Kind::ToolCall(call)) => Some(call.output.clone()),
                _ => None,
            })
            .expect("a tool call")
    }

    /// Waits for the pipe event `agent` is woken with, and returns its text.
    async fn event(&mut self, agent: &str, containing: &str) -> String {
        self.request(Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
        let found = |view: &View| {
            view.entries.iter().find_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.source() == v1::EventSource::Pipe && event.text.contains(containing) => {
                    Some(event.text.clone())
                }
                _ => None,
            })
        };
        let wait = self.view.follow(&mut self.client, |view| found(view).is_some());
        tokio::time::timeout(Duration::from_secs(20), wait).await.expect("the event came");
        let text = found(&self.view).expect("it");
        self.view.follow(&mut self.client, |view| idle(view, agent)).await;
        text
    }
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn two_claktories_pair_approve_and_their_agents_talk() {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let mut a = claktory(&dir_a, "Site", "Lea").await;
    let mut b = claktory(&dir_b, "Marketing", "Max").await;

    // A invites; B reads the code, then joins with it.
    let create = v1::CreatePipeInvite { project_id: "site".into(), name: "Launch".into(), purpose: "launch dates".into() };
    let Body::PipeInvite(invite) = a.request(Request::CreatePipeInvite(create)).await else { panic!("no invite") };
    assert_eq!(a.until(|pipe| pipe.state() == v1::PipeState::Invited).await.id, invite.pipe_id);
    let read = v1::ReadPipeInvite { code: invite.code.clone() };
    let Body::PipeInvitePreview(preview) = b.request(Request::ReadPipeInvite(read)).await else { panic!("no preview") };
    assert_eq!((preview.name.as_str(), preview.peer.expect("peer").purpose.as_str()), ("Launch", "launch dates"));
    let accept =
        v1::AcceptPipeInvite { code: invite.code.clone(), project_id: "marketing".into(), name: "Launch".into(), purpose: "copy".into() };
    assert!(matches!(b.request(Request::AcceptPipeInvite(accept)).await, Body::Ack(_)));

    // Both people see the same digits, and approve.
    let joined = b.until(|pipe| pipe.state() == v1::PipeState::Approval).await;
    let knocked = a.until(|pipe| pipe.state() == v1::PipeState::Approval).await;
    assert_eq!(knocked.digits, joined.digits);
    assert_eq!(knocked.digits.len(), 4);
    assert_eq!(knocked.peer.expect("peer").purpose, "copy");
    assert_eq!(b.view.workspace.selected_pipe_id, joined.id);
    let approve = |pipe: &str, agent: &str| {
        Request::ApprovePipe(v1::ApprovePipe {
            pipe_id: pipe.into(),
            approve: true,
            member_ids: vec![agent.into()],
            lead_id: agent.into(),
            liaison_name: String::new(),
        })
    };
    assert!(matches!(b.request(approve(&joined.id, "max")).await, Body::Ack(_)));
    b.until(|pipe| pipe.state() == v1::PipeState::WaitingPeer).await;
    assert!(matches!(a.request(approve(&knocked.id, "lea")).await, Body::Ack(_)));
    a.until(|pipe| pipe.state() == v1::PipeState::Open).await;
    b.until(|pipe| pipe.state() == v1::PipeState::Open).await;

    // A's person writes; B's lead hears it, fenced, from A's person.
    let send = v1::SendPipeMessage { pipe_id: knocked.id.clone(), text: "Launch is Friday.".into() };
    assert!(matches!(a.request(Request::SendPipeMessage(send)).await, Body::Ack(_)));
    let heard = b.event("max", "Launch is Friday.").await;
    assert!(heard.contains("Untrusted message from the other end of the pipe \"Launch\""), "{heard}");
    let got = pipes::messages_for_test(&b.factory, &joined.id);
    let got = got.iter().find(|message| message.text == "Launch is Friday.").expect("received");
    assert_eq!(got.author.clone().expect("author").kind(), v1::PipeAuthorKind::TheirPerson);

    // B's agent answers; with review off it goes at once.
    let off = v1::UpdatePipe { pipe_id: joined.id.clone(), review_outgoing: Some(false), ..Default::default() };
    b.request(Request::UpdatePipe(off)).await;
    let sent = b.call("max", "pipe_send", r#"{"text": "Noted: Friday."}"#).await;
    assert!(sent.starts_with("Sent"), "{sent}");
    a.event("lea", "Noted: Friday.").await;
    let reply = pipes::messages_for_test(&a.factory, &knocked.id);
    let reply = reply.iter().find(|message| message.text == "Noted: Friday.").expect("received");
    let author = reply.author.clone().expect("author");
    assert_eq!((author.kind(), author.name.as_str()), (v1::PipeAuthorKind::TheirAgent, "Max"));

    // A pauses: B sees it paused by the other side, and can't resume it.
    let action =
        |pipe: &str, action: v1::PipeAction| Request::SetPipeState(v1::SetPipeState { pipe_id: pipe.into(), action: action.into() });
    assert!(matches!(a.request(action(&knocked.id, v1::PipeAction::Pause)).await, Body::Ack(_)));
    let paused = b.until(|pipe| pipe.state() == v1::PipeState::Paused).await;
    assert_eq!(paused.paused(), v1::PipePause::ByPeer);
    assert!(matches!(b.request(action(&joined.id, v1::PipeAction::Resume)).await, Body::Error(_)));
    assert!(matches!(a.request(action(&knocked.id, v1::PipeAction::Resume)).await, Body::Ack(_)));
    b.until(|pipe| pipe.state() == v1::PipeState::Open).await;

    // A closes it: closed on both sides.
    assert!(matches!(a.request(action(&knocked.id, v1::PipeAction::Close)).await, Body::Ack(_)));
    b.until(|pipe| pipe.state() == v1::PipeState::Closed).await;
}
