//! App data (C2D): an app reads only the sources its manifest asks for and
//! the person granted, checked again on every query; queries run read
//! only in factoryd, one SELECT or WITH at a time, never reaching agents'
//! memory, pipes or comments on files; an app that pushes its views more
//! than a megabyte a second fails them. The host is a stand-in speaking
//! its protocol.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ed25519_dalek::SigningKey;
use factory_protocol::apps::{self as wire, DataError, Frame, frame::Body as Host};
use factory_protocol::v1::{self, AppSurfaceState as State, client_message::Body as Request, server_message::Body};
use factoryd::apps::AppsLink;
use prost::Message;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

use crate::apps::clapp;
use crate::common::{Client, home_of, last_workspace, start, temp_dir};

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

[[data]]
kind = "factory-records"
reason = "Charts of how your agents work"

[[data]]
kind = "agent-database"
agent = "*"
reason = "Charts of what your agents keep"
"#;

/// A stand-in host: answers factoryd, and asks it what the test says,
/// as the running app would.
#[derive(Clone, Default)]
pub(crate) struct Stand {
    log: Arc<Mutex<Vec<String>>>,
    tell: Arc<Mutex<Option<mpsc::UnboundedSender<Frame>>>>,
    answers: Arc<Mutex<HashMap<u64, Host>>>,
    next: Arc<Mutex<u64>>,
}

impl Stand {
    fn log(&self) -> Vec<String> {
        self.log.lock().expect("log").clone()
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

    /// Sends factoryd `body` as the app; its answer.
    async fn ask(&self, body: Host) -> Host {
        let id = self.send(body);
        self.answer(id).await
    }

    /// Sends factoryd `body` as the app, without waiting: its frame's id.
    fn send(&self, body: Host) -> u64 {
        let id = {
            let mut next = self.next.lock().expect("next");
            *next += 1;
            *next + 1_000
        };
        let tell = self.tell.lock().expect("tell").clone().expect("the host is running");
        tell.send(Frame { id, reply_to: 0, body: Some(body) }).expect("sent");
        id
    }

    async fn answer(&self, id: u64) -> Host {
        for _ in 0..800 {
            if let Some(answer) = self.answers.lock().expect("answers").remove(&id) {
                return answer;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        panic!("factoryd never answered {id}");
    }

    async fn query(&self, source: &str, sql: &str, params: &str) -> wire::DataAnswer {
        let asked = wire::DataQuery { app_id: "charts".into(), source: source.into(), sql: sql.into(), params: params.into() };
        match self.ask(Host::DataQuery(asked)).await {
            Host::DataAnswer(answer) => answer,
            other => panic!("expected DataAnswer, got {other:?}"),
        }
    }

    async fn sources(&self) -> Vec<(String, String, bool)> {
        match self.ask(Host::GetDataSources(wire::GetDataSources { app_id: "charts".into() })).await {
            Host::DataSourceList(list) => list.sources.into_iter().map(|source| (source.id, source.label, source.granted)).collect(),
            other => panic!("expected DataSourceList, got {other:?}"),
        }
    }

    pub(crate) fn launcher(&self) -> factoryd::apps::Launcher {
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
        *self.tell.lock().expect("tell") = Some(out.clone());
        tokio::spawn(async move {
            while let Some(frame) = outgoing.recv().await {
                let bytes = frame.encode_to_vec();
                if write.write_all(&(bytes.len() as u32).to_be_bytes()).await.is_err() || write.write_all(&bytes).await.is_err() {
                    return;
                }
            }
        });
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
            if frame.reply_to != 0 {
                self.answers.lock().expect("answers").insert(frame.reply_to, frame.body.expect("a body"));
                continue;
            }
            let done = |error: &str| Frame { id: 0, reply_to: frame.id, body: Some(Host::Done(wire::Done { error: error.to_owned() })) };
            let reply = match frame.body.expect("a body") {
                Host::Hello(_) => Frame {
                    id: 0,
                    reply_to: frame.id,
                    body: Some(Host::Ready(wire::Ready {
                        version: "stand-in".into(),
                        worlds: vec!["claktory:app@1".into(), "claktory:app@1.1".into()],
                    })),
                },
                Host::Load(load) => {
                    self.log.lock().expect("log").push(format!("load {}", load.world));
                    done("")
                }
                Host::Start(start) => {
                    self.log.lock().expect("log").push(format!("start {}", start.app_id));
                    done("")
                }
                Host::Stop(_) | Host::Unload(_) => done(""),
                Host::ViewOpen(open) => {
                    let create = json!({"version": "1.0", "createSurface": {"surfaceId": open.surface, "catalogId": CATALOG,
                        "components": [{"id": "root", "component": "Text", "text": {"@path": "/title"}}], "dataModel": {"title": "hi"}}});
                    let push = wire::PushView { app_id: open.app_id, surface: open.surface, message: create.to_string() };
                    out.send(Frame { id: 999, reply_to: 0, body: Some(Host::PushView(push)) }).expect("pushed");
                    done("")
                }
                Host::ViewClosed(closed) => {
                    self.log.lock().expect("log").push(format!("closed {}", closed.surface));
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

fn grant(source: &str, granted: bool) -> Request {
    Request::GrantAppSource(v1::GrantAppSource { app_id: "charts".into(), source_id: source.into(), granted })
}

fn rows(answer: &wire::DataAnswer) -> Value {
    assert_eq!(answer.error(), DataError::None, "{}", answer.message);
    let rows: Value = serde_json::from_str(&answer.json).expect("JSON");
    rows["rows"].clone()
}

async fn running(client: &mut Client) -> v1::AppInfo {
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) => workspace.apps.first().filter(|app| app.state() == v1::AppState::Running).cloned(),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn an_app_reads_only_what_the_person_granted_and_only_to_read() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let stand = Stand::default();
    factory.set_apps_host(stand.launcher());
    let (mut client, _, _) = Client::connect(addr).await;

    // Rivet keeps a table of its own.
    let own = home_of(addr).agent_dir("rivet").join("database.sqlite");
    std::fs::create_dir_all(own.parent().expect("a folder")).expect("its folder");
    let db = rusqlite::Connection::open(&own).expect("its database");
    db.execute_batch(
        "PRAGMA journal_mode = wal; CREATE TABLE spend (day TEXT, usd REAL); INSERT INTO spend VALUES ('mon', 1.5), ('tue', 2.25); \
         CREATE TABLE memory_notes (body TEXT);",
    )
    .expect("its table");
    drop(db);

    // The review says what it asks to read, and why.
    let bundle = clapp(MANIFEST, b"\0asm\x01\0\0\0 data", &SigningKey::from_bytes(&[11; 32]));
    let install = |approve: bool, sha256: &str| {
        Request::InstallLocalApp(v1::InstallLocalApp { path: String::new(), approve, sha256: sha256.into(), content: bundle.clone() })
    };
    let Body::AppPermissions(permissions) = client.request(install(false, "")).await.body else { panic!("permissions") };
    let asked: Vec<(&str, &str, &str)> =
        permissions.sources.iter().map(|source| (source.id.as_str(), source.label.as_str(), source.reason.as_str())).collect();
    assert_eq!(
        asked,
        [
            ("factory-records", "Claktory's records", "Charts of how your agents work"),
            ("agent:*", "Agents' own databases (you choose which)", "Charts of what your agents keep"),
        ]
    );
    client.ok(install(true, &permissions.sha256)).await;
    let info = running(&mut client).await;
    let offered: Vec<(&str, v1::AppDataKind, &str, bool)> =
        info.sources.iter().map(|source| (source.id.as_str(), source.kind(), source.agent_id.as_str(), source.granted)).collect();
    assert_eq!(
        offered,
        [("factory-records", v1::AppDataKind::FactoryRecords, "", false), ("agent:rivet", v1::AppDataKind::AgentDatabase, "rivet", false)]
    );
    assert_eq!(info.sources[1].label, "Rivet's database");
    stand.wait_for("start charts").await;

    // Nothing granted: nothing read.
    assert_eq!(
        stand.sources().await,
        [("factory-records".into(), "Claktory's records".into(), false), ("agent:rivet".into(), "Rivet's database".into(), false)]
    );
    assert_eq!(stand.query("factory-records", "SELECT 1", "").await.error(), DataError::NotGranted);
    assert_eq!(stand.query("agent:rivet", "SELECT 1", "").await.error(), DataError::NotGranted);
    assert_eq!(stand.query("agent:nobody", "SELECT 1", "").await.error(), DataError::UnknownSource);

    // Only sources it asks for can be granted.
    let refused = client.request(grant("agent:nobody", true)).await.body;
    assert!(
        matches!(&refused, Body::Error(error) if error.code() == v1::ErrorCode::InvalidRequest && error.message.contains("doesn't ask to read")),
        "{refused:?}"
    );

    // Claktory's records, granted: read, but never memory, pipes or comments, and never written.
    let events = client.ok(grant("factory-records", true)).await;
    let info = &last_workspace(&events).expect("the workspace, before the answer").apps[0];
    assert!(info.sources[0].granted && !info.sources[1].granted);
    assert_eq!(rows(&stand.query("factory-records", "SELECT ?1 + 1, 'a'", "[41]").await), json!([[42, "a"]]));
    assert_eq!(rows(&stand.query("factory-records", "SELECT count(*) > 0 FROM sqlite_master WHERE name = 'runs'", "").await), json!([[1]]));
    for private in ["SELECT * FROM pipes", "SELECT * FROM threads", "SELECT * FROM memory_notes"] {
        let answer = stand.query("factory-records", private, "").await;
        assert!(answer.error() == DataError::Invalid && answer.message.contains("can't be read"), "{private}: {answer:?}");
    }
    for (write, why) in [
        ("DELETE FROM runs", "Only a SELECT"),
        ("SELECT 1; DELETE FROM runs", "One statement at a time"),
        ("ATTACH 'x.db' AS x", "Only a SELECT"),
        ("WITH d AS (SELECT 1) INSERT INTO runs SELECT * FROM runs", "Only statements that read"),
    ] {
        let answer = stand.query("factory-records", write, "").await;
        assert!(answer.error() == DataError::Invalid && answer.message.contains(why), "{write}: {answer:?}");
    }

    // An agent's own database, granted, then taken back: refused at once.
    client.ok(grant("agent:rivet", true)).await;
    assert_eq!(
        rows(&stand.query("agent:rivet", "SELECT day, usd FROM spend ORDER BY day", "").await),
        json!([["mon", 1.5], ["tue", 2.25]])
    );
    let deleted = stand.query("agent:rivet", "WITH d AS (SELECT 1) DELETE FROM spend", "").await;
    assert_eq!(deleted.error(), DataError::Invalid, "{deleted:?}");
    let private = stand.query("agent:rivet", "SELECT * FROM memory_notes", "").await;
    assert!(private.error() == DataError::Invalid && private.message.contains("can't be read"), "{private:?}");

    // An agent's database that's a link (to factory.db, say) isn't read.
    let kept_aside = own.with_extension("kept");
    std::fs::rename(&own, &kept_aside).expect("set aside");
    for link in [std::os::unix::fs::symlink::<std::path::PathBuf, &std::path::PathBuf>, std::fs::hard_link] {
        link(home_of(addr).db_path(), &own).expect("linked");
        let linked = stand.query("agent:rivet", "SELECT count(*) FROM runs", "").await;
        assert!(linked.error() == DataError::Failed && linked.message.contains("a link"), "{linked:?}");
        std::fs::remove_file(&own).expect("unlinked");
    }
    std::fs::rename(&kept_aside, &own).expect("put back");
    client.ok(grant("agent:rivet", false)).await;
    assert_eq!(stand.query("agent:rivet", "SELECT day FROM spend", "").await.error(), DataError::NotGranted);
    let kept: i64 =
        rusqlite::Connection::open(&own).expect("reopened").query_row("SELECT count(*) FROM spend", [], |row| row.get(0)).expect("count");
    assert_eq!(kept, 2, "nothing was written");
    assert_eq!(
        stand.sources().await,
        [("factory-records".into(), "Claktory's records".into(), true), ("agent:rivet".into(), "Rivet's database".into(), false)]
    );
    let config = std::fs::read_to_string(home_of(addr).config_path()).expect("config.json");
    let config: Value = serde_json::from_str(&config).expect("JSON");
    assert_eq!(config["apps"][0]["sources"], json!(["factory-records"]));
}

#[tokio::test]
async fn an_app_that_pushes_too_much_fails_its_views() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let stand = Stand::default();
    factory.set_apps_host(stand.launcher());
    let (mut client, _, _) = Client::connect(addr).await;
    let bundle = clapp(MANIFEST, b"\0asm\x01\0\0\0 data", &SigningKey::from_bytes(&[11; 32]));
    let install = |approve: bool, sha256: &str| {
        Request::InstallLocalApp(v1::InstallLocalApp { path: String::new(), approve, sha256: sha256.into(), content: bundle.clone() })
    };
    let Body::AppPermissions(permissions) = client.request(install(false, "")).await.body else { panic!("permissions") };
    client.ok(install(true, &permissions.sha256)).await;
    running(&mut client).await;
    stand.wait_for("start charts").await;

    let open = Request::OpenAppView(v1::OpenAppView {
        app_id: "charts".into(),
        view: String::new(),
        params: String::new(),
        catalogs: vec![CATALOG.into()],
    });
    let Body::AppSurface(opened) = client.request(open).await.body else { panic!("a surface") };
    let id = opened.surface_id.clone();
    if opened.state() != State::Open {
        client
            .wait_for(|body| {
                matches!(body, Body::AppSurface(found) if found.surface_id == id && found.state() == State::Open).then_some(())
            })
            .await;
    }

    // Three pushes of 600 KB at once: two of them in the same second are
    // past 1 MiB, however slow the machine.
    let big = "x".repeat(600_000);
    let sent: Vec<u64> = (0..3)
        .map(|_| {
            let update = json!({"version": "1.0", "updateDataModel": {"surfaceId": id, "path": "/title", "value": big}});
            stand.send(Host::PushView(wire::PushView { app_id: "charts".into(), surface: id.clone(), message: update.to_string() }))
        })
        .collect();
    let mut errors = Vec::new();
    for id in sent {
        let Host::Done(done) = stand.answer(id).await else { panic!("Done") };
        errors.push(done.error);
    }
    assert!(errors[0].is_empty(), "{errors:?}");
    assert!(errors[1..].iter().any(|error| error.contains("Too much at once")), "{errors:?}");
    let failed = client
        .wait_for(|body| match body {
            Body::AppSurface(found) if found.surface_id == id && found.state() == State::Failed => Some(found.clone()),
            _ => None,
        })
        .await;
    assert_eq!(failed.error, "The Charts app sent too much at once: an app's views take at most 1048576 bytes a second.");
    stand.wait_for(&format!("closed {id}")).await;
}
