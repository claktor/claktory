//! A stand-in factoryd for the host, and the test app (tests/guest) built
//! for wasm32-wasip2.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use claktory_apps::{Deadlines, frames};
use factory_protocol::apps::{DataAnswer, DataQuery, DataSource, DataSourceList, Done, Frame, Load, PushView, SecretValue, frame::Body};
use tokio::io::{DuplexStream, ReadHalf, WriteHalf};

/// The test app's component (world app-with-views), built once per run.
pub fn guest() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| build("guest", &[]))
}

/// The same app built for 1.0 (wit-1.0, as released), without views.
pub fn guest_1_0() -> &'static Path {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT.get_or_init(|| build("guest-1.0", &["--features", "v1_0"]))
}

fn build(target: &str, features: &[&str]) -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join(target);
    let status = std::process::Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["build", "--quiet", "--release", "--target", "wasm32-wasip2", "--target-dir"])
        .arg(&target)
        .args(features)
        .arg("--manifest-path")
        .arg(crate_dir.join("tests/guest/Cargo.toml"))
        .status()
        .expect("cargo runs");
    assert!(status.success(), "the test app builds for wasm32-wasip2 (rustup target add wasm32-wasip2)");
    target.join("wasm32-wasip2/release/claktory_apps_test_guest.wasm")
}

/// A fresh folder for one test, removed when the test's thread ends;
/// the first one a process makes sweeps those of test processes no longer
/// running (#369). CLAKTORY_TEST_KEEP=1 keeps them.
pub fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("claktory-apps-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    if std::env::var_os("CLAKTORY_TEST_KEEP").is_none() {
        static SWEPT: std::sync::Once = std::sync::Once::new();
        SWEPT.call_once(sweep);
        let _ = MADE.try_with(|made| made.borrow_mut().0.push(dir.clone()));
    }
    dir
}

/// The folders a thread made, removed when it ends.
struct Made(Vec<PathBuf>);

impl Drop for Made {
    fn drop(&mut self) {
        for dir in &self.0 {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

thread_local! {
    static MADE: std::cell::RefCell<Made> = const { std::cell::RefCell::new(Made(Vec::new())) };
}

/// Removes claktory-apps-NAME-PID folders whose PID isn't a test binary
/// running now (cargo runs them from target/…/deps/); nothing when `ps`
/// can't say.
fn sweep() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    let me = std::process::id();
    let found: Vec<(u32, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let pid: u32 = name.strip_prefix("claktory-apps-")?.rsplit_once('-')?.1.parse().ok()?;
            (pid != me).then(|| (pid, entry.path()))
        })
        .collect();
    if found.is_empty() {
        return;
    }
    let mut pids: Vec<String> = found.iter().map(|(pid, _)| pid.to_string()).collect();
    pids.sort();
    pids.dedup();
    let Ok(output) = std::process::Command::new("ps").args(["-o", "pid=,command=", "-p", &pids.join(",")]).output() else { return };
    if !output.status.success() && !output.stdout.is_empty() {
        return;
    }
    let listed = String::from_utf8_lossy(&output.stdout);
    let tests: Vec<u32> = listed
        .lines()
        .filter_map(|line| line.trim_start().split_once(char::is_whitespace))
        .filter(|(_, command)| command.contains("/deps/"))
        .filter_map(|(pid, _)| pid.parse().ok())
        .collect();
    for (pid, dir) in found {
        if !tests.contains(&pid) {
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}

/// Deadlines short enough for tests.
pub fn quick() -> Deadlines {
    Deadlines { start: Duration::from_secs(10), call: Duration::from_secs(2), timer: Duration::from_secs(2), stop: Duration::from_secs(1) }
}

/// factoryd, as far as the host can tell: it asks, answers GetSecret from
/// `secrets`, PushView with `push_error` (keeping the message in
/// `pushed`), GetDataSources with `sources` and DataQuery with `answer`
/// (keeping the query in `queries`), and keeps the one-way frames it's
/// sent.
pub struct Factoryd {
    to_host: WriteHalf<DuplexStream>,
    from_host: ReadHalf<DuplexStream>,
    next_id: u64,
    pub secrets: HashMap<String, String>,
    /// What PushView is answered: empty takes it.
    pub push_error: String,
    pub pushed: Vec<PushView>,
    pub sources: Vec<DataSource>,
    /// What DataQuery is answered.
    pub answer: DataAnswer,
    pub queries: Vec<DataQuery>,
    /// One-way frames not looked at yet.
    told: VecDeque<Body>,
    host: tokio::task::JoinHandle<std::io::Result<()>>,
}

impl Factoryd {
    /// Starts a host with `deadlines` and says hello.
    pub async fn start(deadlines: Deadlines) -> Factoryd {
        let (ours, theirs) = tokio::io::duplex(1 << 20);
        let (their_read, their_write) = tokio::io::split(theirs);
        let host = tokio::spawn(claktory_apps::serve_with(their_read, their_write, deadlines));
        let (from_host, to_host) = tokio::io::split(ours);
        let mut factoryd = Factoryd {
            to_host,
            from_host,
            next_id: 1,
            secrets: HashMap::new(),
            push_error: String::new(),
            pushed: Vec::new(),
            sources: Vec::new(),
            answer: DataAnswer::default(),
            queries: Vec::new(),
            told: VecDeque::new(),
            host,
        };
        let hello = factoryd.ask(Body::Hello(factory_protocol::apps::Hello { version: "test".into() })).await;
        let Body::Ready(ready) = hello else { panic!("ready: {hello:?}") };
        assert_eq!(ready.worlds, ["claktory:app@1", "claktory:app@1.1"]);
        factoryd
    }

    /// Sends a request and waits for its answer, meanwhile answering the
    /// host's GetSecret and keeping what it tells.
    pub async fn ask(&mut self, body: Body) -> Body {
        let id = self.next_id;
        self.next_id += 1;
        frames::write(&mut self.to_host, &Frame { id, reply_to: 0, body: Some(body) }).await.expect("write");
        loop {
            if let Some(frame) = self.receive().await
                && frame.reply_to == id
            {
                return frame.body.expect("a body");
            }
        }
    }

    /// Reads a frame: an answer comes back; GetSecret is answered and a
    /// one-way frame kept, giving None.
    async fn receive(&mut self) -> Option<Frame> {
        {
            let frame = tokio::time::timeout(Duration::from_secs(30), frames::read(&mut self.from_host))
                .await
                .expect("the host answers within 30 s")
                .expect("read")
                .expect("the host is still there");
            match frame.body {
                Some(Body::GetSecret(ref get)) => {
                    let value = self.secrets.get(&get.name).cloned();
                    let answer = SecretValue { found: value.is_some(), value: value.unwrap_or_default() };
                    let reply = Frame { id: 0, reply_to: frame.id, body: Some(Body::SecretValue(answer)) };
                    frames::write(&mut self.to_host, &reply).await.expect("write");
                    None
                }
                Some(Body::PushView(ref push)) => {
                    self.pushed.push(push.clone());
                    let reply = Frame { id: 0, reply_to: frame.id, body: Some(Body::Done(Done { error: self.push_error.clone() })) };
                    frames::write(&mut self.to_host, &reply).await.expect("write");
                    None
                }
                Some(Body::GetDataSources(_)) => {
                    let reply = Frame {
                        id: 0,
                        reply_to: frame.id,
                        body: Some(Body::DataSourceList(DataSourceList { sources: self.sources.clone() })),
                    };
                    frames::write(&mut self.to_host, &reply).await.expect("write");
                    None
                }
                Some(Body::DataQuery(ref query)) => {
                    self.queries.push(query.clone());
                    let reply = Frame { id: 0, reply_to: frame.id, body: Some(Body::DataAnswer(self.answer.clone())) };
                    frames::write(&mut self.to_host, &reply).await.expect("write");
                    None
                }
                Some(body) if frame.reply_to == 0 => {
                    self.told.push_back(body);
                    None
                }
                _ => Some(frame),
            }
        }
    }

    /// The first one-way frame `pick` takes, waiting for it if it hasn't
    /// come; the ones before it stay.
    pub async fn told<T>(&mut self, mut pick: impl FnMut(&Body) -> Option<T>) -> T {
        loop {
            if let Some(at) = self.told.iter().position(|body| pick(body).is_some()) {
                let body = self.told.remove(at).expect("there");
                return pick(&body).expect("picked");
            }
            if let Some(frame) = self.receive().await {
                panic!("an answer nobody asked for: {frame:?}");
            }
        }
    }

    /// Whether a one-way frame `pick` takes has come so far.
    pub fn was_told(&self, pick: impl FnMut(&Body) -> bool) -> bool {
        self.told.iter().any(pick)
    }

    /// Loads the test app as `app_id`, with its data in `dir`.
    pub async fn load(&mut self, app_id: &str, dir: &Path, change: impl FnOnce(&mut Load)) -> String {
        let mut load = Load {
            app_id: app_id.into(),
            version: "1.0.0".into(),
            component: guest().display().to_string(),
            data_dir: dir.join(app_id).join("data").display().to_string(),
            hosts: Vec::new(),
            memory_mb: 0,
            storage_mb: 0,
            upgrade_from: String::new(),
            world: "claktory:app@1.1".into(),
        };
        change(&mut load);
        error(self.ask(Body::Load(load)).await)
    }

    pub async fn start_app(&mut self, app_id: &str) -> String {
        error(self.ask(Body::Start(factory_protocol::apps::Start { app_id: app_id.into() })).await)
    }

    pub async fn stop_app(&mut self, app_id: &str) -> String {
        error(self.ask(Body::Stop(factory_protocol::apps::Stop { app_id: app_id.into() })).await)
    }

    /// Calls a tool as the agent "builder".
    pub async fn call(&mut self, app_id: &str, tool: &str, args: &str) -> Body {
        let call = factory_protocol::apps::Call { app_id: app_id.into(), tool: tool.into(), args: args.into(), caller: "builder".into() };
        self.ask(Body::Call(call)).await
    }

    /// A call's text, which must have worked.
    pub async fn text(&mut self, app_id: &str, tool: &str, args: &str) -> String {
        match self.call(app_id, tool, args).await {
            Body::CallResult(result) if !result.is_error => result.text,
            other => panic!("{tool} {args}: {other:?}"),
        }
    }

    /// Asks the app about a view: Done's error, "" when it worked.
    pub async fn view(&mut self, body: Body) -> String {
        error(self.ask(body).await)
    }

    /// Ends factoryd's side; the host stops its apps and returns.
    pub async fn close(self) -> std::io::Result<()> {
        drop(self.to_host);
        drop(self.from_host);
        self.host.await.expect("the host's task")
    }
}

/// A Done's error ("" when it worked).
pub fn error(body: Body) -> String {
    match body {
        Body::Done(done) => done.error,
        other => panic!("Done: {other:?}"),
    }
}
