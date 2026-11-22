//! Moving a Claktory over the network, the target's side (#217, phase 2):
//! `factoryd receive` shows a code, lets in one source once both people
//! confirm the same digits, takes its rounds of blocks (resuming after a
//! drop, never writing through a link), imports, says it's ready, and
//! finishes only once the source commits. A source here is played by the
//! test, frame by frame, as docs/MOVE.md says.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{Connection, RecvStream, SendStream, presets};
use iroh::{Endpoint, EndpointAddr, RelayMode, SecretKey};
use tokio::sync::mpsc;

use crate::common::{Client, start, temp_dir};
use crate::move_import::{bundle, run, source};
use factoryd::home::Home;
use factoryd::move_import::receive::{Asked, Options, Ui};
use factoryd::move_wire::{self, Entry, Frame, Kind, ReceiveCode};

const WAIT: Duration = Duration::from_secs(30);

/// What the person at the target sees and answers.
struct Person {
    codes: mpsc::UnboundedReceiver<ReceiveCode>,
    said: Arc<Mutex<Vec<String>>>,
    asked: Arc<Mutex<Vec<Asked>>>,
    answers: mpsc::UnboundedSender<bool>,
}

impl Person {
    async fn code(&mut self) -> ReceiveCode {
        let said = self.said();
        tokio::time::timeout(WAIT, self.codes.recv())
            .await
            .expect("a code in time")
            .unwrap_or_else(|| panic!("a code; receive said: {said}"))
    }

    fn said(&self) -> String {
        self.said.lock().unwrap().join("\n")
    }
}

/// `factoryd receive` into `home`, running; and its person.
fn receive(home: &Home) -> (tokio::task::JoinHandle<Result<String, factoryd::move_import::ImportError>>, Person) {
    let (code_tx, codes) = mpsc::unbounded_channel();
    let (answers, answer_rx) = mpsc::unbounded_channel::<bool>();
    let answer_rx = Arc::new(tokio::sync::Mutex::new(answer_rx));
    let said = Arc::new(Mutex::new(Vec::new()));
    let asked = Arc::new(Mutex::new(Vec::new()));
    let ui = Ui {
        code: Arc::new(move |code| drop(code_tx.send(code.clone()))),
        say: {
            let said = said.clone();
            Arc::new(move |text| said.lock().unwrap().push(text.to_owned()))
        },
        confirm: {
            let asked = asked.clone();
            Arc::new(move |question| {
                asked.lock().unwrap().push(question);
                let answers = answer_rx.clone();
                Box::pin(async move { answers.lock().await.recv().await.unwrap_or(false) })
            })
        },
    };
    let home = home.clone();
    let task = tokio::spawn(async move {
        let options = Options { committed: false, local_only: true };
        // A port no Claktory listens on, which no other test takes meanwhile.
        let nowhere = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        factoryd::move_import::receive::receive(&home, nowhere, &options, &ui).await
    });
    (task, Person { codes, said, asked, answers })
}

/// The source's end of a connection.
struct Wire {
    connection: Connection,
    send: SendStream,
    receive: RecvStream,
}

impl Wire {
    async fn dial(endpoint: &Endpoint, code: &ReceiveCode) -> Wire {
        let mut addr = EndpointAddr::new(code.endpoint.parse().expect("id"));
        for address in &code.addresses {
            addr = addr.with_ip_addr(*address);
        }
        let connection = tokio::time::timeout(WAIT, endpoint.connect(addr, move_wire::ALPN)).await.expect("in time").expect("connects");
        let (send, receive) = connection.open_bi().await.expect("stream");
        Wire { connection, send, receive }
    }

    async fn send(&mut self, frame: Frame) {
        move_wire::write(&mut self.send, &frame, &[]).await.expect("sent");
    }

    async fn next(&mut self) -> Frame {
        tokio::time::timeout(WAIT, move_wire::read(&mut self.receive)).await.expect("a frame in time").expect("a frame").0
    }

    /// The next frame that isn't progress.
    async fn next_but_progress(&mut self) -> Frame {
        loop {
            match self.next().await {
                Frame::Progress { .. } => {}
                frame => return frame,
            }
        }
    }

    async fn hello(&mut self, code: &ReceiveCode, secret: &str) -> Frame {
        let hello = Frame::Hello {
            secret: secret.to_owned(),
            version: factoryd::update::VERSION.to_owned(),
            machine: "Old Mac".to_owned(),
            os: "macos".to_owned(),
            arch: "aarch64".to_owned(),
        };
        let _ = code;
        self.send(hello).await;
        self.next().await
    }

    /// Dials again after a session ended, until the target is free to
    /// answer (it lets the last one finish first).
    async fn again(endpoint: &Endpoint, code: &ReceiveCode) -> (Wire, Frame) {
        for _ in 0..100 {
            let mut wire = Wire::dial(endpoint, code).await;
            match wire.hello(code, &code.secret).await {
                Frame::Failed { why } if why.contains("right now") => tokio::time::sleep(Duration::from_millis(100)).await,
                answer => return (wire, answer),
            }
        }
        panic!("the target stayed busy");
    }

    /// Hangs up, and waits for the target to see it.
    async fn hang_up(self, person: &Person) {
        let before = person.said().matches("dropped").count();
        self.connection.close(0u32.into(), b"gone");
        drop(self);
        for _ in 0..200 {
            if person.said().matches("dropped").count() > before {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Pairs: hello, the digits the source works out, confirmed.
    async fn pair(&mut self, key: &Endpoint, code: &ReceiveCode, person: &Person) {
        let welcome = self.hello(code, &code.secret).await;
        assert!(matches!(welcome, Frame::Welcome { resumed: false, .. }), "{welcome:?}");
        person.answers.send(true).expect("answer");
        self.send(Frame::Confirmed {}).await;
        // The target says its person confirmed too.
        assert_eq!(self.next().await, Frame::Confirmed {});
        // The person was asked about the digits the source shows too.
        let digits = move_wire::digits(&code.endpoint, &key.id().to_string(), &code.secret);
        for _ in 0..100 {
            if !person.asked.lock().unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let asked = person.asked.lock().unwrap().last().cloned().expect("asked");
        assert_eq!(asked.digits, digits);
        assert_eq!((asked.machine.as_str(), asked.platform.as_str()), ("Old Mac", "macOS arm64"));
    }

    /// Sends a round of `entries` from `tree`, and the blocks the target
    /// asks for (all of them, or the first `only`); the target's
    /// `round_done`, and what it asked for.
    async fn round(
        &mut self,
        n: u32,
        last: bool,
        tree: &Path,
        entries: &[Entry],
        only: Option<usize>,
    ) -> (Frame, usize, Vec<(String, u32)>) {
        self.send(Frame::Round { n, last }).await;
        for entry in entries {
            self.send(Frame::Entry(entry.clone())).await;
        }
        self.send(Frame::RoundEnd { n }).await;
        let mut needs = VecDeque::new();
        loop {
            match self.next().await {
                Frame::Need { path, blocks } => needs.extend(blocks.into_iter().map(|index| (path.clone(), index))),
                Frame::NeedEnd { .. } => break,
                other => panic!("expected needs, got {other:?}"),
            }
        }
        let asked = needs.len();
        let mut sent = Vec::new();
        while let Some((path, index)) = needs.pop_front() {
            if only.is_some_and(|only| sent.len() == only) {
                return (Frame::Unknown, asked, sent);
            }
            let entry = entries.iter().find(|entry| entry.path == path).expect("listed");
            let bytes = std::fs::read(tree.join(&path)).expect("read");
            let start = (u64::from(index) * entry.block) as usize;
            let end = (start + entry.block as usize).min(bytes.len());
            let sent_path = path.clone();
            move_wire::write(&mut self.send, &Frame::Block { path, index, size: (end - start) as u32 }, &bytes[start..end])
                .await
                .expect("block");
            sent.push((sent_path, index));
        }
        self.send(Frame::BlocksEnd { n }).await;
        (self.next().await, asked, sent)
    }
}

/// A source's endpoint: one key, dialing from one place, as a source does.
async fn endpoint() -> Endpoint {
    endpoint_with(&SecretKey::generate()).await
}

/// The same, with this key: the source after a restart.
async fn endpoint_with(key: &SecretKey) -> Endpoint {
    Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Disabled)
        .clear_ip_transports()
        .bind_addr(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
        .expect("addr")
        .secret_key(key.clone())
        .bind()
        .await
        .expect("bind")
}

/// The listing of `tree`, as a source lists it.
fn listing(tree: &Path) -> Vec<Entry> {
    fn walk(dir: &Path, prefix: &str, out: &mut Vec<Entry>) {
        use std::os::unix::fs::PermissionsExt;
        let mut items: Vec<_> = std::fs::read_dir(dir).expect("read").filter_map(Result::ok).collect();
        items.sort_by_key(std::fs::DirEntry::file_name);
        for item in items {
            let name = item.file_name().to_string_lossy().into_owned();
            let path = if prefix.is_empty() { name } else { format!("{prefix}/{name}") };
            let meta = std::fs::symlink_metadata(item.path()).expect("meta");
            let mode = meta.permissions().mode() & 0o7777;
            if meta.file_type().is_symlink() {
                let target = std::fs::read_link(item.path()).expect("link").display().to_string();
                out.push(Entry { path, kind: Kind::Symlink, target, ..Entry::default() });
            } else if meta.is_dir() {
                out.push(Entry { path: path.clone(), kind: Kind::Dir, mode, ..Entry::default() });
                walk(&item.path(), &path, out);
            } else {
                let block = move_wire::block_size(meta.len());
                let hashes = move_wire::hash_file(&item.path(), block).expect("hash");
                out.push(Entry { path, kind: Kind::File, size: meta.len(), mode, mtime_ms: 0, block, hashes, target: String::new() });
            }
        }
    }
    let mut out = Vec::new();
    walk(tree, "", &mut out);
    out
}

/// A bundle of a real source, unpacked: the tree a source sends.
async fn tree() -> (crate::move_import::Source, PathBuf, serde_json::Value) {
    let source = source().await;
    let (path, passphrase) = bundle(&source, factoryd::update::VERSION);
    let tree = source.dir.join("tree");
    factoryd::move_bundle::unpack(&path, &passphrase, &tree).expect("unpacked");
    let manifest = serde_json::from_str(&std::fs::read_to_string(tree.join("manifest.json")).expect("manifest")).expect("json");
    (source, tree, manifest)
}

#[tokio::test]
async fn a_claktory_moves_over_the_network_resuming_after_a_drop_and_starts_only_once_committed() {
    let (source, tree, manifest) = tree().await;
    let target = temp_dir();
    let (here, user) = (Home::new(target.join("home")), target.join("user"));
    let (task, mut person) = receive(&here);
    let code = person.code().await;
    assert_eq!(ReceiveCode::parse(&code.text()).as_ref(), Some(&code));
    let why = factoryd::move_import::unfinished(&here).expect("it doesn't start while receiving");
    assert!(why.starts_with("A move into this Claktory (") && why.contains("run factoryd receive again"), "{why}");

    let secret_key = SecretKey::generate();
    let key = endpoint_with(&secret_key).await;
    let mut wire = Wire::dial(&key, &code).await;
    wire.pair(&key, &code, &person).await;

    // The plan, checked here.
    let map = vec![(source.user.display().to_string(), user.display().to_string())];
    wire.send(Frame::Plan { manifest: manifest.clone(), map, tools: vec!["sh".to_owned(), "no-such-tool-here".to_owned()] }).await;
    let Frame::Checked { problems, map, missing, .. } = wire.next().await else { panic!("checked") };
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(missing, vec!["no-such-tool-here".to_owned()]);
    assert!(map.iter().any(|(from, to)| *from == source.user.display().to_string() && *to == user.display().to_string()), "{map:?}");

    // A first round, cut off halfway.
    let entries = listing(&tree);
    let (_, asked, sent) = wire.round(1, false, &tree, &entries, Some(3)).await;
    assert!(asked > 3, "{asked}");
    // Once those three blocks are written there, the connection drops.
    let staged = here.root().join("import/bundle");
    for _ in 0..500 {
        let arrived = sent.iter().all(|(path, index)| {
            let entry = entries.iter().find(|entry| entry.path == *path).expect("listed");
            let bytes = std::fs::read(staged.join(path)).unwrap_or_default();
            let start = (u64::from(*index) * entry.block) as usize;
            let end = (start + entry.block as usize).min(entry.size as usize);
            bytes.len() >= end && move_wire::block_hash(&bytes[start..end]) == entry.hashes[*index as usize]
        });
        if arrived {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    wire.hang_up(&person).await;

    // Back, with the same key: no digits again, and only what's missing.
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
    assert_eq!(person.asked.lock().unwrap().len(), 1, "asked once");
    let (done, again, _) = wire.round(2, false, &tree, &entries, None).await;
    assert!(again <= asked - 3, "the blocks that came stayed: {again} of {asked}");
    let Frame::RoundDone { bytes_have, bytes_total, .. } = done else { panic!("{done:?}") };
    assert_eq!(bytes_have, bytes_total);

    // The source dies without a word (killed, and started again at once by
    // its supervisor, with its key): it's let in again straight away, in
    // place of the session it left, not turned away as another computer.
    wire.send(Frame::Round { n: 3, last: false }).await;
    drop(wire);
    drop(key);
    let key = endpoint_with(&secret_key).await;
    let mut wire = Wire::dial(&key, &code).await;
    let welcome = wire.hello(&code, &code.secret).await;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");

    // Then the source changes a little: the final round sends that only,
    // and what's gone from it goes here too.
    let notes = tree.join("home/agents/writer/notes.txt");
    std::fs::create_dir_all(notes.parent().unwrap()).expect("dir");
    std::fs::write(&notes, "new\n").expect("write");
    let gone =
        entries.iter().find(|entry| entry.kind == Kind::File && entry.path.starts_with("folders/0/untracked")).expect("an untracked file");
    std::fs::remove_file(tree.join(&gone.path)).expect("remove");
    let gone = gone.path.trim_start_matches("folders/0/untracked/").to_owned();
    let entries = listing(&tree);
    let (_, last, _) = wire.round(4, true, &tree, &entries, None).await;
    assert_eq!(last, 1, "only the new file");
    assert!(
        here.root().join("import/bundle/home/agents/writer/notes.txt").exists() || here.root().join("agents/writer/notes.txt").exists()
    );

    // Imported and verified, it says so; it still doesn't start.
    let Frame::Ready { old_app_code } = wire.next_but_progress().await else { panic!("ready") };
    assert!(old_app_code.is_some_and(|code| code.contains(&format!("t={}", source.token))));
    assert!(factoryd::move_import::unfinished(&here).is_some(), "not until the source commits");

    // The source commits: done, and it's this computer's Claktory now.
    let export_id = manifest["export_id"].as_str().unwrap().to_owned();
    wire.send(Frame::Commit { at_ms: 1, export_id: export_id.clone() }).await;
    // It dies again before it hears done: back, it commits again, and
    // hears it then.
    drop(wire);
    drop(key);
    let key = endpoint_with(&secret_key).await;
    let mut wire = Wire::dial(&key, &code).await;
    let welcome = wire.hello(&code, &code.secret).await;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
    wire.send(Frame::Commit { at_ms: 1, export_id }).await;
    // (A move still waiting says ready again first, as after any resume.)
    let mut answer = wire.next_but_progress().await;
    if matches!(answer, Frame::Ready { .. }) {
        answer = wire.next_but_progress().await;
    }
    assert_eq!(answer, Frame::Done {});
    wire.connection.close(0u32.into(), b"done");
    let text = tokio::time::timeout(WAIT, task).await.expect("in time").expect("joined").expect("moved");
    assert!(text.contains("Imported the Claktory from Old Mac"), "{text}");
    assert!(!text.contains("Run this Claktory in one place only"), "a move leaves one: {text}");
    assert!(factoryd::move_import::unfinished(&here).is_none() && !here.root().join("import").exists());
    assert_eq!(std::fs::read_to_string(here.root().join("agents/writer/notes.txt")).expect("notes"), "new\n");
    let site = user.join("site");
    assert_eq!(run(&site, &["branch", "--show-current"]), "main");
    assert!(!site.join(&gone).exists(), "what the source no longer had didn't come");
    let (addr, _factory) = start(&target).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.projects[0].path, site.display().to_string());
    assert!(person.said().contains("Confirmed: Old Mac is moving its Claktory here."), "{}", person.said());
}

#[tokio::test]
async fn receive_lets_in_one_source_by_its_digits_and_never_writes_outside_what_it_receives() {
    let target = temp_dir();
    let here = Home::new(target.join("home"));
    let (task, mut person) = receive(&here);
    let code = person.code().await;

    // A wrong secret.
    let key = endpoint().await;
    let mut wire = Wire::dial(&key, &code).await;
    let Frame::Failed { why } = wire.hello(&code, &"00".repeat(32)).await else { panic!("refused") };
    assert!(why.contains("isn't the code"), "{why}");
    wire.connection.close(0u32.into(), b"gone");

    // The digits turned down here: a new code, and the old one is done.
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { .. }), "{welcome:?}");
    person.answers.send(false).expect("no");
    let Frame::Failed { why } = wire.next().await else { panic!("declined") };
    assert!(why.contains("digits weren't confirmed"), "{why}");
    wire.connection.close(0u32.into(), b"gone");
    let code_now = person.code().await;
    assert_ne!(code_now.secret, code.secret);
    assert_eq!(code_now.endpoint, code.endpoint);
    let (wire, answer) = Wire::again(&key, &code).await;
    let Frame::Failed { why } = answer else { panic!("the old code no longer works") };
    assert!(why.contains("isn't the code"), "{why}");
    wire.connection.close(0u32.into(), b"gone");
    let code = code_now;

    // While one source pairs, another is turned away without digits.
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { .. }), "{welcome:?}");
    let other = endpoint().await;
    let mut second = Wire::dial(&other, &code).await;
    second
        .send(Frame::Hello {
            secret: code.secret.clone(),
            version: String::new(),
            machine: "Pest".to_owned(),
            os: String::new(),
            arch: String::new(),
        })
        .await;
    let Frame::Failed { why } = second.next().await else { panic!("turned away") };
    assert!(why.contains("Another computer is moving its Claktory here right now"), "{why}");
    person.answers.send(true).expect("yes");
    wire.send(Frame::Confirmed {}).await;
    for _ in 0..100 {
        if person.said().contains("Confirmed: Old Mac") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    second.connection.close(0u32.into(), b"gone");
    wire.hang_up(&person).await;

    // Once claimed, no other key gets in, even with the code.
    let (second, answer) = Wire::again(&other, &code).await;
    let Frame::Failed { why } = answer else { panic!("refused") };
    assert!(why.contains("already moving here"), "{why}");
    second.connection.close(0u32.into(), b"gone");

    // Hostile listings: nothing is written outside the staging folder.
    let outside = target.join("outside");
    std::fs::create_dir_all(&outside).expect("outside");
    let staging = here.root().join("import/bundle");
    let file = |path: &str, bytes: &[u8]| Entry {
        path: path.to_owned(),
        kind: Kind::File,
        size: bytes.len() as u64,
        mode: 0o644,
        block: move_wire::MIN_BLOCK,
        hashes: if bytes.is_empty() { Vec::new() } else { vec![move_wire::block_hash(bytes)] },
        ..Entry::default()
    };
    let link =
        |path: &str, target: &str| Entry { path: path.to_owned(), kind: Kind::Symlink, target: target.to_owned(), ..Entry::default() };
    let dir = |path: &str| Entry { path: path.to_owned(), kind: Kind::Dir, mode: 0o755, ..Entry::default() };
    let hostile: Vec<(Vec<Entry>, &str)> = vec![
        // A link that climbs out.
        (vec![link("home/x", "../../outside")], "leads out"),
        // A link to a folder in the tree, then a file through it.
        (vec![dir("home"), dir("folders"), link("home/l", "../folders"), file("home/l/evil", b"x")], "through a link"),
        // A path that isn't allowed.
        (vec![file("home/../evil", b"x")], "isn't allowed"),
        // Blocks that don't add up.
        (vec![Entry { hashes: Vec::new(), ..file("home/f", b"abc") }], "don't add up"),
    ];
    for (entries, said) in hostile {
        let (mut wire, welcome) = Wire::again(&key, &code).await;
        assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
        wire.send(Frame::Round { n: 1, last: false }).await;
        for entry in entries {
            wire.send(Frame::Entry(entry)).await;
        }
        let Frame::Failed { why } = wire.next().await else { panic!("refused: {said}") };
        assert!(why.contains(said), "{said}: {why}");
        wire.connection.close(0u32.into(), b"gone");
    }
    // A file swapped for a link before its block came: the block goes
    // nowhere; then a folder swapped for a link between rounds.
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
    wire.send(Frame::Round { n: 1, last: false }).await;
    wire.send(Frame::Entry(dir("home"))).await;
    wire.send(Frame::Entry(dir("home/d"))).await;
    wire.send(Frame::Entry(file("home/d/f", b"one"))).await;
    wire.send(Frame::Entry(file("home/a", b"two"))).await;
    wire.send(Frame::Entry(dir("folders"))).await;
    wire.send(Frame::Entry(link("home/a", "../folders/a"))).await;
    wire.send(Frame::RoundEnd { n: 1 }).await;
    let mut needs = Vec::new();
    loop {
        match wire.next().await {
            Frame::Need { path, .. } => needs.push(path),
            Frame::NeedEnd { .. } => break,
            other => panic!("{other:?}"),
        }
    }
    assert!(needs.contains(&"home/a".to_owned()));
    move_wire::write(&mut wire.send, &Frame::Block { path: "home/a".to_owned(), index: 0, size: 3 }, b"two").await.expect("block");
    wire.send(Frame::BlocksEnd { n: 1 }).await;
    assert!(matches!(wire.next().await, Frame::RoundDone { .. }));
    assert!(!staging.join("folders/a").exists(), "nothing written through the link");
    assert!(std::fs::symlink_metadata(staging.join("home/a")).expect("a").file_type().is_symlink());
    wire.send(Frame::Round { n: 2, last: false }).await;
    wire.send(Frame::Entry(link("home/d", "../folders"))).await;
    wire.send(Frame::Entry(file("home/d/f", b"one"))).await;
    let Frame::Failed { why } = wire.next().await else { panic!("refused") };
    assert!(why.contains("through a link"), "{why}");
    assert!(std::fs::read_dir(&outside).expect("outside").next().is_none(), "nothing outside");
    assert!(!staging.join("folders/f").exists());

    wire.connection.close(0u32.into(), b"gone");

    // Links that each stay inside but climb out as a chain: the final
    // round refuses them, two links or three.
    for chain in [
        vec![link("home/b", "."), link("home/a", "b/../..")],
        vec![link("home/c", "."), link("home/d2", "c"), link("home/e", "d2/c/../..")],
    ] {
        let (mut wire, welcome) = Wire::again(&key, &code).await;
        assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
        wire.send(Frame::Round { n: 3, last: true }).await;
        wire.send(Frame::Entry(dir("home"))).await;
        for entry in chain {
            wire.send(Frame::Entry(entry)).await;
        }
        wire.send(Frame::RoundEnd { n: 3 }).await;
        let need_end = wire.next().await;
        assert!(matches!(need_end, Frame::NeedEnd { .. }), "{need_end:?}");
        wire.send(Frame::BlocksEnd { n: 3 }).await;
        let Frame::Failed { why } = wire.next().await else { panic!("refused") };
        assert!(why.contains("through another link"), "{why}");
        wire.connection.close(0u32.into(), b"gone");
    }

    // The source calls it off: everything here goes.
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { resumed: true, .. }), "{welcome:?}");
    wire.send(Frame::Cancel { why: "the person changed their mind".to_owned() }).await;
    // Not moved: never a success (the installer would start an empty Claktory).
    let ended = tokio::time::timeout(WAIT, task).await.expect("in time").expect("joined").expect_err("called off");
    assert!(matches!(ended, factoryd::move_import::ImportError::CalledOff(_)), "{ended:?}");
    assert_eq!(ended.exit_code(), 3);
    let text = ended.to_string();
    assert!(text.contains("Old Mac called the move off. The person changed their mind. Nothing changed here."), "{text}");
    let (line, code) = factoryd::cli::receive_json::end(&Err(ended));
    assert_eq!((line["ok"].clone(), line["error"].clone(), code), (serde_json::json!(false), serde_json::json!("called_off"), 3));
    assert!(!here.root().join("import").exists() && factoryd::move_import::unfinished(&here).is_none());
}

/// The lines `factoryd receive --json` has written so far, once one of
/// them is `event`'s (or its `nth`, counting from 1).
async fn json_line(lines: &Mutex<Vec<serde_json::Value>>, event: &str, nth: usize) -> serde_json::Value {
    for _ in 0..1500 {
        let found = lines.lock().unwrap().iter().filter(|line| line["event"] == event).nth(nth - 1).cloned();
        if let Some(line) = found {
            return line;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("no {event} line {nth}: {:?}", lines.lock().unwrap());
}

fn keys(line: &serde_json::Value) -> Vec<&str> {
    let mut keys: Vec<&str> = line.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

/// #245: an agent drives `factoryd receive --json` by its lines, whose
/// shape stays as it is: the code, progress, the digits to ask the person
/// about (their yes or no on stdin), and the last line.
#[tokio::test]
async fn receive_json_says_each_step_in_a_line_and_takes_the_persons_answer_on_stdin() {
    use factoryd::cli::receive_json;
    use factoryd::move_import::ImportError;
    let target = temp_dir();
    let here = Home::new(target.join("home"));
    let lines = Arc::new(Mutex::new(Vec::<serde_json::Value>::new()));
    let (stdin, typed) = mpsc::unbounded_channel::<String>();
    let emit = {
        let lines = lines.clone();
        Arc::new(move |line| lines.lock().unwrap().push(line))
    };
    let ui = receive_json::ui(emit, Arc::new(tokio::sync::Mutex::new(typed)));
    let home = here.clone();
    let task = tokio::spawn(async move {
        let options = Options { committed: false, local_only: true };
        let nowhere = std::net::SocketAddr::from(([127, 0, 0, 1], 1));
        factoryd::move_import::receive::receive(&home, nowhere, &options, &ui).await
    });

    let line = json_line(&lines, "code", 1).await;
    assert_eq!(keys(&line), ["code", "event", "machine", "message", "platform", "version"], "{line}");
    let code = ReceiveCode::parse(line["code"].as_str().expect("the code")).expect("a code that parses");
    assert_eq!(line["platform"], format!("{}-{}", code.os, code.arch));
    assert!(line["message"].as_str().is_some_and(|message| message.contains("Move to another computer")), "{line}");

    // The source shows its digits: the person is asked, through the agent.
    let key = endpoint().await;
    let (mut wire, welcome) = Wire::again(&key, &code).await;
    assert!(matches!(welcome, Frame::Welcome { .. }), "{welcome:?}");
    let asked = json_line(&lines, "confirm", 1).await;
    assert_eq!(keys(&asked), ["digits", "event", "machine", "message", "platform"], "{asked}");
    let digits = move_wire::digits(&code.endpoint, &key.id().to_string(), &code.secret);
    assert_eq!(
        (asked["digits"].as_str(), asked["machine"].as_str(), asked["platform"].as_str()),
        (Some(digits.as_str()), Some("Old Mac"), Some("macOS arm64"))
    );
    // Anything but yes or no asks again; no turns the source down, and a new code follows.
    stdin.send("maybe".to_owned()).expect("typed");
    assert_eq!(json_line(&lines, "confirm", 2).await, asked);
    stdin.send("no".to_owned()).expect("typed");
    let Frame::Failed { why } = wire.next().await else { panic!("declined") };
    assert!(why.contains("digits weren't confirmed"), "{why}");
    let again = json_line(&lines, "code", 2).await;
    assert_ne!(again["code"], line["code"]);
    assert!(lines.lock().unwrap().iter().all(|line| ["code", "confirm", "progress"].contains(&line["event"].as_str().unwrap_or_default())));
    if let Some(progress) = lines.lock().unwrap().iter().find(|line| line["event"] == "progress") {
        assert_eq!(keys(progress), ["event", "message"]);
    }
    wire.connection.close(0u32.into(), b"gone");
    task.abort();

    // The last line, by how it ended, and its exit code.
    let (done, exit) = receive_json::end(&Ok("Moved.".to_owned()));
    assert_eq!((done, exit), (serde_json::json!({"ok": true, "event": "done", "message": "Moved."}), 0));
    let (refused, exit) = receive_json::end(&Err(ImportError::Refused("Not here.".to_owned())));
    assert_eq!((refused, exit), (serde_json::json!({"ok": false, "error": "refused", "message": "Not here."}), 3));
    let (failed, exit) = receive_json::end(&Err(ImportError::Failed("Broke.".to_owned())));
    assert_eq!((failed, exit), (serde_json::json!({"ok": false, "error": "failed", "message": "Broke."}), 1));
    assert_eq!(keys(&receive_json::stopped()), ["error", "message", "ok"]);
}
