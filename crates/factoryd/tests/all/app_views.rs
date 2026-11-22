//! App views (C1H): an app with views opens one as a surface when a device
//! asks, the same one for every device showing that view with those
//! params; devices get it whole, then each change, only while they show
//! it; actions reach the app only from a component that has them; an
//! invalid message fails the surface, a stopped app's surfaces wait for it
//! and open again, and nobody showing one for a while closes it. The host
//! is a stand-in speaking its protocol.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use factory_protocol::apps::{self as wire, Frame, frame::Body as Host};
use factory_protocol::v1::{self, AppSurfaceState as State, client_message::Body as Request, server_message::Body};
use factoryd::apps::AppsLink;
use prost::Message;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::apps::clapp;
use crate::common::{Client, start, temp_dir};

const CATALOG: &str = "https://claktory.com/a2ui/catalog/v1";

const MANIFEST: &str = r#"
id = "charts"
name = "Charts"
version = "0.1.0"
world = "claktory:app@1.1"
publisher = "claktory"

[[views]]
id = "main"
title = "Charts"

[[views]]
id = "blank"
title = "Blank"

[[views]]
id = "refused"
title = "Refused"
"#;

/// A stand-in host running a views app: view "main" pushes a surface with
/// a title from the data model and two buttons, "go" (sets the title) and
/// "brk" (makes the app push something invalid); "blank" shows nothing;
/// "refused" refuses.
#[derive(Clone, Default)]
struct Stand {
    log: Arc<Mutex<Vec<String>>>,
}

fn create(surface: &str, params: &str) -> String {
    json!({"version": "1.0", "createSurface": {"surfaceId": surface, "catalogId": CATALOG,
        "components": [
            {"id": "root", "component": "Column", "children": ["title", "go", "brk"]},
            {"id": "title", "component": "Text", "text": {"@path": "/title"}},
            {"id": "go_label", "component": "Text", "text": "Go"},
            {"id": "go", "component": "Button", "child": "go_label", "action": {"event": {"name": "go", "context": {"q": {"@path": "/q"}}}}},
            {"id": "brk", "component": "Button", "child": "go_label", "action": {"event": {"name": "break"}}},
        ],
        "dataModel": {"title": params}}})
    .to_string()
}

impl Stand {
    fn log(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
    }

    fn note(&self, line: String) {
        self.log.lock().expect("log").push(line);
    }

    async fn wait_for(&self, line: &str) {
        for _ in 0..400 {
            if self.log().iter().any(|logged| logged == line) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("the host never logged \"{line}\": {:?}", self.log());
    }

    fn launcher(&self) -> factoryd::apps::Launcher {
        let stand = self.clone();
        Arc::new(move || {
            let (ours, theirs) = tokio::io::duplex(1 << 22);
            let (their_read, their_write) = tokio::io::split(theirs);
            tokio::spawn(stand.clone().serve(ours));
            Ok(AppsLink { reader: Box::new(their_read), writer: Box::new(their_write), child: None })
        })
    }

    async fn serve(self, stream: tokio::io::DuplexStream) {
        let (mut read, mut write) = tokio::io::split(stream);
        let (out, mut outgoing) = mpsc::unbounded_channel::<Frame>();
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                let bytes = frame.encode_to_vec();
                if write.write_all(&(bytes.len() as u32).to_be_bytes()).await.is_err() || write.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        });
        let mut next = 1_000;
        let mut push = |app: &str, surface: &str, message: String| {
            next += 1;
            let push = wire::PushView { app_id: app.into(), surface: surface.into(), message };
            out.send(Frame { id: next, reply_to: 0, body: Some(Host::PushView(push)) }).expect("pushed");
        };
        loop {
            let mut length = [0u8; 4];
            if read.read_exact(&mut length).await.is_err() {
                return;
            }
            let mut bytes = vec![0; u32::from_be_bytes(length) as usize];
            if read.read_exact(&mut bytes).await.is_err() {
                return;
            }
            let frame = Frame::decode(bytes.as_slice()).expect("a frame");
            let done = |error: &str| Frame { id: 0, reply_to: frame.id, body: Some(Host::Done(wire::Done { error: error.to_owned() })) };
            let reply = match frame.body.expect("a body") {
                Host::Done(answer) => {
                    self.note(format!("push answered {}", if answer.error.is_empty() { "ok" } else { &answer.error }));
                    continue;
                }
                Host::Hello(_) => Frame {
                    id: 0,
                    reply_to: frame.id,
                    body: Some(Host::Ready(wire::Ready {
                        version: "stand-in".into(),
                        worlds: vec!["claktory:app@1".into(), "claktory:app@1.1".into()],
                    })),
                },
                Host::Load(load) => {
                    self.note(format!("load {}", load.world));
                    done("")
                }
                Host::Start(start) => {
                    self.note(format!("start {}", start.app_id));
                    done("")
                }
                Host::Stop(stop) => {
                    self.note(format!("stop {}", stop.app_id));
                    done("")
                }
                Host::Unload(_) => done(""),
                Host::ViewOpen(open) => {
                    self.note(format!("open {} {}", open.view, open.params));
                    match open.view.as_str() {
                        "main" => {
                            push(&open.app_id, &open.surface, create(&open.surface, &open.params));
                            done("")
                        }
                        "blank" => done(""),
                        _ => done("not today"),
                    }
                }
                Host::ViewAction(action) => {
                    let message: Value = serde_json::from_str(&action.message).expect("JSON");
                    let name = message["action"]["name"].as_str().unwrap_or("error").to_owned();
                    self.note(format!("action {name} {}", message["action"]["context"]));
                    if name == "go" {
                        let update =
                            json!({"version": "1.0", "updateDataModel": {"surfaceId": action.surface, "path": "/title", "value": "went"}});
                        push(&action.app_id, &action.surface, update.to_string());
                    } else if name == "break" {
                        push(&action.app_id, &action.surface, r#"{"version":"1.0","updateComponents":{"surfaceId":"x"}}"#.into());
                    }
                    done("")
                }
                Host::ViewClosed(closed) => {
                    self.note(format!("closed {}", closed.surface));
                    done("")
                }
                other => panic!("the stand-in host got {other:?}"),
            };
            if out.send(reply).is_err() {
                return;
            }
        }
    }
}

fn open(view: &str, params: &str, catalogs: &[&str]) -> Request {
    Request::OpenAppView(v1::OpenAppView {
        app_id: "charts".into(),
        view: view.into(),
        params: params.into(),
        catalogs: catalogs.iter().map(|catalog| catalog.to_string()).collect(),
    })
}

fn surface(body: Body) -> v1::AppSurface {
    match body {
        Body::AppSurface(surface) => surface,
        other => panic!("expected AppSurface, got {other:?}"),
    }
}

fn failure(body: &Body) -> (v1::ErrorCode, String) {
    match body {
        Body::Error(error) => (error.code(), error.message.clone()),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// The next AppSurface for `id` in state `state`.
async fn state_of(client: &mut Client, id: &str, state: State) -> v1::AppSurface {
    client
        .wait_for(|body| {
            matches!(body, Body::AppSurface(found) if found.surface_id == id && found.state() == state).then(|| surface(body.clone()))
        })
        .await
}

fn action(surface: &str, name: &str, source: &str) -> Request {
    let message = json!({"version": "1.0", "action": {"name": name, "surfaceId": surface, "sourceComponentId": source,
        "timestamp": "2026-11-22T10:00:00Z", "context": {"q": "tokens"}}});
    Request::AppViewAction(v1::AppViewAction { surface_id: surface.into(), message: message.to_string() })
}

fn data(surface: &v1::AppSurface) -> Value {
    let snapshot: Value = serde_json::from_str(&surface.snapshot).expect("a snapshot");
    snapshot["createSurface"]["dataModel"].clone()
}

#[tokio::test]
async fn a_view_is_one_surface_for_every_device_kept_in_step() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_app_view_grace(Duration::from_millis(300));
    let stand = Stand::default();
    factory.set_apps_host(stand.launcher());
    let (mut mac, _, _) = Client::connect(addr).await;
    let (mut phone, _, _) = Client::connect(addr).await;
    let (mut old, _, _) = Client::connect(addr).await;

    let bundle = clapp(MANIFEST, b"\0asm\x01\0\0\0 views", &SigningKey::from_bytes(&[9; 32]));
    let install = |approve: bool, sha256: &str| {
        Request::InstallLocalApp(v1::InstallLocalApp { path: String::new(), approve, sha256: sha256.into(), content: bundle.clone(), ..Default::default() })
    };
    let Body::AppPermissions(permissions) = mac.request(install(false, "")).await.body else { panic!("permissions") };
    mac.ok(install(true, &permissions.sha256)).await;
    let info = mac
        .wait_for(|body| match body {
            Body::Workspace(workspace) => workspace.apps.first().filter(|app| app.state() == v1::AppState::Running).cloned(),
            _ => None,
        })
        .await;
    let views: Vec<(&str, &str)> = info.views.iter().map(|view| (view.id.as_str(), view.title.as_str())).collect();
    assert_eq!(views, [("main", "Charts"), ("blank", "Blank"), ("refused", "Refused")]);
    stand.wait_for("load claktory:app@1.1").await;

    // A device that can't render the catalog is told so, and nothing opens.
    let unsupported = surface(mac.request(open("", "", &["https://a2ui.org/basic"])).await.body);
    assert_eq!((unsupported.state(), unsupported.surface_id.as_str(), unsupported.title.as_str()), (State::Unsupported, "", "Charts"));
    let (code, why) = failure(&mac.request(open("nope", "", &[CATALOG])).await.body);
    assert_eq!((code, why.as_str()), (v1::ErrorCode::NotFound, "The Charts app has no view \"nope\"."));
    assert!(failure(&mac.request(open("", "[1]", &[CATALOG])).await.body).1.contains("a JSON object"));

    // The first view, opened by the Mac: opening, then open with what the app pushed.
    let opened = surface(mac.request(open("", r#"{"range": "7d", "agent": "a"}"#, &[CATALOG])).await.body);
    assert_eq!((opened.view.as_str(), opened.params.as_str()), ("main", r#"{"agent":"a","range":"7d"}"#));
    assert!(opened.surface_id.starts_with("charts-"));
    let id = opened.surface_id.clone();
    let shown = if opened.state() == State::Open { opened } else { state_of(&mut mac, &id, State::Open).await };
    assert_eq!((shown.seq, shown.catalog.as_str()), (1, CATALOG));
    assert_eq!(data(&shown), json!({"title": r#"{"agent":"a","range":"7d"}"#}));
    stand.wait_for(r#"open main {"agent":"a","range":"7d"}"#).await;

    // The phone, the same view with the same params in another order: the same surface, whole.
    let same = surface(phone.request(open("main", r#"{"agent":"a","range":"7d"}"#, &[CATALOG])).await.body);
    assert_eq!((same.surface_id.as_str(), same.state(), same.seq), (id.as_str(), State::Open, 1));
    assert_eq!(same.snapshot, shown.snapshot);
    assert_eq!(stand.log().iter().filter(|line| line.starts_with("open main")).count(), 1, "the app opened it once");

    // An action from the Mac: the app hears it, both devices get the change.
    mac.ok(action(&id, "go", "go")).await;
    stand.wait_for(r#"action go {"q":"tokens"}"#).await;
    for device in [&mut mac, &mut phone] {
        let update = device.wait_for(|body| match body {
            Body::AppViewUpdate(update) if update.surface_id == id => Some(update.clone()),
            _ => None,
        });
        let update = update.await;
        assert_eq!(update.seq, 2);
        let message: Value = serde_json::from_str(&update.message).expect("JSON");
        assert_eq!(message["updateDataModel"]["value"], "went");
    }
    let again = surface(phone.request(open("main", r#"{"range":"7d","agent":"a"}"#, &[CATALOG])).await.body);
    assert_eq!((again.seq, data(&again)), (2, json!({"title": "went"})), "a fresh snapshot has the change");

    // Only components with that action can send it.
    let (code, why) = failure(&mac.request(action(&id, "go", "title")).await.body);
    assert_eq!(code, v1::ErrorCode::InvalidRequest);
    assert!(why.contains("no component \"title\" with an action"), "{why}");
    assert!(failure(&mac.request(action(&id, "delete", "go")).await.body).1.contains("isn't \"delete\""));
    let (code, _) = failure(&mac.request(action("charts-0000", "go", "go")).await.body);
    assert_eq!(code, v1::ErrorCode::NotFound);

    // Something invalid from the app fails the surface for both, keeping what it showed.
    mac.ok(action(&id, "break", "brk")).await;
    let failed = state_of(&mut mac, &id, State::Failed).await;
    assert!(failed.error.starts_with("The Charts app sent something Claktory can't show: VALIDATION_FAILED"), "{}", failed.error);
    assert_eq!(data(&failed), json!({"title": "went"}));
    state_of(&mut phone, &id, State::Failed).await;
    stand.wait_for(&format!("closed {id}")).await;
    assert!(stand.log().iter().any(|line| line.starts_with("push answered VALIDATION_FAILED")), "{:?}", stand.log());
    assert_eq!(failure(&mac.request(action(&id, "go", "go")).await.body).0, v1::ErrorCode::NotFound);

    // Opening it again makes a new one.
    let fresh = surface(mac.request(open("", r#"{"range":"7d","agent":"a"}"#, &[CATALOG])).await.body);
    assert_ne!(fresh.surface_id, id);
    let id = fresh.surface_id.clone();
    let fresh = if fresh.state() == State::Open { fresh } else { state_of(&mut mac, &id, State::Open).await };
    assert_eq!(data(&fresh), json!({"title": r#"{"agent":"a","range":"7d"}"#}));
    let on_phone = surface(phone.request(open("main", r#"{"agent":"a","range":"7d"}"#, &[CATALOG])).await.body);
    assert_eq!(on_phone.surface_id, id);

    // An app that opens a view without showing it, or refuses: failed, saying so.
    let blank = surface(mac.request(open("blank", "", &[CATALOG])).await.body);
    let blank = state_of(&mut mac, &blank.surface_id, State::Failed).await;
    assert_eq!(blank.error, "The Charts app opened this view without showing anything.");
    let refused = surface(mac.request(open("refused", "", &[CATALOG])).await.body);
    let refused = state_of(&mut mac, &refused.surface_id, State::Failed).await;
    assert_eq!(refused.error, "The Charts app couldn't show this view: not today");

    // Turned off, its surface waits; turned on, it opens again.
    mac.ok(Request::SetAppRunning(v1::SetAppRunning { id: "charts".into(), running: false })).await;
    let stopped = state_of(&mut mac, &id, State::Stopped).await;
    assert_eq!(
        (stopped.error.as_str(), stopped.snapshot.as_str()),
        ("The Charts app isn't running now. This opens again once it runs.", "")
    );
    let while_stopped = surface(phone.request(open("blank", "", &[CATALOG])).await.body);
    assert_eq!(while_stopped.state(), State::Stopped, "opening it while the app is off");
    mac.ok(Request::SetAppRunning(v1::SetAppRunning { id: "charts".into(), running: true })).await;
    state_of(&mut mac, &id, State::Opening).await;
    let reopened = state_of(&mut mac, &id, State::Open).await;
    assert_eq!(data(&reopened), json!({"title": r#"{"agent":"a","range":"7d"}"#}));

    // A device that never opened a view never heard of any.
    // (Everything sent to it so far comes before this answer.)
    let heard = old.request(Request::SetHighlights(v1::SetHighlights { on: false })).await;
    assert!(matches!(heard.body, Body::Ack(_)), "{:?}", heard.body);
    assert!(heard.events.len() > 3, "it heard the app's changes in the workspace: {}", heard.events.len());
    assert!(
        !heard.events.iter().any(|event| matches!(event.body, Some(Body::AppSurface(_) | Body::AppViewUpdate(_)))),
        "{:?}",
        heard.events
    );

    // The Mac closes it, but the phone still shows it; once the phone has
    // gone too (its connection ended), after the grace, the app hears it's
    // closed.
    mac.ok(Request::CloseAppView(v1::CloseAppView { surface_id: id.clone() })).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(!stand.log().contains(&format!("closed {id}")), "{:?}", stand.log());
    drop(phone);
    stand.wait_for(&format!("closed {id}")).await;
    drop(mac);

    // Uninstalled: a device showing a view hears it closed.
    let (mut late, _, _) = Client::connect(addr).await;
    let last = surface(late.request(open("", "", &[CATALOG])).await.body);
    let last = if last.state() == State::Open { last } else { state_of(&mut late, &last.surface_id, State::Open).await };
    late.ok(Request::UninstallApp(v1::UninstallApp { id: "charts".into(), keep_data: false })).await;
    let closed = state_of(&mut late, &last.surface_id, State::Closed).await;
    assert_eq!(closed.error, "The Charts app was uninstalled.");
}

/// The same, end to end with the real apps host and a real app: runs when
/// `CLAKTORY_APPS_HOST_BIN` names the claktory-apps binary and
/// `CLAKTORY_APPS_VIEWS_GUEST` the views test app (crates/claktory-apps'
/// tests build it into target/tmp/guest/wasm32-wasip2/release/
/// claktory_apps_test_guest.wasm).
#[tokio::test]
async fn the_real_host_runs_a_real_apps_views() {
    let (Some(host), Some(guest)) = (std::env::var_os("CLAKTORY_APPS_HOST_BIN"), std::env::var_os("CLAKTORY_APPS_VIEWS_GUEST")) else {
        eprintln!("skipped: set CLAKTORY_APPS_HOST_BIN and CLAKTORY_APPS_VIEWS_GUEST to run it");
        return;
    };
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let launcher: factoryd::apps::Launcher = Arc::new(move || {
        let mut child = tokio::process::Command::new(&host)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let (stdin, stdout) = (child.stdin.take().expect("stdin"), child.stdout.take().expect("stdout"));
        Ok(AppsLink { reader: Box::new(stdout), writer: Box::new(stdin), child: Some(child) })
    });
    factory.set_apps_host(launcher);
    let (mut client, _, _) = Client::connect(addr).await;
    let manifest = "id = \"viewer\"\nname = \"Viewer\"\nversion = \"1.0.0\"\nworld = \"claktory:app@1.1\"\n\n\
                    [[views]]\nid = \"main\"\ntitle = \"Main\"\n\n[[views]]\nid = \"invalid\"\ntitle = \"Invalid\"\n";
    let wasm = std::fs::read(guest).expect("the guest");
    let bundle = clapp(manifest, &wasm, &SigningKey::from_bytes(&[5; 32]));
    let install = |approve: bool, sha256: &str| {
        Request::InstallLocalApp(v1::InstallLocalApp { path: String::new(), approve, sha256: sha256.into(), content: bundle.clone(), ..Default::default() })
    };
    let Body::AppPermissions(permissions) = client.request(install(false, "")).await.body else { panic!("permissions") };
    client.ok(install(true, &permissions.sha256)).await;
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) => workspace.apps.first().filter(|app| app.state() == v1::AppState::Running).map(|_| ()),
            _ => None,
        })
        .await;

    let open = |view: &str| {
        Request::OpenAppView(v1::OpenAppView {
            app_id: "viewer".into(),
            view: view.into(),
            params: r#"{"id":"a"}"#.into(),
            catalogs: vec![CATALOG.into()],
        })
    };
    let opened = surface(client.request(open("")).await.body);
    let shown = if opened.state() == State::Open { opened } else { state_of(&mut client, &opened.surface_id, State::Open).await };
    let snapshot: Value = serde_json::from_str(&shown.snapshot).expect("JSON");
    assert_eq!(snapshot["createSurface"]["components"], json!([{"id": "root", "component": "Text", "text": r#"{"id":"a"}"#}]));

    let invalid = surface(client.request(open("invalid")).await.body);
    let failed = state_of(&mut client, &invalid.surface_id, State::Failed).await;
    assert!(failed.error.contains("VALIDATION_FAILED at /version"), "{}", failed.error);
    client.ok(Request::CloseAppView(v1::CloseAppView { surface_id: shown.surface_id })).await;
}
