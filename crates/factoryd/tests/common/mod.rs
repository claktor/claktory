#![allow(dead_code)]

pub mod stand_in;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_agent::auth::{AuthEvent, AuthInteraction, AuthPrompt};
use factory_agent::stream::event_stream;
use factory_agent::types::ModelCost;
use factory_agent::{
    AssistantMessage, AssistantPart, Error, Message, Model, StopReason, StreamEvent, StreamFn, Usage, UserContent, UserMessage, UserPart,
};
use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, Hello, ServerMessage, client_message, server_message::Body};
use factoryd::Factory;
use factoryd::home::Home;
use factoryd::models::{API_KEY, Models, OAUTH};
use futures_util::future::BoxFuture;
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::tungstenite::Message as Frame;
use tokio_tungstenite::{WebSocketStream, client_async};

pub const MODEL: &str = "scripted/echo";
/// Another model, which works like `MODEL`.
pub const OTHER_MODEL: &str = "scripted/parrot";
/// A model of a provider whose prompt cache lifetime Claktory chooses
/// ("anthropic"), with a 200,000-token window, whose responses report
/// 160,000 tokens of context and are stamped with when they were asked for,
/// as real ones are. Listed only by `Scripted::with_long_model`.
pub const LONG_MODEL: &str = "anthropic/long";
/// How long a test waits for the server before calling it a hang. Never a
/// measure of speed: a test process starved of the processor (agents' builds
/// and tests run beside it) can stall for many seconds, and only a real hang
/// should fail.
const WAIT: Duration = Duration::from_secs(60);

/// Lets this process open as many files as the tests need: every test of
/// the crate runs in it (tests/all), each with a Factory of its own, and
/// macOS starts a process at 256.
pub fn raise_open_files() {
    static RAISED: std::sync::Once = std::sync::Once::new();
    // As factoryd raises its own at start (#490).
    RAISED.call_once(|| {
        factoryd::open_files::raise();
    });
}

/// A symbolic link at `at` to `target`. On Windows (where Developer Mode
/// lets the test account make one) a folder's or a file's, as `target` is.
pub fn symlink(target: &Path, at: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(target, at)
    }
    #[cfg(windows)]
    {
        if target.is_dir() { std::os::windows::fs::symlink_dir(target, at) } else { std::os::windows::fs::symlink_file(target, at) }
    }
}

/// `path`'s permission bits. On Windows, which has none, what they'd be for
/// what its DACL allows: 0o700 for a folder and 0o600 for a file only this
/// user and SYSTEM can open (Claktory's private DACL), 0o777 otherwise; so
/// a test that a file is 0o600 holds on both.
pub fn mode(path: &Path) -> u32 {
    #[cfg(unix)]
    {
        std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(path).expect("metadata").permissions()) & 0o777
    }
    #[cfg(windows)]
    {
        let private = !factory_agent::windows::system::open_to_others(path).expect("its security");
        match (private, path.is_dir()) {
            (true, true) => 0o700,
            (true, false) => 0o600,
            (false, _) => 0o777,
        }
    }
}

/// `path` as a command line in an agent's shell names it: as it is, or on
/// Windows with forward slashes and no `\\?\` (Git Bash reads C:/…).
pub fn shell_path(path: &Path) -> String {
    let text = path.display().to_string();
    if cfg!(windows) { text.strip_prefix(r"\\?\").unwrap_or(&text).replace('\\', "/") } else { text }
}

/// Opens a file or folder to set its times, as each system allows: Windows
/// wants write-attributes access, which `File::open` doesn't ask.
pub fn open_to_date(path: &Path) -> std::io::Result<std::fs::File> {
    #[cfg(unix)]
    return std::fs::File::open(path);
    #[cfg(windows)]
    return factoryd::windows::fs::open_to_date(path);
}

/// `text` as it goes inside a JSON string: a Windows path's backslashes
/// escaped, as the tests write tool arguments by hand.
pub fn json_text(text: impl std::fmt::Display) -> String {
    let quoted = serde_json::to_string(&text.to_string()).expect("a string");
    quoted[1..quoted.len() - 1].to_owned()
}

/// A fresh directory for one test: `$TMPDIR/factoryd-test-<pid>-<n>`.
/// Removed when the thread that made it ends, which for a test is when the
/// test does (libtest runs each in a thread of its own), and anything left
/// when the process exits; folders of test processes that are gone (killed,
/// crashed) are swept by the next one (#369: they held hundreds of GB).
/// FACTORYD_TEST_KEEP=1 keeps them all, to look at a failing test's.
pub fn temp_dir() -> PathBuf {
    raise_open_files();
    static NEXT: AtomicU64 = AtomicU64::new(0);
    // On Windows TEMP may be in 8.3 short names (C:\Users\CLAKTO~3\…),
    // which the backend spells out wherever it resolves a path.
    #[cfg(windows)]
    let base = factoryd::windows::fs::long_path(&std::env::temp_dir());
    #[cfg(unix)]
    let base = std::env::temp_dir();
    let dir = base.join(format!("{TEST_DIRS}{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    if std::env::var_os("FACTORYD_TEST_KEEP").is_none() {
        cleanup::track(&dir);
    }
    dir
}

/// Whether `ps` starts here. A sandbox that runs no set-uid program (an
/// agent's command under the browser's profile before it let /bin/ps out)
/// can't start it, and what needs it can't be tested there.
pub fn ps_runs() -> bool {
    std::process::Command::new("ps").args(["-o", "pid=", "-p", &std::process::id().to_string()]).output().is_ok()
}

/// Ends the background tasks agents started under `dir`, as its cleanup
/// does when its test ends.
pub fn end_tasks_under(dir: &Path) {
    cleanup::end_tasks(&[dir.to_owned()]);
}

/// What the test folders' names start with.
const TEST_DIRS: &str = "factoryd-test-";

mod cleanup {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, Once};

    /// Every folder this process made and hasn't removed yet: removed at
    /// exit, whatever thread made it.
    static LEFT: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());

    /// The folders a thread made, removed when it ends.
    struct Made(Vec<PathBuf>);

    impl Drop for Made {
        fn drop(&mut self) {
            for dir in self.0.drain(..) {
                remove(&dir);
            }
        }
    }

    thread_local! {
        static MADE: std::cell::RefCell<Made> = const { std::cell::RefCell::new(Made(Vec::new())) };
    }

    fn remove(dir: &Path) {
        if has_tasks(dir, 0) {
            end_tasks(&[dir.to_owned()]);
        }
        let _ = std::fs::remove_dir_all(dir);
        LEFT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|left| left != dir);
    }

    extern "C" fn at_exit() {
        let left = std::mem::take(&mut *LEFT.lock().unwrap_or_else(std::sync::PoisonError::into_inner));
        end_tasks(&left);
        for dir in left {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// `dir` goes when this thread ends (the main thread's only at exit,
    /// since it outlives every test), and at exit in any case.
    pub(super) fn track(dir: &Path) {
        static FIRST: Once = Once::new();
        FIRST.call_once(|| {
            // SAFETY: at_exit is a plain function that only removes folders.
            unsafe { libc::atexit(at_exit) };
            std::thread::spawn(sweep);
        });
        LEFT.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(dir.to_owned());
        if std::thread::current().name() != Some("main") {
            // A thread already ending has no MADE: exit removes it then.
            let _ = MADE.try_with(|made| made.borrow_mut().0.push(dir.to_owned()));
        }
    }

    /// Removes the folders of test processes no longer running; never
    /// those of one that is (other suites run beside this one).
    fn sweep() {
        let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
        let me = std::process::id();
        let mut found: Vec<(u32, PathBuf)> = Vec::new();
        for entry in entries.flatten() {
            let name = entry.file_name();
            let pid = name
                .to_str()
                .and_then(|name| name.strip_prefix(super::TEST_DIRS))
                .and_then(|rest| rest.split_once('-'))
                .and_then(|(pid, _)| pid.parse::<u32>().ok());
            if let Some(pid) = pid.filter(|pid| *pid != me) {
                found.push((pid, entry.path()));
            }
        }
        let mut pids: Vec<u32> = found.iter().map(|(pid, _)| *pid).collect();
        pids.sort_unstable();
        pids.dedup();
        let Some(tests) = test_processes(&pids) else { return };
        let gone: Vec<PathBuf> = found.into_iter().filter(|(pid, _)| !tests.contains(pid)).map(|(_, dir)| dir).collect();
        end_tasks(&gone);
        for dir in gone {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    /// Whether an agent under `dir` started background tasks (an
    /// `agents/<id>/tasks` folder, a few levels down at most).
    fn has_tasks(dir: &Path, depth: usize) -> bool {
        if dir.file_name().is_some_and(|name| name == "tasks")
            && dir.parent().and_then(Path::parent).is_some_and(|up| up.ends_with("agents"))
        {
            return true;
        }
        depth < 5
            && std::fs::read_dir(dir).is_ok_and(|entries| {
                entries.flatten().any(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()) && has_tasks(&entry.path(), depth + 1))
            })
    }

    /// Ends the process groups of background tasks started under `dirs`
    /// (Bruno: a test's bash loop outlived it by 28 hours). A task's shell
    /// leads its group and names its status file, under the test's home
    /// (factory-agent's bash `factory-task`); never this process's group.
    #[cfg(unix)]
    pub(crate) fn end_tasks(dirs: &[PathBuf]) {
        if dirs.is_empty() {
            return;
        }
        let Ok(output) = std::process::Command::new("ps").args(["-axo", "pgid=,command="]).output() else { return };
        // The folders as given and as the system names them (/private/var…).
        let mut under: Vec<String> = Vec::new();
        for dir in dirs {
            under.push(format!("{}/", dir.display()));
            if let Ok(real) = dir.canonicalize() {
                under.push(format!("{}/", real.display()));
            }
        }
        // SAFETY: getpgrp only reads this process's group.
        let own = unsafe { libc::getpgrp() };
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let Some((group, command)) = line.trim_start().split_once(char::is_whitespace) else { continue };
            if !command.contains("factory-task") || !under.iter().any(|dir| command.contains(dir.as_str())) {
                continue;
            }
            let Ok(group) = group.parse::<libc::pid_t>() else { continue };
            if group > 1 && group != own {
                // SAFETY: signals only that task's own process group.
                unsafe { libc::kill(-group, libc::SIGKILL) };
            }
        }
    }

    /// On Windows: the jobs of the background tasks started under `dirs`,
    /// as each home under them recorded them (`windows/jobs.json`), stopped
    /// as the person's Stop would (only a job that's this user's own with
    /// Claktory's DACL).
    #[cfg(windows)]
    pub(crate) fn end_tasks(dirs: &[PathBuf]) {
        fn recorded(dir: &Path, depth: usize, pids: &mut Vec<u32>) {
            if let Some(saved) = std::fs::read(dir.join("windows").join("jobs.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            {
                pids.extend(
                    saved["jobs"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|job| job["pid"].as_u64())
                        .filter_map(|pid| u32::try_from(pid).ok()),
                );
            }
            if depth < 4
                && let Ok(entries) = std::fs::read_dir(dir)
            {
                for entry in entries.flatten().filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir())) {
                    recorded(&entry.path(), depth + 1, pids);
                }
            }
        }
        let mut pids = Vec::new();
        for dir in dirs {
            recorded(dir, 0, &mut pids);
        }
        for pid in pids {
            factory_agent::windows::job::stop(pid);
        }
    }

    /// Which of `pids` are test binaries running now (cargo puts them in
    /// target/…/deps/): a pid since reused by another program doesn't keep
    /// a dead test's folders. None when `ps` can't say, to remove nothing.
    fn test_processes(pids: &[u32]) -> Option<Vec<u32>> {
        if pids.is_empty() {
            return Some(Vec::new());
        }
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        let output = std::process::Command::new("ps").args(["-o", "pid=,command=", "-p", &list.join(",")]).output().ok()?;
        // ps exits 1 when none of them runs.
        if !output.status.success() && !output.stdout.is_empty() {
            return None;
        }
        let listed = String::from_utf8_lossy(&output.stdout);
        Some(
            listed
                .lines()
                .filter_map(|line| line.trim_start().split_once(char::is_whitespace))
                .filter(|(_, command)| command.contains("/deps/"))
                .filter_map(|(pid, _)| pid.parse().ok())
                .collect(),
        )
    }
}

pub fn home(dir: &Path) -> Home {
    Home::new(dir.join("home"))
}

/// A Factory whose scripted provider is already signed in.
pub async fn start(dir: &Path) -> (SocketAddr, Arc<Factory>) {
    start_with(dir, Scripted::signed_in()).await
}

pub async fn start_with(dir: &Path, models: Scripted) -> (SocketAddr, Arc<Factory>) {
    start_with_models(dir, Arc::new(models)).await
}

/// A Factory with models of the test's own.
pub async fn start_with_models(dir: &Path, models: Arc<dyn Models>) -> (SocketAddr, Arc<Factory>) {
    raise_open_files();
    // Logs of commands with secrets go through the real `factoryd redact`.
    factoryd::set_filter_executable(env!("CARGO_BIN_EXE_factoryd").into());
    let factory = Factory::open(home(dir), models).await.expect("open factory");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("local addr");
    homes().insert(addr, home(dir));
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    (addr, factory)
}

/// The homes of the Factories tests started, by address: their clients
/// connect with their person token, as this machine's app.
fn homes() -> std::sync::MutexGuard<'static, std::collections::HashMap<SocketAddr, Home>> {
    static HOMES: std::sync::LazyLock<Mutex<std::collections::HashMap<SocketAddr, Home>>> = std::sync::LazyLock::new(Default::default);
    HOMES.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Says that the Factory at `addr` keeps its files in `home`, for one the
/// test runs as a process of its own.
pub fn serves(addr: SocketAddr, home: Home) {
    homes().insert(addr, home);
}

/// Where the Factory at `addr` keeps its files.
pub fn home_of(addr: SocketAddr) -> Home {
    homes().get(&addr).cloned().expect("a Factory the test started")
}

/// The token that lets a test's client in on the loopback port at `addr`
/// as this computer's app, the person (test builds only; the real
/// app connects over its socket).
pub fn local_token(addr: SocketAddr) -> String {
    factoryd::authority::person_token_for_test(home_of(addr).root())
}

/// The tool token agents' commands use at `addr`: `<home>/tool-token`.
pub fn tool_token(addr: SocketAddr) -> String {
    let home = home_of(addr);
    std::fs::read_to_string(home.root().join("tool-token")).expect("the tool token").trim().to_owned()
}

/// A provider with one model, which answers "echo: <the prompt>" (Claktory's
/// tags in it defused, as a kept reply's are) streamed in two pieces and reports 1200 tokens of context. Asked for a summary, it
/// writes one. A prompt ending in "hang" never gets an answer.
///
/// Sign in through the browser by pasting a redirect address with
/// `code=good`, or with the API key "sk-good".
pub struct Scripted {
    /// How it's signed in; None when it isn't.
    signed_in_with: Mutex<Option<&'static str>>,
    /// The system prompt of the latest request.
    pub system_prompt: Arc<Mutex<String>>,
    /// The tools of the latest request, as JSON.
    pub tools: Arc<Mutex<String>>,
    /// What the latest summary was asked of: the request's text, message by message.
    pub compacted: Arc<Mutex<String>>,
    /// Every request, oldest first.
    pub requests: Arc<Mutex<Vec<Request>>>,
    /// Lists `LONG_MODEL` too.
    long: bool,
    /// Its usage read is refused, as a provider refuses a sign-in it
    /// won't take any more (after a move, say).
    refuses_usage: bool,
}

/// What a model request was sent with.
#[derive(Clone, Debug)]
pub struct Request {
    pub model: String,
    /// The effort, e.g. "high"; "off" when it doesn't reason.
    pub effort: String,
    pub system_prompt: String,
    /// The tools' names, in order.
    pub tools: Vec<String>,
    /// How long the prompt cache was asked to last: "5m" or "1h".
    pub cache_ttl: String,
    /// The last message's text as the model read it, tags and all.
    pub prompt: String,
    /// The images in it.
    pub images: Vec<String>,
}

const ACCOUNT: &str = "Scripted account";
const KEY: &str = "Scripted key";

impl Scripted {
    pub fn signed_in() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(Some(KEY)),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
            long: false,
            refuses_usage: false,
        }
    }

    /// Signed in with its subscription, whose weekly limit is 40% used.
    pub fn subscribed() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(Some(ACCOUNT)),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
            long: false,
            refuses_usage: false,
        }
    }

    pub fn signed_out() -> Scripted {
        Scripted {
            signed_in_with: Mutex::new(None),
            system_prompt: Arc::default(),
            tools: Arc::default(),
            compacted: Arc::default(),
            requests: Arc::default(),
            long: false,
            refuses_usage: false,
        }
    }

    /// Lists `LONG_MODEL` too.
    pub fn with_long_model(mut self) -> Scripted {
        self.long = true;
        self
    }

    /// Its usage read is refused: the provider won't take the sign-in.
    pub fn refusing_usage(mut self) -> Scripted {
        self.refuses_usage = true;
        self
    }

    fn current(&self) -> Option<&'static str> {
        *self.signed_in_with.lock().expect("lock")
    }
}

impl Models for Scripted {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        let mut models = match self.current() {
            Some(_) => vec![
                v1::Model {
                    id: MODEL.into(),
                    name: "Echo".into(),
                    context_window: 10_000,
                    efforts: vec!["off".into()],
                    image_input: false,
                    added: false,
                    image_output: false,
                },
                v1::Model {
                    id: OTHER_MODEL.into(),
                    name: "Parrot".into(),
                    context_window: 10_000,
                    efforts: vec!["off".into()],
                    image_input: false,
                    added: false,
                    image_output: false,
                },
            ],
            None => Vec::new(),
        };
        if self.long && self.current().is_some() {
            models.push(v1::Model {
                id: LONG_MODEL.into(),
                name: "Long".into(),
                context_window: 200_000,
                efforts: vec!["off".into()],
                image_input: false,
                added: false,
                image_output: false,
            });
        }
        Box::pin(async move { models })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        let current = self.current();
        Box::pin(async move {
            vec![v1::Provider {
                id: "scripted".into(),
                name: "Scripted".into(),
                signed_in_with: current.unwrap_or_default().into(),
                can_sign_out: current.is_some(),
                methods: vec![
                    v1::LoginMethod { id: OAUTH.into(), label: ACCOUNT.into() },
                    v1::LoginMethod { id: API_KEY.into(), label: KEY.into() },
                ],
                usage: None,
                plan: String::new(),
                accounts: Vec::new(),
                cache_ttl: String::new(),
                ..Default::default()
            }]
        })
    }

    fn login<'a>(
        &'a self,
        _provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        _add: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let signed_in_with = if method == OAUTH {
                interaction
                    .notify(AuthEvent::AuthUrl { url: "https://scripted.test/authorize".into(), instructions: Some("Approve".into()) });
                let pasted = interaction.prompt(AuthPrompt::ManualCode { message: "Paste the redirect".into(), placeholder: None }).await?;
                if !pasted.contains("code=good") {
                    return Err(Error::Other("the authorization code was rejected".into()));
                }
                ACCOUNT
            } else {
                let key = interaction.prompt(AuthPrompt::Secret { message: "Enter Scripted key".into(), placeholder: None }).await?;
                if key != "sk-good" {
                    return Err(Error::Other("invalid API key".into()));
                }
                KEY
            };
            *self.signed_in_with.lock().expect("lock") = Some(signed_in_with);
            Ok(())
        })
    }

    fn logout<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        *self.signed_in_with.lock().expect("lock") = None;
        Box::pin(async { Ok(()) })
    }

    fn usage<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<Option<v1::ProviderUsage>, Error>> {
        let subscribed = self.current() == Some(ACCOUNT);
        let refused = self.refuses_usage;
        Box::pin(async move {
            if refused {
                return Err(factory_agent::auth::refusal(true, "Scripted refused this sign-in: sign in again.".into()));
            }
            Ok(subscribed.then(|| v1::ProviderUsage {
                windows: vec![
                    v1::UsageWindow { id: "weekly".into(), label: "Weekly".into(), used_percent: 40.0, resets_at: 1_795_554_000_000 },
                    v1::UsageWindow { id: "five_hour".into(), label: "5-hour".into(), used_percent: 2.5, resets_at: 0 },
                ],
                ..v1::ProviderUsage::default()
            }))
        })
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        let long = self.long && id == LONG_MODEL;
        (id == MODEL || id == OTHER_MODEL || long).then(|| Model {
            id: if long {
                "long"
            } else if id == OTHER_MODEL {
                "parrot"
            } else {
                "echo"
            }
            .into(),
            name: if long {
                "Long"
            } else if id == OTHER_MODEL {
                "Parrot"
            } else {
                "Echo"
            }
            .into(),
            api: "scripted".into(),
            provider: if long { "anthropic" } else { "scripted" }.into(),
            base_url: String::new(),
            reasoning: false,
            // Parrot reads images too.
            input: if id == OTHER_MODEL { vec!["text".into(), "image".into()] } else { vec!["text".into()] },
            cost: ModelCost::default(),
            context_window: if long { 200_000 } else { 10_000 },
            max_tokens: 1_000,
            thinking_level_map: None,
            compat: None,
            headers: None,
            extra: Default::default(),
        })
    }

    fn stream(&self) -> StreamFn {
        let system_prompt = self.system_prompt.clone();
        let tools = self.tools.clone();
        let compacted = self.compacted.clone();
        let requests = self.requests.clone();
        Arc::new(move |model, context, options| {
            requests.lock().expect("lock").push(Request {
                model: model.id.clone(),
                effort: options.reasoning.map_or("off".to_owned(), |level| format!("{level:?}").to_lowercase()),
                system_prompt: context.system_prompt.clone().unwrap_or_default(),
                tools: context.tools.iter().map(|tool| tool.name.to_string()).collect(),
                cache_ttl: options.cache_ttl.as_str().to_owned(),
                prompt: match context.messages.last() {
                    Some(Message::User(user)) => text_of(&user.content),
                    _ => String::new(),
                },
                images: match context.messages.last() {
                    Some(Message::User(UserMessage { content: UserContent::Parts(parts), .. })) => parts
                        .iter()
                        .filter_map(|part| match part {
                            UserPart::Image { mime_type, .. } => Some(mime_type.clone()),
                            UserPart::Text { .. } => None,
                        })
                        .collect(),
                    _ => Vec::new(),
                },
            });
            *system_prompt.lock().expect("lock") = context.system_prompt.clone().unwrap_or_default();
            *tools.lock().expect("lock") = serde_json::to_string(&context.tools).expect("tools");
            let prompt = match context.messages.last() {
                Some(Message::User(user)) => text_of(&user.content),
                Some(Message::ToolResult(result)) => {
                    let mut text = String::from("done: ");
                    for part in &result.content {
                        if let factory_agent::ToolResultPart::Text { text: part } = part {
                            text.push_str(part);
                        }
                    }
                    text
                }
                _ => String::new(),
            };
            let prompt = without_device(prompt);
            // Asked to compact, it summarizes like a real model would.
            let reply = if prompt.contains("<summary>") {
                let texts: Vec<String> = context
                    .messages
                    .iter()
                    .map(|message| match message {
                        Message::User(user) => text_of(&user.content),
                        _ => String::new(),
                    })
                    .collect();
                *compacted.lock().expect("lock") = texts.join("\n");
                format!("<summary>{} earlier messages.</summary>", context.messages.len() - 1)
            } else if prompt.starts_with("done: ") {
                prompt.clone()
            } else {
                // A real model never writes Claktory's own tags back: the
                // echo has them defused, as Claktory keeps a reply (G7).
                format!("echo: {}", factory_agent::text::defuse_tags(&prompt))
            };
            let half = (0..=reply.len() / 2).rev().find(|at| reply.is_char_boundary(*at)).unwrap_or(0);
            let (first, second) = reply.split_at(half);
            let mut partial = AssistantMessage {
                content: Vec::new(),
                api: model.api.clone(),
                provider: model.provider.clone(),
                model: model.id.clone(),
                usage: Usage { input: if model.provider == "anthropic" { 160_000 } else { 1_000 }, output: 200, ..Usage::default() },
                stop_reason: StopReason::Stop,
                error_message: None,
                timestamp: if model.provider == "anthropic" { factoryd::transcript::now_ms() as u64 } else { 1 },
                extra: Default::default(),
            };
            let (sink, stream) = event_stream();
            sink.push(StreamEvent::Start { partial: partial.clone() });
            // "slowcall <tool> <arguments>" calls it too, writing the
            // arguments in two pieces a moment apart, as a model streams them.
            if let Some(call) = prompt.strip_prefix("slowcall ")
                && let Some((name, raw)) = call.split_once(' ')
            {
                let (name, raw) = (name.to_owned(), raw.to_owned());
                tokio::spawn(async move {
                    let arguments = serde_json::from_str(&raw).expect("tool arguments");
                    let part =
                        |arguments| AssistantPart::ToolCall { id: "call-1".into(), name: name.clone(), arguments, thought_signature: None };
                    partial.content.push(part(serde_json::json!({})));
                    sink.push(StreamEvent::ToolcallStart { content_index: 0, partial: partial.clone() });
                    let (first, second) = raw.split_at(raw.len() / 2);
                    sink.push(StreamEvent::ToolcallDelta { content_index: 0, delta: first.into(), partial: partial.clone() });
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                    sink.push(StreamEvent::ToolcallDelta { content_index: 0, delta: second.into(), partial: partial.clone() });
                    partial.content[0] = part(arguments);
                    sink.push(StreamEvent::ToolcallEnd {
                        content_index: 0,
                        tool_call: partial.content[0].clone(),
                        partial: partial.clone(),
                    });
                    partial.stop_reason = StopReason::ToolUse;
                    sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message: partial });
                });
                return Ok(stream);
            }
            // "call <tool> <arguments>" calls the tool.
            if let Some(call) = prompt.strip_prefix("call ")
                && let Some((name, arguments)) = call.split_once(' ')
            {
                let arguments = serde_json::from_str(arguments).expect("tool arguments");
                partial.content.push(AssistantPart::ToolCall {
                    id: "call-1".into(),
                    name: name.into(),
                    arguments,
                    thought_signature: None,
                });
                partial.stop_reason = StopReason::ToolUse;
                sink.push(StreamEvent::Done { reason: StopReason::ToolUse, message: partial });
                return Ok(stream);
            }
            // "refuse my key" fails as a provider that refuses the key does.
            if prompt.ends_with("refuse my key") {
                partial.stop_reason = StopReason::Error;
                partial.error_message =
                    Some(r#"401 {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#.into());
                let refused = factory_agent::accounts::Refused { account: "1".into(), method: "api_key".into() };
                factory_agent::accounts::mark_refused(&mut partial, &refused);
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: partial });
                return Ok(stream);
            }
            // "decline" trips Echo's safety classifiers; any other model
            // answers it normally, so a fallback can.
            if prompt.ends_with("decline") && model.id == "echo" {
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some("I can't help with this.".into());
                factory_agent::refusal::mark(
                    &mut partial,
                    &serde_json::json!({"category": "cyber", "explanation": "I can't help with this."}),
                );
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: partial });
                return Ok(stream);
            }
            // "fail" fails for good, as a model that won't answer does.
            if prompt.ends_with("fail") {
                partial.stop_reason = StopReason::Error;
                partial.error_message = Some("scripted failure".into());
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: partial });
                return Ok(stream);
            }
            // "hang" never finishes, like a run a restart cuts off; so does
            // the summary of a conversation that mentions "slow summary".
            let slow_summary = prompt.contains("<summary>") && compacted.lock().expect("lock").contains("slow summary");
            if prompt.ends_with("hang") || slow_summary {
                tokio::spawn(async move {
                    let _open = sink;
                    std::future::pending::<()>().await
                });
                return Ok(stream);
            }
            partial.content.push(AssistantPart::Text { text: String::new(), text_signature: None });
            sink.push(StreamEvent::TextStart { content_index: 0, partial: partial.clone() });
            for piece in [first, second] {
                if let Some(AssistantPart::Text { text, .. }) = partial.content.first_mut() {
                    text.push_str(piece);
                }
                sink.push(StreamEvent::TextDelta { content_index: 0, delta: piece.into(), partial: partial.clone() });
            }
            sink.push(StreamEvent::TextEnd { content_index: 0, content: reply.clone(), partial: partial.clone() });
            sink.push(StreamEvent::Done { reason: StopReason::Stop, message: partial });
            Ok(stream)
        })
    }
}

/// The person's message without the tag naming the device it came from,
/// which a real model reads past.
fn without_device(text: String) -> String {
    if let Some(rest) = text.strip_prefix("<sent ")
        && let Some((_, inner)) = rest.split_once(">\n")
        && let Some(inner) = inner.strip_suffix("\n</sent>")
    {
        return inner.to_owned();
    }
    text
}

fn text_of(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => {
            let mut text = String::new();
            for part in parts {
                if let UserPart::Text { text: part } = part {
                    text.push_str(part);
                }
            }
            text
        }
    }
}

/// What a client's WebSocket runs over: the loopback port, or a stream of
/// the private tunnel.
pub trait Io: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send> Io for T {}

pub struct Client {
    socket: WebSocketStream<Box<dyn Io>>,
    next_id: u64,
    /// What must live as long as the client: its tunnel connection.
    _keep: Option<Box<dyn std::any::Any + Send>>,
}

pub struct Reply {
    /// Events the server sent before answering.
    pub events: Vec<ServerMessage>,
    pub body: Body,
}

impl Client {
    pub async fn open(addr: SocketAddr) -> Client {
        let stream = TcpStream::connect(addr).await.expect("connect");
        // Named as the app names the loopback port: Windows checks the Host.
        Client::over_as(Box::new(stream), None, &format!("ws://{addr}/")).await
    }

    /// A client over `stream`, keeping `keep` while it lives.
    pub async fn over(stream: Box<dyn Io>, keep: Option<Box<dyn std::any::Any + Send>>) -> Client {
        Client::over_as(stream, keep, "ws://factory/").await
    }

    async fn over_as(stream: Box<dyn Io>, keep: Option<Box<dyn std::any::Any + Send>>, url: &str) -> Client {
        let (socket, _) = client_async(url, stream).await.expect("websocket handshake");
        Client { socket, next_id: 1, _keep: keep }
    }

    /// Connects and completes the handshake. Returns the initial state.
    pub async fn connect(addr: SocketAddr) -> (Client, v1::Workspace, Option<v1::Transcript>) {
        Client::connect_as(addr, v1::DeviceKind::Unspecified).await
    }

    /// Connects as an app on the kind of device given.
    pub async fn connect_as(addr: SocketAddr, kind: v1::DeviceKind) -> (Client, v1::Workspace, Option<v1::Transcript>) {
        Client::connect_with(addr, kind, false).await
    }

    /// Connects as an app that applies WorkspacePatch (`patches`) or not.
    pub async fn connect_with(addr: SocketAddr, kind: v1::DeviceKind, patches: bool) -> (Client, v1::Workspace, Option<v1::Transcript>) {
        let mut client = Client::open(addr).await;
        let hello = Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: "test".into(),
            token: local_token(addr),
            device_kind: kind.into(),
            workspace_patches: patches,
            ..Default::default()
        };
        client.send(ClientMessage { id: 1, body: Some(client_message::Body::Hello(hello)) }).await;
        let (workspace, transcript) = client.welcomed().await;
        (client, workspace, transcript)
    }

    /// Reads the answer to Hello: Welcome, then the state.
    pub async fn welcomed(&mut self) -> (v1::Workspace, Option<v1::Transcript>) {
        let client = self;
        match client.receive().await.body {
            Some(Body::Welcome(_)) => {}
            other => panic!("expected welcome, got {other:?}"),
        }
        let workspace = match client.receive().await.body {
            Some(Body::Workspace(workspace)) => workspace,
            other => panic!("expected workspace, got {other:?}"),
        };
        let transcript = if workspace.selected_agent_id.is_empty() {
            None
        } else {
            match client.receive().await.body {
                Some(Body::Transcript(transcript)) => Some(transcript),
                other => panic!("expected transcript, got {other:?}"),
            }
        };
        client.next_id = 2;
        (workspace, transcript)
    }

    pub async fn send(&mut self, message: ClientMessage) {
        self.send_bytes(message.encode_to_vec()).await;
    }

    pub async fn send_bytes(&mut self, bytes: Vec<u8>) {
        self.socket.send(Frame::Binary(bytes.into())).await.expect("send");
    }

    pub async fn receive(&mut self) -> ServerMessage {
        let next = async {
            loop {
                match self.socket.next().await.expect("server closed the connection").expect("read") {
                    Frame::Binary(payload) => return ServerMessage::decode(payload).expect("decode"),
                    _ => continue,
                }
            }
        };
        tokio::time::timeout(WAIT, next).await.expect("timed out waiting for the server")
    }

    /// Sends a request and collects everything up to its answer.
    pub async fn request(&mut self, body: client_message::Body) -> Reply {
        let id = self.next_id;
        self.next_id += 1;
        self.send(ClientMessage { id, body: Some(body) }).await;
        let mut events = Vec::new();
        loop {
            let message = self.receive().await;
            if message.reply_to == id {
                return Reply { events, body: message.body.expect("reply body") };
            }
            assert_eq!(message.reply_to, 0, "reply to another request");
            events.push(message);
        }
    }

    /// Sends a request that must succeed.
    pub async fn ok(&mut self, body: client_message::Body) -> Vec<ServerMessage> {
        let reply = self.request(body).await;
        match reply.body {
            Body::Ack(_) => reply.events,
            other => panic!("expected ack, got {other:?}"),
        }
    }

    /// Reads events until `pick` finds what it's looking for.
    pub async fn wait_for<T>(&mut self, mut pick: impl FnMut(&Body) -> Option<T>) -> T {
        loop {
            let message = self.receive().await;
            if let Some(body) = &message.body
                && let Some(found) = pick(body)
            {
                return found;
            }
        }
    }

    /// Sends a WebSocket ping and waits for its pong, keeping what else
    /// comes meanwhile.
    pub async fn ping(&mut self, payload: &'static [u8]) -> Vec<ServerMessage> {
        self.socket.send(Frame::Ping(payload.into())).await.expect("ping");
        let mut kept = Vec::new();
        let pong = async {
            loop {
                match self.socket.next().await.expect("server closed the connection").expect("read") {
                    Frame::Pong(answer) if answer.as_ref() == payload => return,
                    Frame::Binary(payload) => kept.push(ServerMessage::decode(payload).expect("decode")),
                    _ => continue,
                }
            }
        };
        tokio::time::timeout(WAIT, pong).await.expect("timed out waiting for the pong");
        kept
    }

    pub async fn closed(&mut self) -> bool {
        let next = tokio::time::timeout(WAIT, self.socket.next()).await.expect("timed out waiting for close");
        matches!(next, None | Some(Err(_)) | Some(Ok(Frame::Close(_))))
    }
}

/// The last workspace among `events`.
pub fn last_workspace(events: &[ServerMessage]) -> Option<&v1::Workspace> {
    let mut found = None;
    for event in events {
        if let Some(Body::Workspace(workspace)) = &event.body {
            found = Some(workspace);
        }
    }
    found
}

/// The Workspace a patch makes of `last`, the way the protocol says an app
/// applies one: field by field, generically.
pub fn apply_patch(last: &v1::Workspace, patch: &v1::WorkspacePatch) -> v1::Workspace {
    let mut next = patch.workspace.clone().expect("a patch carries a workspace");
    assert!(!patch.kept.contains(&2), "agents are never kept whole");
    next.merge(kept_fields(&last.encode_to_vec(), &patch.kept).as_slice()).expect("kept fields");
    for agent in &mut next.agents {
        let old = || last.agents.iter().find(|old| old.id == agent.id).expect("a kept agent was there").clone();
        if patch.kept_agents.contains(&agent.id) {
            *agent = old();
        } else if let Some(fields) = patch.kept_agent_fields.iter().find(|fields| fields.id == agent.id) {
            assert!(!fields.kept.contains(&1), "an agent's id is never kept");
            agent.merge(kept_fields(&old().encode_to_vec(), &fields.kept).as_slice()).expect("kept agent fields");
        }
    }
    next
}

/// The encoded occurrences of `message`'s fields numbered in `kept`.
fn kept_fields(message: &[u8], kept: &[u32]) -> Vec<u8> {
    let mut fields = Vec::new();
    let mut at = 0;
    while at < message.len() {
        let start = at;
        let key = varint(message, &mut at);
        match key & 7 {
            0 => {
                varint(message, &mut at);
            }
            1 => at += 8,
            2 => at += varint(message, &mut at) as usize,
            5 => at += 4,
            other => panic!("wire type {other}"),
        }
        if kept.contains(&((key >> 3) as u32)) {
            fields.extend_from_slice(&message[start..at]);
        }
    }
    fields
}

fn varint(bytes: &[u8], at: &mut usize) -> u64 {
    let mut value = 0;
    for shift in (0..64).step_by(7) {
        let byte = bytes[*at];
        *at += 1;
        value |= u64::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            break;
        }
    }
    value
}

pub fn create_project(name: &str, path: &Path) -> client_message::Body {
    client_message::Body::CreateProject(v1::CreateProject { name: name.into(), path: path.display().to_string() })
}

pub fn create_agent(project_id: &str, name: &str, prompt: &str) -> client_message::Body {
    client_message::Body::CreateAgent(v1::CreateAgent {
        project_id: project_id.into(),
        name: name.into(),
        prompt: prompt.into(),
        model: MODEL.into(),
        effort: "off".into(),
        color: String::new(),
        folder_id: String::new(),
        role: String::new(),
        lead_id: String::new(),
    })
}

pub fn send_message(agent_id: &str, text: &str) -> client_message::Body {
    client_message::Body::SendMessage(v1::SendMessage { agent_id: agent_id.into(), text: text.into(), ..Default::default() })
}

/// A transcript's entries as short strings, for comparing whole conversations.
pub fn describe(entries: &[v1::Entry]) -> Vec<String> {
    use v1::entry::Kind;
    let mut out = Vec::new();
    for entry in entries {
        out.push(match &entry.kind {
            Some(Kind::User(user)) => format!("user: {}", user.text),
            Some(Kind::Assistant(assistant)) => format!("assistant: {}", assistant.text),
            Some(Kind::Thinking(thinking)) => format!("thinking: {}", thinking.text),
            Some(Kind::ToolCall(call)) => format!("tool {}: {}", call.name, call.output),
            Some(Kind::Compaction(compaction)) => format!("compaction: {}", compaction.summary),
            Some(Kind::Notice(notice)) => format!("notice: {}", notice.text),
            Some(Kind::Event(event)) if event.summary.is_empty() => format!("event from {}: {}", event.from, event.text),
            Some(Kind::Event(event)) => format!("event from {}: {}", event.from, event.summary),
            Some(Kind::Proposal(proposal)) => format!("proposal {} ({:?}): {}", proposal.id, proposal.state(), proposal.text),
            Some(Kind::SecretRequest(request)) => format!("secret request {} ({:?}): {}", request.id, request.state(), request.reason),
            Some(Kind::ConnectProposal(proposal)) => {
                format!("connect proposal {} ({:?}): {} for {}", proposal.id, proposal.state(), proposal.connector_id, proposal.purpose)
            }
            Some(Kind::DevicePairing(pairing)) => format!("device pairing {} ({:?}): {}", pairing.id, pairing.state(), pairing.name),
            Some(Kind::SecretApproval(approval)) => {
                format!("secret approval ({:?}): {} for {}", approval.state(), approval.secret_names.join(", "), approval.command)
            }
            Some(Kind::AskPerson(ask)) => format!("ask {} ({:?}): {}", ask.id, ask.state(), ask.title),
            Some(Kind::PipeStart(start)) => format!("pipe start {} ({:?}): {}", start.id, start.state(), start.name),
            Some(Kind::AppGrant(proposal)) => {
                format!("app grant {} ({:?}): {} for {}", proposal.id, proposal.state(), proposal.app_id, proposal.agent_id)
            }
            Some(Kind::AppAccess(proposal)) => format!("app access {} ({:?}): {}", proposal.id, proposal.state(), proposal.app_id),
            None => "empty".into(),
        });
    }
    out
}

/// What a client shows: the workspace, the selected agent's transcript and
/// the focused project's explorer, kept up to date from events the way the
/// app does it.
#[derive(Default)]
pub struct View {
    pub workspace: v1::Workspace,
    /// The latest entries, as the snapshot sent them and updates added to
    /// them; the first may be a later one than seq 0 (Transcript.has_earlier).
    pub entries: Vec<v1::Entry>,
    pub has_earlier: bool,
    pub deltas: usize,
    /// The explorer sent last, as an app that keeps only one sees it.
    pub explorer: Option<v1::Explorer>,
    /// Every project's explorer, as last sent.
    pub explorers: std::collections::BTreeMap<String, v1::Explorer>,
    /// The open extension panel's content, as last sent.
    pub extension_view: Option<v1::ExtensionView>,
    /// What waits for the selected agent, as last sent.
    pub queued: Option<v1::QueuedMessages>,
    /// Every highlight sent, in order.
    pub highlights: Vec<v1::Highlight>,
    /// What came for the conversation this connection watches
    /// (ServerMessage.watched, #514), in order, kept apart.
    pub watched: Vec<Body>,
}

impl View {
    /// The explorer's rows as indented names: "src/", "  main.rs", and
    /// "(hidden)" after rows .factoryignore hides.
    pub fn tree(&self) -> Vec<String> {
        let mut rows = Vec::new();
        for entry in self.explorer.as_ref().map(|explorer| explorer.entries.as_slice()).unwrap_or_default() {
            let indent = "  ".repeat(entry.depth as usize);
            let slash = if entry.is_dir { "/" } else { "" };
            let hidden = if entry.hidden { " (hidden)" } else { "" };
            rows.push(format!("{indent}{}{slash}{hidden}", entry.name));
        }
        rows
    }
}

impl View {
    pub fn apply(&mut self, body: &Body) {
        match body {
            Body::Workspace(workspace) => self.workspace = workspace.clone(),
            Body::WorkspacePatch(patch) => self.workspace = apply_patch(&self.workspace, patch),
            Body::Explorer(explorer) => {
                self.explorer = Some(explorer.clone());
                self.explorers.insert(explorer.project_id.clone(), explorer.clone());
            }
            Body::ExtensionView(view) => self.extension_view = Some(view.clone()),
            Body::QueuedMessages(queued) => self.queued = Some(queued.clone()),
            Body::Highlight(highlight) => self.highlights.push(highlight.clone()),
            Body::Transcript(transcript) => {
                self.entries = transcript.entries.clone();
                self.has_earlier = transcript.has_earlier;
            }
            Body::Entry(update) => {
                let Some(entry) = update.entry.clone() else { return };
                // An entry older than the ones held is left for when they're loaded.
                let Some(at) = (entry.seq as usize).checked_sub(self.first_seq()) else { return };
                // Before the selected agent's transcript is loaded (a run
                // already going as it's selected), its entries come with it.
                if self.entries.is_empty() && entry.seq > 0 {
                    return;
                }
                if at < self.entries.len() {
                    self.entries[at] = entry;
                } else {
                    assert_eq!(at, self.entries.len(), "entries arrive in order");
                    self.entries.push(entry);
                }
            }
            Body::Delta(delta) => {
                use v1::entry::Kind;
                self.deltas += 1;
                // As for entries: one not held yet comes whole with the
                // transcript (a run already going as it's selected).
                let Some(entry) = (delta.seq as usize).checked_sub(self.first_seq()).and_then(|at| self.entries.get_mut(at)) else {
                    return;
                };
                match entry.kind.as_mut() {
                    Some(Kind::Assistant(v1::AssistantText { text })) | Some(Kind::Thinking(v1::Thinking { text })) => {
                        text.push_str(&delta.text)
                    }
                    other => panic!("delta for {other:?}"),
                }
            }
            _ => {}
        }
    }

    /// The seq of the first entry held.
    pub fn first_seq(&self) -> usize {
        self.entries.first().map_or(0, |entry| entry.seq as usize)
    }

    pub fn agent(&self, id: &str) -> &v1::Agent {
        self.workspace.agents.iter().find(|agent| agent.id == id).expect("agent in workspace")
    }

    /// Applies events until `done` holds.
    pub async fn follow(&mut self, client: &mut Client, mut done: impl FnMut(&View) -> bool) {
        while !done(self) {
            let message = client.receive().await;
            self.take(message);
        }
    }

    /// One message: the watched conversation's kept apart (#514).
    pub fn take(&mut self, message: ServerMessage) {
        match message.body {
            Some(body) if message.watched => self.watched.push(body),
            Some(body) => self.apply(&body),
            None => {}
        }
    }
}

/// Applies a request's pre-ack events to `view` and returns the reply.
pub async fn apply_request(client: &mut Client, view: &mut View, body: client_message::Body) -> Body {
    let reply = client.request(body).await;
    for event in reply.events {
        view.take(event);
    }
    reply.body
}
