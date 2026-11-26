//! Websites shared over pipes (pipe-port-sharing.md) end to end: two
//! Claktories pair through an invite, one shares a port on its computer,
//! and the other reaches it through the pipe. A file of its own: the pipes'
//! transport store is process-wide, and the transport's own tests register
//! a test store.

#[path = "common/mod.rs"]
mod common;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use common::{Client, View, apply_request, create_agent, create_project, temp_dir};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use factoryd::pipes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Side {
    factory: Arc<Factory>,
    client: Client,
    view: View,
    /// The notifications it was sent, as `until` saw them.
    notes: Vec<v1::Notify>,
}

/// A Claktory with a project, its tunnel quick and its relay dead, so
/// nothing uses the network.
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
    Side { factory, client, view, notes: Vec::new() }
}

impl Side {
    async fn request(&mut self, body: Request) -> Body {
        apply_request(&mut self.client, &mut self.view, body).await
    }

    fn pipe(&self) -> v1::Pipe {
        self.view.workspace.pipes.first().cloned().expect("the pipe")
    }

    async fn until(&mut self, done: impl Fn(&View) -> bool) {
        self.until_with(|view, _| done(view)).await;
    }

    /// Until `done` holds of the view and the notifications sent so far.
    async fn until_with(&mut self, done: impl Fn(&View, &[v1::Notify]) -> bool) {
        let wait = async {
            while !done(&self.view, &self.notes) {
                let message = self.client.receive().await;
                if let Some(body) = &message.body {
                    if let Body::Notify(notify) = body {
                        self.notes.push(notify.clone());
                    }
                    self.view.apply(body);
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(20), wait).await.expect("it got there");
    }

    async fn until_named(&mut self, name: &str, state: v1::PipeState) -> v1::Pipe {
        self.until(|view| view.workspace.pipes.iter().any(|pipe| pipe.name == name && pipe.state() == state)).await;
        self.view.workspace.pipes.iter().find(|pipe| pipe.name == name).cloned().expect("the pipe")
    }

    fn shares(&self) -> Vec<v1::PipeShare> {
        self.view.workspace.pipe_shares.clone()
    }

    async fn pipe_state(&mut self, action: v1::PipeAction) {
        let request = v1::SetPipeState { pipe_id: self.pipe().id, action: action.into() };
        assert!(matches!(self.request(Request::SetPipeState(request)).await, Body::Ack(_)));
    }

    fn said(&self, containing: &str) -> bool {
        pipes::messages_for_test(&self.factory, &self.pipe().id).iter().any(|message| message.text.contains(containing))
    }
}

/// Two Claktories with an open pipe between them: A's project Site, B's
/// Marketing.
async fn paired() -> (Side, Side) {
    let (dir_a, dir_b) = (temp_dir(), temp_dir());
    let mut a = claktory(&dir_a, "Site", "Lea").await;
    let mut b = claktory(&dir_b, "Marketing", "Max").await;
    pair((&mut a, "site", "lea"), (&mut b, "marketing", "max"), "Launch").await;
    (a, b)
}

/// Opens a pipe named `name` from `inviter` to `joiner` (each: the side,
/// its project and its agent); their ids for it.
async fn pair(inviter: (&mut Side, &str, &str), joiner: (&mut Side, &str, &str), name: &str) -> (String, String) {
    let (a, a_project, a_agent) = inviter;
    let (b, b_project, b_agent) = joiner;
    let create = v1::CreatePipeInvite { project_id: a_project.into(), name: name.into(), purpose: "launch".into(), ..Default::default() };
    let Body::PipeInvite(invite) = a.request(Request::CreatePipeInvite(create)).await else { panic!("no invite") };
    let accept = v1::AcceptPipeInvite { code: invite.code, project_id: b_project.into(), name: name.into(), ..Default::default() };
    assert!(matches!(b.request(Request::AcceptPipeInvite(accept)).await, Body::Ack(_)));
    let mut ids = Vec::new();
    for (side, agent) in [(&mut *a, a_agent), (&mut *b, b_agent)] {
        let pipe = side.until_named(name, v1::PipeState::Approval).await;
        let approve = v1::ApprovePipe {
            pipe_id: pipe.id.clone(),
            approve: true,
            member_ids: vec![agent.into()],
            lead_id: agent.into(),
            ..Default::default()
        };
        assert!(matches!(side.request(Request::ApprovePipe(approve)).await, Body::Ack(_)));
        ids.push(pipe.id);
    }
    a.until_named(name, v1::PipeState::Open).await;
    b.until_named(name, v1::PipeState::Open).await;
    (ids[0].clone(), ids[1].clone())
}

/// A server on this computer that echoes what it's sent: the website.
async fn echo_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its port").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buffer = [0u8; 4096];
                while let Ok(read) = socket.read(&mut buffer).await {
                    if read == 0 || socket.write_all(&buffer[..read]).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// Opens a stream to B's share `id` and says `text` through it: what came
/// back, or why it couldn't.
async fn through(b: &Side, id: &str, text: &str) -> Result<String, String> {
    let (mut send, mut receive) = pipes::shares::stream_for_test(&b.factory, id).await?;
    send.write_all(text.as_bytes()).await.map_err(|error| error.to_string())?;
    let mut back = vec![0u8; text.len()];
    tokio::time::timeout(Duration::from_secs(10), receive.read_exact(&mut back))
        .await
        .map_err(|_| "no answer".to_owned())?
        .map_err(|error| error.to_string())?;
    Ok(String::from_utf8_lossy(&back).into_owned())
}

/// One test: the transport's store is process-wide, so the scenarios run
/// one after the other.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn websites_shared_between_two_claktories() {
    a_shared_website_reaches_the_other_side_while_its_pipe_is_open().await;
    a_third_claktory_pinned_to_another_pipe_gets_nothing().await;
    the_other_side_opens_it_in_a_browser_at_its_own_address().await;
}

/// C is pinned to A too, on a pipe of its own: asking for the website A
/// shares with B, by its id, it gets nothing (Fugo 31072).
async fn a_third_claktory_pinned_to_another_pipe_gets_nothing() {
    let (mut a, mut b) = paired().await;
    let mut c = claktory(&temp_dir(), "Press", "Pia").await;
    let (_, c_pipe) = pair((&mut a, "site", "lea"), (&mut c, "press", "pia"), "Coverage").await;
    let port = echo_server().await;
    let a_launch = a.view.workspace.pipes.iter().find(|pipe| pipe.name == "Launch").expect("Launch").id.clone();
    let share = v1::SharePipeWebsite { port: u32::from(port), name: "Dashboard".into(), pipe_ids: vec![a_launch], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(share)).await, Body::Ack(_)));
    let id = a.shares().into_iter().find(|share| share.mine).expect("A's").id;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.live)).await;
    let theirs = b.shares().into_iter().next().expect("B's").id;
    assert_eq!(through(&b, &theirs, "hi").await.as_deref(), Ok("hi"));
    let asked = pipes::shares::ask_for_test(&c.factory, &c_pipe, &id).await;
    assert_eq!(asked, Err("This website isn't shared with you now.".to_owned()));
    assert!(c.shares().is_empty());
}

/// A website on this computer: answers each request with the Host and
/// cookies that reached it, one request a connection.
async fn website() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let port = listener.local_addr().expect("its port").port();
    tokio::spawn(async move {
        while let Ok((mut socket, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") {
                    if socket.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_string();
                let header = |name: &str| {
                    head.lines()
                        .find_map(|line| line.split_once(':').filter(|(key, _)| key.eq_ignore_ascii_case(name)).map(|(_, value)| value.trim().to_owned()))
                        .unwrap_or_default()
                };
                let body = format!("hello from A; host={}; cookie={}", header("host"), header("cookie"));
                let answer = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = socket.write_all(answer.as_bytes()).await;
            });
        }
    });
    port
}

/// A browser's request on this computer: (status, head, body).
async fn browse(address: &str, host: &str, path: &str, cookie: &str) -> Result<(u16, String, String), String> {
    let mut socket = tokio::net::TcpStream::connect(address).await.map_err(|error| error.to_string())?;
    let cookie = if cookie.is_empty() { String::new() } else { format!("Cookie: {cookie}\r\n") };
    let request = format!("GET {path} HTTP/1.1\r\nHost: {host}\r\n{cookie}Connection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.map_err(|error| error.to_string())?;
    let mut answer = String::new();
    tokio::time::timeout(Duration::from_secs(10), socket.read_to_string(&mut answer))
        .await
        .map_err(|_| "no answer".to_owned())?
        .map_err(|error| error.to_string())?;
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let status = head.split(' ').nth(1).and_then(|status| status.parse().ok()).unwrap_or(0);
    Ok((status, head.to_owned(), body.to_owned()))
}

/// The receiving side (Cioccolata): B opens A's website in a browser on its
/// own computer, at an address of its own for the share, through the pipe.
async fn the_other_side_opens_it_in_a_browser_at_its_own_address() {
    let (mut a, mut b) = paired().await;
    let port = website().await;
    let share = v1::SharePipeWebsite { port: u32::from(port), name: "Dashboard".into(), pipe_ids: vec![a.pipe().id], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(share)).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| !share.mine && share.live && !share.address.is_empty())).await;
    let theirs = b.shares().into_iter().find(|share| !share.mine).expect("B's");
    // W2: B's person is told once, and it stands out until it's opened.
    // Never with its name, the other side's words, on a lock screen.
    b.until_with(|_, notes| notes.iter().any(|note| !note.share_id.is_empty())).await;
    let shared: Vec<&v1::Notify> = b.notes.iter().filter(|note| !note.share_id.is_empty()).collect();
    assert_eq!(shared.len(), 1, "{:?}", b.notes);
    assert_eq!((shared[0].pipe_id.as_str(), shared[0].share_id.as_str()), (b.pipe().id.as_str(), theirs.id.as_str()));
    assert_eq!(shared[0].kind(), v1::NotifyKind::NeedsYou);
    assert_eq!((shared[0].title.as_str(), shared[0].body.as_str()), ("Launch", "Launch shares a website with you"));
    assert!(!format!("{:?}", shared[0]).contains("Dashboard"), "{:?}", shared[0]);
    assert!(theirs.unopened);
    // The chat's line announcing it says which share it is, for Open on it;
    // no other line does.
    let lines = pipes::messages_for_test(&b.factory, &b.pipe().id);
    let announced: Vec<&v1::PipeMessage> = lines.iter().filter(|line| !line.share_id.is_empty()).collect();
    assert_eq!(announced.len(), 1, "{lines:?}");
    assert!(announced[0].text.ends_with("shares Dashboard."), "{:?}", announced[0]);
    assert_eq!(announced[0].share_id, theirs.id);

    // W3: the form says which program listens on the port shared.
    let check = v1::CheckPipeSharePort { port: u32::from(port) };
    let Body::PipeSharePort(checked) = a.request(Request::CheckPipeSharePort(check)).await else { panic!("no answer") };
    assert_eq!(checked.pid, std::process::id(), "{checked:?}");
    assert!(!checked.program.is_empty() && !checked.program.contains('/'), "{checked:?}");
    // Shared with that pipe already: the form says so, and once is enough
    // (row 1340).
    assert_eq!(checked.shared_pipe_ids, [a.pipe().id], "{checked:?}");
    let again = v1::SharePipeWebsite { port: u32::from(port), name: "Dashboard".into(), pipe_ids: vec![a.pipe().id], ..Default::default() };
    match a.request(Request::SharePipeWebsite(again)).await {
        Body::Error(error) => assert!(error.message.contains(&format!("Dashboard on port {port} is shared with")) && error.message.ends_with("already."), "{}", error.message),
        other => panic!("shared twice: {other:?}"),
    }
    assert_eq!(a.shares().iter().filter(|share| share.mine).count(), 1);

    // Its address is its own, known before Open; nothing listens until then.
    let host = theirs.address.strip_prefix("http://").and_then(|rest| rest.strip_suffix('/')).expect("an address").to_owned();
    let (name, local) = host.split_once(':').expect("a port");
    assert!(name.starts_with("s-") && name.ends_with(".localhost"), "{host}");
    // A's website holds that port here, so B's is another.
    assert_ne!(local, port.to_string());
    assert_eq!(theirs.fallback_address, format!("http://127.0.0.1:{local}/"));
    assert!(!theirs.listening);
    let at = format!("127.0.0.1:{local}");
    assert!(browse(&at, &host, "/", "").await.is_err(), "listening before Open");

    // Open: a one-time address, traded for the share's key.
    let open = v1::OpenPipeShare { share_id: theirs.id.clone() };
    let Body::PipeShareOpened(opened) = b.request(Request::OpenPipeShare(open)).await else { panic!("not opened") };
    assert!(opened.url.starts_with(&format!("http://{host}/__claktory_open?k=")), "{}", opened.url);
    assert!(opened.fallback_url.starts_with(&format!("http://{at}/__claktory_open?k=")), "{}", opened.fallback_url);
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && share.listening && !share.unopened)).await;
    let path = opened.url.split_once(&host).expect("its path").1.to_owned();
    let (status, head, _) = browse(&at, &host, &path, "").await.expect("answered");
    assert_eq!(status, 303, "{head}");
    let key = head
        .lines()
        .find_map(|line| line.split_once(':').filter(|(name, _)| name.eq_ignore_ascii_case("set-cookie")).map(|(_, value)| value.trim().to_owned()))
        .expect("the key");
    let key = key.split(';').next().expect("its value").to_owned();

    // With it, A's website, through the pipe, its Host as the browser sent it.
    let (status, _, body) = browse(&at, &host, "/", &format!("{key}; theme=dark")).await.expect("answered");
    assert_eq!((status, body.as_str()), (200, format!("hello from A; host={host}; cookie=theme=dark").as_str()));
    // Without it, or under another name, nothing reaches A.
    assert_eq!(browse(&at, &host, "/", "").await.expect("answered").0, 403);
    assert_eq!(browse(&at, &format!("evil.example:{local}"), "/", &key).await.expect("answered").0, 421);
    // W1: a paired device's browser, through a forward at a port of its
    // own, reaches it under the same host, with the same key.
    let forwarded = host.replace(&format!(":{local}"), ":24567");
    let (status, _, body) = browse(&at, &forwarded, "/", &key).await.expect("answered");
    assert_eq!((status, body.as_str()), (200, format!("hello from A; host={forwarded}; cookie=").as_str()));

    // Hidden, and shown again: the person's choice, kept here.
    let hide = |hidden| Request::HidePipeShare(v1::HidePipeShare { share_id: theirs.id.clone(), hidden });
    assert!(matches!(b.request(hide(true)).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && share.hidden)).await;
    assert!(matches!(b.request(hide(false)).await, Body::Ack(_)));

    // Paused, nothing answers; resumed, the same address and key work again.
    a.pipe_state(v1::PipeAction::Pause).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && !share.listening)).await;
    assert!(browse(&at, &host, "/", &key).await.is_err(), "listening while paused");
    a.pipe_state(v1::PipeAction::Resume).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && share.live && share.listening)).await;
    let mut answered = Err(String::new());
    for _ in 0..50 {
        answered = browse(&at, &host, "/", &key).await;
        if answered.as_ref().is_ok_and(|(status, ..)| *status == 200) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(answered.map(|(status, ..)| status), Ok(200));
    let again = b.shares().into_iter().find(|share| share.id == theirs.id).expect("B's");
    assert_eq!(again.address, theirs.address);

    // The same when B pauses and resumes it (Abbacchio, row 1106).
    b.pipe_state(v1::PipeAction::Pause).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && !share.listening)).await;
    assert!(browse(&at, &host, "/", &key).await.is_err(), "listening while paused");
    b.pipe_state(v1::PipeAction::Resume).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.id == theirs.id && share.live && share.listening)).await;
    let mut answered = Err(String::new());
    for _ in 0..50 {
        answered = browse(&at, &host, "/", &key).await;
        if answered.as_ref().is_ok_and(|(status, ..)| *status == 200) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(answered.map(|(status, ..)| status), Ok(200));

    // Told once: never again for the same share, paused and resumed.
    assert_eq!(b.notes.iter().filter(|note| !note.share_id.is_empty()).count(), 1, "{:?}", b.notes);

    // A port B opens a website at can't be shared back from B.
    let check = v1::CheckPipeSharePort { port: local.parse().expect("a number") };
    let Body::PipeSharePort(checked) = b.request(Request::CheckPipeSharePort(check)).await else { panic!("no answer") };
    assert!(!checked.problem.is_empty(), "{checked:?}");

    // Stopped: the listener closes at once.
    let mine = a.shares().into_iter().find(|share| share.mine).expect("A's");
    assert!(matches!(a.request(Request::StopPipeShare(v1::StopPipeShare { share_id: mine.id })).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.is_empty()).await;
    assert!(browse(&at, &host, "/", &key).await.is_err(), "listening after Stop");
    let open = v1::OpenPipeShare { share_id: theirs.id.clone() };
    assert!(matches!(b.request(Request::OpenPipeShare(open)).await, Body::Error(_)));
}

async fn a_shared_website_reaches_the_other_side_while_its_pipe_is_open() {
    let (mut a, mut b) = paired().await;
    let port = echo_server().await;
    let a_pipe = a.pipe().id;

    // Claktory's own ports are refused; the website's answers.
    let own = v1::CheckPipeSharePort { port: 7420 };
    let Body::PipeSharePort(checked) = a.request(Request::CheckPipeSharePort(own)).await else { panic!("no answer") };
    assert_eq!(checked.problem, "Port 7420 is Claktory's own.");
    let check = v1::CheckPipeSharePort { port: u32::from(port) };
    let Body::PipeSharePort(checked) = a.request(Request::CheckPipeSharePort(check)).await else { panic!("no answer") };
    assert!(checked.listening && checked.problem.is_empty(), "{checked:?}");
    let refused = v1::SharePipeWebsite { port: 7421, name: "Dev".into(), pipe_ids: vec![a_pipe.clone()], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(refused)).await, Body::Error(_)));

    // A shares it; B is offered it, and its chat says so.
    let share = v1::SharePipeWebsite { port: u32::from(port), name: "Dashboard".into(), pipe_ids: vec![a_pipe.clone()], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(share)).await, Body::Ack(_)));
    let mine = a.shares().into_iter().find(|share| share.mine).expect("A's share");
    assert_eq!((mine.name.as_str(), mine.port, mine.pipe_ids.clone(), mine.live), ("Dashboard", u32::from(port), vec![a_pipe.clone()], true));
    assert!(a.said("You shared Dashboard"));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| !share.mine && share.live)).await;
    let theirs = b.shares().into_iter().find(|share| !share.mine).expect("B's");
    assert_eq!((theirs.name.as_str(), theirs.port, theirs.pipe_ids.clone()), ("Dashboard", u32::from(port), vec![b.pipe().id]));
    assert!(b.said("shares Dashboard."));

    // Its bytes go both ways through the pipe.
    assert_eq!(through(&b, &theirs.id, "hello").await.as_deref(), Ok("hello"));
    // A's agents read that B opened it (#502): when, never what went through.
    let listed = pipes::shares::listing_for_test(&a.factory, &a_pipe).join("\n");
    assert!(listed.contains("; active; the other side opened it first "), "{listed}");
    assert!(!listed.contains("hello"), "{listed}");

    // A pause cuts a live stream at once and refuses new ones.
    let (mut send, mut receive) = pipes::shares::stream_for_test(&b.factory, &theirs.id).await.expect("a stream");
    send.write_all(b"live").await.expect("sent");
    let mut back = [0u8; 4];
    receive.read_exact(&mut back).await.expect("echoed");
    a.pipe_state(v1::PipeAction::Pause).await;
    let mut rest = [0u8; 1];
    let ended = tokio::time::timeout(Duration::from_secs(5), receive.read(&mut rest)).await.expect("cut, not left hanging");
    assert!(!matches!(ended, Ok(Some(n)) if n > 0), "nothing more comes");
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| !share.mine && !share.live)).await;
    assert!(through(&b, &theirs.id, "again").await.is_err());
    a.until(|view| view.workspace.pipe_shares.iter().all(|share| !share.live)).await;

    // Resumed with the same key, it works again.
    a.pipe_state(v1::PipeAction::Resume).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| !share.mine && share.live)).await;
    let mut back = Err(String::new());
    for _ in 0..50 {
        back = through(&b, &theirs.id, "again").await;
        if back.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(back.as_deref(), Ok("again"));

    // Stopped: B's goes, its chat says so, and nothing more connects.
    assert!(matches!(a.request(Request::StopPipeShare(v1::StopPipeShare { share_id: mine.id.clone() })).await, Body::Ack(_)));
    a.until(|view| view.workspace.pipe_shares.is_empty()).await;
    b.until(|view| view.workspace.pipe_shares.is_empty()).await;
    assert!(b.said("stopped sharing Dashboard."));
    assert!(through(&b, &theirs.id, "gone").await.is_err());

    // Shared with a group: it follows the group. Out of it, the pipe stops
    // getting it; back in, it gets it again.
    let group = v1::CreatePipeGroup { project_id: "site".into(), name: "Partners".into(), pipe_id: a_pipe.clone() };
    assert!(matches!(a.request(Request::CreatePipeGroup(group)).await, Body::Ack(_)));
    a.until(|view| !view.workspace.pipe_groups.is_empty()).await;
    let group = a.view.workspace.pipe_groups[0].id.clone();
    // A name with spaces, accents and a dash reaches the other side as it is (Abbacchio row 1105).
    const BOARD: &str = "Café board — Ünïcode ok";
    let share = v1::SharePipeWebsite { port: u32::from(port), name: BOARD.into(), group_id: group.clone(), ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(share)).await, Body::Ack(_)));
    a.until(|view| view.workspace.pipe_shares.iter().any(|share| share.group_id == group && share.pipe_ids == [a_pipe.clone()])).await;
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == BOARD && share.live)).await;
    assert!(b.said(&format!("shares {BOARD}.")));
    let out = v1::SetPipeGroup { pipe_id: a_pipe.clone(), group_id: String::new() };
    assert!(matches!(a.request(Request::SetPipeGroup(out)).await, Body::Ack(_)));
    a.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == BOARD && share.pipe_ids.is_empty())).await;
    b.until(|view| view.workspace.pipe_shares.iter().all(|share| share.name != BOARD)).await;
    assert!(a.said(&format!("You stopped sharing {BOARD} here: the pipe left its group.")));
    let back = v1::SetPipeGroup { pipe_id: a_pipe.clone(), group_id: group.clone() };
    assert!(matches!(a.request(Request::SetPipeGroup(back)).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == BOARD && share.live)).await;
    // Its group deleted, it stays with the pipes it reaches.
    assert!(matches!(a.request(Request::DeletePipeGroup(v1::DeletePipeGroup { group_id: group.clone() })).await, Body::Ack(_)));
    a.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == BOARD && share.group_id.is_empty() && share.pipe_ids == [a_pipe.clone()]))
        .await;
    let board = a.shares().into_iter().find(|share| share.name == BOARD).expect(BOARD);
    assert!(matches!(a.request(Request::StopPipeShare(v1::StopPipeShare { share_id: board.id })).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.is_empty()).await;

    // Shared again, then another key pinned during a pause: resume doesn't
    // bring it back, there or anywhere (Fugo 30940).
    let share = v1::SharePipeWebsite { port: u32::from(port), name: "Metrics".into(), pipe_ids: vec![a_pipe.clone()], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(share)).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == "Metrics" && share.live)).await;
    a.pipe_state(v1::PipeAction::Pause).await;
    factoryd::pipe_link::repin_for_test(&a.factory, &a_pipe, "another-key");
    a.pipe_state(v1::PipeAction::Resume).await;
    a.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == "Metrics" && share.pipe_ids.is_empty() && !share.live))
        .await;
    assert!(a.said("Metrics stopped here: the other side isn't the one it was shared with."));
    // B hears it stopped (on the connection A had before the pin changed).
    b.until(|view| view.workspace.pipe_shares.is_empty()).await;
    assert!(b.said("stopped sharing Metrics."));

    // Another account on the other side ends a share there for good, on
    // both sides: B's first, then A's.
    let notes = v1::SharePipeWebsite { port: u32::from(port), name: "Notes".into(), pipe_ids: vec![a_pipe.clone()], ..Default::default() };
    assert!(matches!(a.request(Request::SharePipeWebsite(notes)).await, Body::Ack(_)));
    b.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == "Notes" && share.live)).await;
    let (t, week) = (factoryd::transcript::now_ms() / 1000, 7 * 24 * 3600);
    let b_pipe = b.pipe().id;
    let another_account = |side: &Side, pipe: &str| {
        pipes::identity::vouch_for_test(&side.factory, true, Some(String::new()), |statement: &str, endpoint: &str, _now: i64| {
            let parts: Vec<&str> = statement.split('|').collect();
            let [account, email, iat, exp, key] = parts[..] else { return None };
            let (iat, exp): (i64, i64) = (iat.parse().ok()?, exp.parse().ok()?);
            (key == endpoint).then(|| (account.to_owned(), email.to_owned(), iat, exp))
        });
        for (account, at) in [("a1", t), ("b1", t + 1)] {
            let said = format!("{account}|{account}@example.com|{at}|{}|peer-endpoint", at + week);
            pipes::identity::heard_for_test(&side.factory, pipe, "peer-endpoint", &said);
        }
        pipes::shares::identity_heard_for_test(&side.factory, pipe);
    };
    another_account(&b, &b_pipe);
    b.until(|view| view.workspace.pipe_shares.is_empty()).await;
    assert!(b.said("Notes stopped here: this pipe's Claktory now belongs to a different account."));
    another_account(&a, &a_pipe);
    a.until(|view| view.workspace.pipe_shares.iter().any(|share| share.name == "Notes" && share.pipe_ids.is_empty() && !share.live)).await;
    assert!(a.said("Notes stopped here: this pipe's Claktory now belongs to a different account."));

    // Closed: what was shared with it is over.
    a.pipe_state(v1::PipeAction::Close).await;
    a.until(|view| view.workspace.pipe_shares.is_empty()).await;
}
