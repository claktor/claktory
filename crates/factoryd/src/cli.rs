//! `factoryd`'s commands for people and agents setting Claktory up from a
//! shell (`status`, `account …`, `settings set`, `pair --no-wait`, `allow`,
//! `deny`, `unpair`, `devices`, `provider key --stdin`, `login`), as
//! onboarding.md specifies. Each talks to the running backend through its
//! loopback port with this machine's own token (`<home>/remote/local-pairing`),
//! so the backend stays the one that decides and remembers (tenet 7).
//!
//! With `--json` each prints one JSON line on stdout, `{"ok":true,…}` or
//! `{"ok":false,"error":"<code>","message":"…"}`, and exits with a code per
//! outcome (`Exit`); without it, a sentence for a person. Secrets never come
//! in arguments: an API key comes on stdin (`--stdin`).
//!
//! `pair`, `allow`, `deny`, `unpair` and `devices` without `--json` (or
//! `--no-wait`) keep working as before, on their own, without a backend.

use std::io::{BufRead, IsTerminal, Read};
use std::net::SocketAddr;
use std::process::ExitCode;
use std::time::Duration;

use factory_protocol::PROTOCOL_VERSION;
use factory_protocol::v1::{self, ClientMessage, ServerMessage, client_message, server_message::Body};
use futures_util::{SinkExt, StreamExt};
use prost::Message as _;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as Frame;

use crate::home::{Home, Instance};

/// How a command ended, as its exit code says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// Done.
    Ok = 0,
    /// Done as far as it can go: the next step is the person's (reading an
    /// emailed code, signing in in a browser, opening a pairing code).
    NeedsHuman = 2,
    /// Refused: wrong arguments, a wrong code, something that doesn't exist.
    Refused = 3,
    /// No backend answered on this machine.
    Unreachable = 4,
}

/// What a command answers: its JSON line, the sentence a person reads, and
/// its exit code.
#[derive(Debug)]
pub struct Answer {
    pub json: Value,
    pub text: String,
    pub exit: Exit,
}

impl Answer {
    fn ok(mut json: Value, text: impl Into<String>) -> Answer {
        json["ok"] = Value::Bool(true);
        Answer { json, text: text.into(), exit: Exit::Ok }
    }

    fn needs_human(mut answer: Answer) -> Answer {
        answer.exit = Exit::NeedsHuman;
        answer
    }

    fn fail(exit: Exit, error: &str, message: impl Into<String>) -> Answer {
        let message = message.into();
        Answer { json: json!({"ok": false, "error": error, "message": message}), text: message, exit }
    }

    fn refused(error: &str, message: impl Into<String>) -> Answer {
        Answer::fail(Exit::Refused, error, message)
    }
}

/// The commands this module answers, and how to use them.
pub const USAGE: &str = "usage: factoryd [--dev] COMMAND [--json]
Each prints one JSON line with --json; exit 0 done, 2 the next step is the person's, 3 refused, 4 Claktory isn't running.
  status                                   what this Claktory is and has
  account sign-in --email EMAIL [--service URL] | account verify --code CODE
  account status | account resend | account sign-out
  settings set KEY VALUE                   user.name, name, color, default_model, default_effort
  pair [NAME] [--no-wait]                  a code to add a device with; without --no-wait or --json, waits for it and asks here;
                                           without NAME, the device that uses it gives its own name
  devices | allow DEVICE --number DIGITS | deny DEVICE | unpair DEVICE
                                           --number: the 4 digits the device's screen shows
  provider key PROVIDER --stdin [--label LABEL]   an API key, read from stdin
  login [PROVIDER]                         sign in in a browser; with --json, paste the code or address on stdin
  export [--final] [--out FILE] [--skip FOLDER]   this Claktory in one encrypted file, to move it to another computer;
                                           --final stops it here for good, keeping everything (see docs/MOVE.md)
  export --restore | export --delete-moved after --final: brings this copy back, or deletes it once the moved one works
  move CODE [--skip FOLDER]                 moves this Claktory to the computer that shows CODE (factoryd receive there)
  import BUNDLE [--map FROM=TO]… [--passphrase-stdin]
                                           make this Claktory from one moved from another computer; run again
                                           to finish one that stopped, or import --abandon to undo it
  receive [--committed]                    show a code to move another computer's Claktory here over the network:
                                           paste it into Claktory there (Move to another computer…); run again to carry on;
                                           --json: a line per event, and yes or no to the digits on stdin
  supervise                                runs the backend and keeps it running";

/// How to use `command`, or every command: USAGE's lines for it.
fn usage(command: Option<&str>) -> String {
    let mut lines = USAGE.lines();
    let header = format!("{}\n{}", lines.next().unwrap_or_default(), lines.next().unwrap_or_default());
    // A command's lines, with the indented ones that carry them on.
    let mut named: Vec<&str> = Vec::new();
    let mut taking = false;
    for line in lines {
        if line.starts_with("          ") {
            if taking {
                named.push(line);
            }
            continue;
        }
        taking = command.is_some_and(|command| line.split_whitespace().any(|word| word == command));
        if taking {
            named.push(line);
        }
    }
    if named.is_empty() { USAGE.to_owned() } else { format!("{header}\n{}", named.join("\n")) }
}

/// What these commands call themselves in their Hello (client_version),
/// so the backend tells them from an app (`remote::Asker`).
pub(crate) const CLIENT_NAME: &str = "factoryd-cli";

/// Runs `args` (after `factoryd`, `--dev` included) if it's one of these
/// commands; None for anything else, which the caller handles as before.
pub fn dispatch(args: &[String]) -> Option<ExitCode> {
    let mut flags = Flags::parse(args)?;
    flags.interactive = std::io::stdin().is_terminal();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("factoryd: {error}");
            return Some(ExitCode::FAILURE);
        }
    };
    let instance = match instance(flags.dev) {
        Some(instance) => instance,
        None => return Some(print(&Answer::refused("no_home", "Can't find your home folder: HOME isn't set."), flags.json)),
    };
    let stdin = flags.stdin;
    let answer = runtime.block_on(run(&instance, &flags, || {
        let mut text = String::new();
        if stdin {
            let _ = std::io::stdin().read_to_string(&mut text);
        }
        text
    }));
    Some(print(&answer, flags.json))
}

fn print(answer: &Answer, json: bool) -> ExitCode {
    if json {
        println!("{}", answer.json);
    } else if answer.exit == Exit::Ok || answer.exit == Exit::NeedsHuman {
        println!("{}", answer.text);
    } else {
        eprintln!("{}", answer.text);
    }
    ExitCode::from(answer.exit as u8)
}

/// The Claktory these commands talk to, as the backend itself finds it.
fn instance(dev: bool) -> Option<Instance> {
    let mut instance = crate::home::instance(dev, &std::env::home_dir()?);
    if let Some(root) = std::env::var_os("FACTORY_HOME") {
        instance.home = Home::new(root.into());
    }
    if let Some(port) = std::env::var("FACTORY_PORT").ok().and_then(|port| port.parse().ok()) {
        instance.addr.set_port(port);
    }
    Some(instance)
}

/// A command and its flags.
#[derive(Debug, Default)]
pub struct Flags {
    pub dev: bool,
    pub json: bool,
    pub no_wait: bool,
    pub stdin: bool,
    pub email: Option<String>,
    pub code: Option<String>,
    pub service: Option<String>,
    pub label: Option<String>,
    /// `allow DEVICE --number 0427`: allow it only if it shows these digits.
    pub number: Option<String>,
    /// The command's words: ["account", "sign-in"], ["pair", "Matt's", "iPhone"].
    pub words: Vec<String>,
    /// A flag this module doesn't know, or one without its value.
    pub bad: Option<String>,
    /// --help or -h: say how to use the command, and do nothing.
    pub help: bool,
    /// Run by a person at a terminal (stdin is one): only then does `allow`
    /// let a device in on a Claktory that has devices already.
    pub interactive: bool,
    /// What the person typed when `allow` asked for the device's digits:
    /// tests stand in for the terminal with it; None reads the terminal.
    pub typed: Option<String>,
}

impl Flags {
    /// The command in `args`, if it's one of these; None for the backend's
    /// own (serve, supervise, …) and for the device commands without
    /// `--json` or `--no-wait`, which work as before.
    pub fn parse(args: &[String]) -> Option<Flags> {
        let mut flags = Flags::default();
        let mut args = args.iter();
        while let Some(arg) = args.next() {
            let mut value = |name: &str| match args.next() {
                Some(value) => Some(value.clone()),
                None => {
                    flags.bad.get_or_insert_with(|| format!("{name} needs a value"));
                    None
                }
            };
            match arg.as_str() {
                "--dev" => flags.dev = true,
                "--json" => flags.json = true,
                "--no-wait" => flags.no_wait = true,
                "--stdin" => flags.stdin = true,
                "--email" => flags.email = value("--email"),
                "--code" => flags.code = value("--code"),
                "--service" => flags.service = value("--service"),
                "--label" => flags.label = value("--label"),
                "--number" => flags.number = value("--number"),
                "--help" | "-h" => flags.help = true,
                // `pair NAME --host ADDRESS`, from before the private tunnel: ignored.
                "--host" => drop(value("--host")),
                // Never a name: `pair --help` once made a device called "--help".
                flag if flag.starts_with('-') => {
                    flags.bad.get_or_insert_with(|| format!("{flag} isn't an option"));
                }
                word => flags.words.push(word.to_owned()),
            }
        }
        // main's own, with options of its own.
        if flags.words.first().is_some_and(|word| word == "import" || word == "receive") {
            return None;
        }
        if flags.help || flags.bad.is_some() {
            return Some(flags);
        }
        let command = flags.words.first().map(String::as_str)?;
        let ours = match command {
            "status" | "account" | "settings" | "provider" | "help" => true,
            "pair" | "deny" | "unpair" | "devices" => flags.json || flags.no_wait,
            // The backend checks the number the person typed (#352 P4), and
            // refuses a device that has one without it.
            "allow" => true,
            "login" => flags.json,
            _ => false,
        };
        ours.then_some(flags)
    }
}

/// Runs the command in `flags` against the backend of `instance`;
/// `stdin` reads what was piped in, when the command takes it.
pub async fn run(instance: &Instance, flags: &Flags, stdin: impl FnOnce() -> String) -> Answer {
    if flags.help || flags.words.first().is_some_and(|word| word == "help") {
        let usage = usage(flags.words.first().map(String::as_str).filter(|word| *word != "help"));
        return Answer::ok(json!({"usage": usage}), usage);
    }
    if let Some(bad) = &flags.bad {
        let usage = usage(flags.words.first().map(String::as_str));
        return Answer::refused("usage", format!("{bad}.\n{usage}"));
    }
    let words: Vec<&str> = flags.words.iter().map(String::as_str).collect();
    // Check the command before connecting, so a typo isn't "unreachable".
    let known = matches!(
        words.as_slice(),
        ["status"]
            | ["account", "sign-in" | "verify" | "status" | "resend" | "sign-out"]
            | ["settings", "set", _, _]
            | ["pair", ..]
            | ["allow" | "deny" | "unpair", _, ..]
            | ["devices"]
            | ["provider", "key", _]
            | ["login", _]
    );
    if !known {
        return Answer::refused("usage", USAGE);
    }
    let mut client = match Client::connect(&instance.home, instance.addr).await {
        Ok(client) => client,
        Err(answer) => return answer,
    };
    match words.as_slice() {
        ["status"] => Answer::ok(status(&client.workspace, &instance.home), status_text(&client.workspace, &instance.home)),
        ["account", "sign-in"] => account_sign_in(&mut client, flags).await,
        ["account", "verify"] => account_verify(&mut client, flags).await,
        ["account", "status"] => {
            let account = account(&client.workspace);
            let text = format!("Account: {}.", account["state"].as_str().unwrap_or_default().replace('_', " "));
            Answer::ok(json!({"account": account}), text)
        }
        ["account", "resend"] => match client.request(client_message::Body::AccountResend(v1::AccountResend {})).await {
            Ok(_) => {
                let account = account(&client.workspace);
                let text = format!("Sent a new code to {}.", account["email"].as_str().unwrap_or_default());
                Answer::needs_human(Answer::ok(account, text))
            }
            Err(answer) => answer,
        },
        ["account", "sign-out"] => match client.request(client_message::Body::AccountSignOut(v1::AccountSignOut {})).await {
            Ok(_) => Answer::ok(json!({"state": "signed_out"}), "Signed out of your Claktory account."),
            Err(answer) => answer,
        },
        ["settings", "set", key, value] => settings_set(&mut client, key, value).await,
        ["pair", name @ ..] => pair(&mut client, &name.join(" ")).await,
        ["devices"] => {
            let devices = devices(&client.workspace, &instance.home);
            let text = devices_text(&devices);
            Answer::ok(json!({"devices": devices}), text)
        }
        [verb @ ("allow" | "deny" | "unpair"), device @ ..] => decide_device(&mut client, verb, &device.join(" "), flags).await,
        ["provider", "key", provider] => provider_key(&mut client, provider, flags, &stdin()).await,
        ["login", provider] => login(&mut client, provider, flags.json).await,
        _ => Answer::refused("usage", USAGE),
    }
}

/// A connection to the backend on this machine's loopback port.
struct Client {
    socket: tokio_tungstenite::WebSocketStream<TcpStream>,
    next: u64,
    /// The Workspace as the backend last sent it.
    workspace: v1::Workspace,
    has_workspace: bool,
}

/// How long to wait for the backend to answer a request.
const ANSWER_WAIT: Duration = Duration::from_secs(30);

impl Client {
    async fn connect(home: &Home, addr: SocketAddr) -> Result<Client, Answer> {
        let unreachable = |why: String| {
            Answer::fail(
                Exit::Unreachable,
                "unreachable",
                format!("Claktory isn't running on this computer ({why}). Start it, then try again."),
            )
        };
        let stream = TcpStream::connect(addr).await.map_err(|error| unreachable(format!("nothing answers on {addr}: {error}")))?;
        let (socket, _) = tokio_tungstenite::client_async(format!("ws://{addr}"), stream)
            .await
            .map_err(|error| unreachable(format!("{addr} isn't Claktory: {error}")))?;
        let mut client = Client { socket, next: 1, workspace: v1::Workspace::default(), has_workspace: false };
        let token = local_token(home);
        let hello = v1::Hello {
            protocol_version: PROTOCOL_VERSION,
            client_version: format!("{CLIENT_NAME} {}", env!("CARGO_PKG_VERSION")),
            token: token.clone().unwrap_or_default(),
            ..Default::default()
        };
        let welcome = client.request(client_message::Body::Hello(hello)).await;
        match welcome {
            Ok(Body::Welcome(_)) => {}
            Ok(other) => return Err(unreachable(format!("it answered {other:?}"))),
            Err(answer) if answer.json["error"] == "unauthorized" => return Err(not_yours(home, token.is_some())),
            Err(answer) => return Err(answer),
        }
        // The Workspace comes right after Welcome.
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !client.has_workspace {
            if client.receive(deadline).await?.is_none() {
                return Err(unreachable("it sent no workspace".to_owned()));
            }
        }
        Ok(client)
    }

    /// Sends a request and waits for its answer, keeping the Workspace
    /// current meanwhile; an Error answer becomes the command's.
    async fn request(&mut self, body: client_message::Body) -> Result<Body, Answer> {
        let id = self.next;
        self.next += 1;
        let message = ClientMessage { id, body: Some(body) };
        if let Err(error) = self.socket.send(Frame::Binary(message.encode_to_vec().into())).await {
            return Err(Answer::fail(Exit::Unreachable, "unreachable", format!("Lost Claktory: {error}")));
        }
        let deadline = tokio::time::Instant::now() + ANSWER_WAIT;
        loop {
            let Some(message) = self.receive(deadline).await? else {
                return Err(Answer::fail(Exit::Unreachable, "unreachable", "Claktory didn't answer in time."));
            };
            if message.reply_to != id {
                continue;
            }
            return match message.body {
                Some(Body::Error(error)) => Err(error_answer(&error)),
                Some(body) => Ok(body),
                None => Err(Answer::fail(Exit::Unreachable, "unreachable", "Claktory answered nothing.")),
            };
        }
    }

    /// The next message by `deadline`, after applying it if it's a
    /// Workspace; None at the deadline.
    async fn receive(&mut self, deadline: tokio::time::Instant) -> Result<Option<ServerMessage>, Answer> {
        loop {
            let frame = match tokio::time::timeout_at(deadline, self.socket.next()).await {
                Err(_) => return Ok(None),
                Ok(None) => return Err(Answer::fail(Exit::Unreachable, "unreachable", "Claktory closed the connection.")),
                Ok(Some(Err(error))) => return Err(Answer::fail(Exit::Unreachable, "unreachable", format!("Lost Claktory: {error}"))),
                Ok(Some(Ok(frame))) => frame,
            };
            let Frame::Binary(bytes) = frame else { continue };
            let Ok(message) = ServerMessage::decode(bytes) else { continue };
            if let Some(Body::Workspace(workspace)) = &message.body {
                self.workspace = workspace.clone();
                self.has_workspace = true;
            }
            return Ok(Some(message));
        }
    }

    /// Waits up to `wait` for the Workspace to say what `done` looks for.
    async fn until<T>(&mut self, wait: Duration, mut done: impl FnMut(&v1::Workspace) -> Option<T>) -> Option<T> {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            if let Some(found) = done(&self.workspace) {
                return Some(found);
            }
            match self.receive(deadline).await {
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return None,
            }
        }
    }
}

/// The token of this machine's own pairing code: what lets a program on this
/// machine in on the loopback port.
fn local_token(home: &Home) -> Option<String> {
    let code = std::fs::read_to_string(home.root().join("remote/local-pairing")).ok()?;
    code.trim().split(['?', '&']).find_map(|part| part.strip_prefix("t=")).map(str::to_owned)
}

/// The backend on the port turned this user's key away: it's another user's
/// Claktory (their key is in their own data folder, which only they can
/// read), or this user's hasn't written its key yet.
fn not_yours(home: &Home, had_key: bool) -> Answer {
    let path = home.root().join("remote/local-pairing");
    let message = if had_key {
        "This Claktory belongs to another user on this computer: it didn't accept your key. Run the command as the user who runs Claktory."
            .to_owned()
    } else {
        format!(
            "This Claktory belongs to another user on this computer, or it hasn't finished starting: there's no key for it in {}. Run the command as the user who runs Claktory, or try again in a moment.",
            path.display()
        )
    };
    Answer::refused("not_yours", message)
}

/// The backend's error as the command's: its code in words, and its message.
fn error_answer(error: &v1::Error) -> Answer {
    use v1::ErrorCode;
    let (exit, code) = match error.code() {
        ErrorCode::NotFound => (Exit::Refused, "not_found"),
        ErrorCode::InvalidRequest => (Exit::Refused, "invalid"),
        ErrorCode::WrongProof => (Exit::Refused, "wrong_code"),
        ErrorCode::RateLimited => (Exit::Refused, "rate_limited"),
        ErrorCode::Unreachable => (Exit::Unreachable, "service_unreachable"),
        ErrorCode::Unauthorized => (Exit::Refused, "unauthorized"),
        ErrorCode::VaultLocked => (Exit::NeedsHuman, "vault_locked"),
        ErrorCode::UnsupportedProtocolVersion => (Exit::Refused, "version_mismatch"),
        _ => (Exit::Refused, "failed"),
    };
    Answer::fail(exit, code, error.message.clone())
}

// ---- status ----

fn status(workspace: &v1::Workspace, home: &Home) -> Value {
    let settings = workspace.settings.clone().unwrap_or_default();
    let remote = workspace.remote.clone().unwrap_or_default();
    let person = workspace.person.clone().unwrap_or_default();
    let providers: Vec<Value> = workspace
        .providers
        .iter()
        .map(|provider| {
            let accounts: Vec<Value> = provider
                .accounts
                .iter()
                .map(|account| {
                    json!({"id": account.id, "label": account.label, "plan": account.plan, "method": account.method_id, "in_use": account.in_use})
                })
                .collect();
            json!({"id": provider.id, "name": provider.name, "signed_in": !provider.signed_in_with.is_empty(), "accounts": accounts})
        })
        .collect();
    let rivet = workspace
        .agents
        .iter()
        .find(|agent| agent.id == "rivet")
        .map_or(Value::Null, |rivet| json!({"model": rivet.model, "ready": rivet.model_available}));
    json!({
        "version": workspace.version,
        "platform": format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH),
        "name": if settings.name.is_empty() { remote.machine.clone() } else { settings.name.clone() },
        "endpoint_id": remote.endpoint_id,
        "account": account(workspace),
        "person": {"name": person.name, "initials": person.initials},
        "providers": providers,
        "devices": devices(workspace, home),
        "rivet": rivet,
    })
}

fn status_text(workspace: &v1::Workspace, home: &Home) -> String {
    let status = status(workspace, home);
    let signed_in: Vec<&str> =
        workspace.providers.iter().filter(|provider| !provider.signed_in_with.is_empty()).map(|provider| provider.name.as_str()).collect();
    let models =
        if signed_in.is_empty() { "no model provider signed in".to_owned() } else { format!("signed in to {}", signed_in.join(", ")) };
    // Only devices that are in count as paired: one whose code is unused,
    // or expired, isn't a device yet (Abbacchio's fresh Linux install).
    let count =
        |state: &str| status["devices"].as_array().map_or(0, |devices| devices.iter().filter(|device| device["state"] == state).count());
    let (devices, waiting) = (count("paired"), count("waiting"));
    let waiting = if waiting == 0 { String::new() } else { format!(", {waiting} waiting to be allowed") };
    format!(
        "Claktory {} ({}), {}: account {}, {models}, {devices} device{}{waiting}.",
        status["name"].as_str().unwrap_or_default(),
        status["version"].as_str().unwrap_or_default(),
        status["platform"].as_str().unwrap_or_default(),
        status["account"]["state"].as_str().unwrap_or_default().replace('_', " "),
        if devices == 1 { "" } else { "s" }
    )
}

// ---- account ----

fn account(workspace: &v1::Workspace) -> Value {
    use v1::ClaktoryAccountState as State;
    let account = workspace.account.clone().unwrap_or_default();
    let state = match account.state() {
        State::SignedIn => "signed_in",
        State::AwaitingCode => "awaiting_code",
        State::Revoked => "revoked",
        // Not now on the first step: signing in works as from signed out.
        State::Skipped => "skipped",
        // SignedOut, and states from a newer protocol.
        _ => "signed_out",
    };
    let mut value = json!({"state": state, "service": account.service});
    match account.state() {
        State::AwaitingCode => {
            value["sent_to"] = json!(mask(&account.email));
            if !account.code_from.is_empty() {
                value["sent_from"] = json!(account.code_from);
            }
            let left = (account.code_expires_ms - crate::transcript::now_ms()).max(0) / 1000;
            value["expires_in"] = json!(left);
        }
        State::SignedIn | State::Revoked => value["email"] = json!(account.email),
        _ => {}
    }
    if !account.problem.is_empty() {
        value["problem"] = json!(account.problem);
    }
    value
}

/// An email as a code's answer shows it: "m…@example.com".
fn mask(email: &str) -> String {
    match email.split_once('@') {
        Some((user, domain)) => format!("{}…@{domain}", user.chars().next().map(String::from).unwrap_or_default()),
        None => email.to_owned(),
    }
}

async fn account_sign_in(client: &mut Client, flags: &Flags) -> Answer {
    let Some(email) = flags.email.as_deref() else {
        return Answer::refused("usage", "Give the email: factoryd account sign-in --email you@example.com");
    };
    if let Some(service) = &flags.service {
        let set = v1::SetAccountService { url: service.clone() };
        if let Err(answer) = client.request(client_message::Body::SetAccountService(set)).await {
            return answer;
        }
    }
    if let Err(answer) = client.request(client_message::Body::AccountSignIn(v1::AccountSignIn { email: email.to_owned() })).await {
        return answer;
    }
    let account = account(&client.workspace);
    let text = format!(
        "Emailed a sign-in code to {}. Run factoryd account verify --code CODE with it.",
        account["sent_to"].as_str().unwrap_or(email)
    );
    Answer::needs_human(Answer::ok(account, text))
}

async fn account_verify(client: &mut Client, flags: &Flags) -> Answer {
    let Some(code) = flags.code.as_deref() else {
        return Answer::refused("usage", "Give the emailed code: factoryd account verify --code CODE");
    };
    if let Err(answer) = client.request(client_message::Body::AccountVerify(v1::AccountVerify { code: code.trim().to_owned() })).await {
        return answer;
    }
    let account = account(&client.workspace);
    let text = format!("Signed in to your Claktory account as {}.", account["email"].as_str().unwrap_or_default());
    Answer::ok(account, text)
}

// ---- settings ----

/// The settings `settings set` changes, through the same checks as the app.
const SETTINGS: [&str; 5] = ["user.name", "name", "color", "default_model", "default_effort"];

async fn settings_set(client: &mut Client, key: &str, value: &str) -> Answer {
    let mut update = v1::UpdateSettings::default();
    match key {
        "user.name" => update.user_name = Some(value.to_owned()),
        "name" => update.name = Some(value.to_owned()),
        "color" => update.color = Some(value.to_owned()),
        "default_model" => update.default_model = Some(value.to_owned()),
        "default_effort" => update.default_effort = Some(value.to_owned()),
        _ => return Answer::refused("unknown_setting", format!("No setting {key}; these can be set: {}.", SETTINGS.join(", "))),
    }
    match client.request(client_message::Body::UpdateSettings(update)).await {
        Ok(_) => Answer::ok(json!({"key": key, "value": value}), format!("Set {key} to \"{value}\".")),
        Err(answer) => answer,
    }
}

// ---- devices ----

/// The devices, as `devices --json` lists them. A waiting one has its
/// digits and the key it asks with (`endpoint_id`, from this machine's
/// devices file: a setup on the device compares it with its own).
fn devices(workspace: &v1::Workspace, home: &Home) -> Vec<Value> {
    let remote = workspace.remote.clone().unwrap_or_default();
    let now = crate::transcript::now_ms();
    let keys: std::collections::HashMap<String, String> = crate::remote::devices(home)
        .unwrap_or_default()
        .into_iter()
        .filter(|device| !device.pending_endpoint_id.is_empty())
        .map(|device| (device.id, device.pending_endpoint_id))
        .collect();
    remote
        .devices
        .iter()
        .map(|device| {
            let pending = remote.pending_devices.iter().find(|pending| pending.device_id == device.id);
            let mut value = json!({
                "id": device.id,
                "name": crate::remote::shown_name(&device.name),
                "connected": device.connected,
                "last_seen_ms": device.last_seen_ms,
            });
            let state = match pending {
                // Never its number (#352 P4, Fugo): the person reads it off
                // the device's own screen and types it.
                Some(pending) => {
                    value["endpoint_id"] = json!(keys.get(&device.id).cloned().unwrap_or_default());
                    value["expires_in"] = json!((pending.expires_ms - now).max(0) / 1000);
                    "waiting"
                }
                None if device.code_expires_ms > now => {
                    value["expires_in"] = json!((device.code_expires_ms - now) / 1000);
                    "code_unused"
                }
                None if device.code_expires_ms > 0 => "code_expired",
                None => "paired",
            };
            value["state"] = json!(state);
            value
        })
        .collect()
}

/// What the CLI says of a device waiting to be allowed, `name` as
/// `remote::shown_name` shows it: never its number (#352 P4, Fugo), which
/// the person reads off the device's own screen.
pub fn waiting_sentence(id: &str, name: &str) -> String {
    format!(
        "A device calling itself \"{name}\" is waiting. Allow it only if its screen shows the number you type: in a terminal on this computer, run factoryd allow {id} and type the 4 digits on its screen. Over SSH, connect with ssh -t, or it isn't a terminal."
    )
}

/// The line the person types at this computer's terminal after `prompt`;
/// None without a terminal.
fn ask_terminal(prompt: &str) -> Option<String> {
    use std::io::Write;
    let mut tty = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").ok()?;
    tty.write_all(prompt.as_bytes()).ok()?;
    tty.flush().ok()?;
    let mut line = String::new();
    std::io::BufReader::new(tty).read_line(&mut line).ok()?;
    Some(line.trim().to_owned())
}

fn devices_text(devices: &[Value]) -> String {
    if devices.is_empty() {
        return "No devices yet.".to_owned();
    }
    let lines: Vec<String> = devices
        .iter()
        .map(|device| {
            let id = device["id"].as_str().unwrap_or_default();
            let what = match device["state"].as_str().unwrap_or_default() {
                "waiting" => return format!("{id}  {}", waiting_sentence(id, device["name"].as_str().unwrap_or_default())),
                "code_unused" => "its code hasn't been used yet".to_owned(),
                "code_expired" => "its code expired".to_owned(),
                _ if device["connected"].as_bool() == Some(true) => "connected".to_owned(),
                _ => "allowed".to_owned(),
            };
            format!("{id}  {}  {what}", device["name"].as_str().unwrap_or_default())
        })
        .collect();
    lines.join("\n")
}

async fn pair(client: &mut Client, name: &str) -> Answer {
    let request = v1::CreatePairing { name: name.trim().to_owned(), ..Default::default() };
    match client.request(client_message::Body::CreatePairing(request)).await {
        Ok(Body::PairingCode(code)) => {
            let expires_in = crate::remote::CODE_LIFETIME.as_secs();
            let text = format!(
                "Open this code in Claktory on the device ({} minutes, once):\n\n{}\n\nThen allow it: factoryd allow {}",
                expires_in / 60,
                code.code,
                code.device_id
            );
            // The state as `devices --json` says it, until the device uses the code.
            let json = json!({"code": code.code, "device_id": code.device_id, "expires_in": expires_in, "state": "code_unused"});
            Answer::ok(json, text)
        }
        Ok(other) => Answer::refused("failed", format!("Claktory answered {other:?}")),
        Err(answer) => answer,
    }
}

/// A device by its id, or else its name (any case); an answer saying why
/// not when it's neither, or the name is two devices'.
fn find_device(workspace: &v1::Workspace, wanted: &str) -> Result<v1::PairedDevice, Answer> {
    let remote = workspace.remote.clone().unwrap_or_default();
    if let Some(device) = remote.devices.iter().find(|device| device.id == wanted) {
        return Ok(device.clone());
    }
    let named: Vec<&v1::PairedDevice> = remote.devices.iter().filter(|device| device.name.eq_ignore_ascii_case(wanted)).collect();
    match named.as_slice() {
        [device] => Ok((*device).clone()),
        [] => Err(Answer::refused("not_found", format!("No device is called {wanted}. factoryd devices lists them."))),
        _ => {
            let ids: Vec<&str> = named.iter().map(|device| device.id.as_str()).collect();
            Err(Answer::refused("ambiguous", format!("More than one device is called {wanted}: use its id, {}.", ids.join(" or "))))
        }
    }
}

/// Why `allow` run by a program, not a person at a terminal, lets no device
/// in once the Claktory has one: from then on, a device that's in allows
/// new ones (Fugo's rule, #245).
pub const HAS_DEVICES: &str = "This Claktory already has devices, so only a person can allow a new one: in Settings → Devices on one of them, or in a terminal on this computer with factoryd allow. Over SSH, connect with ssh -t, or it isn't a terminal.";

/// Whether a device other than this machine's own app is allowed in.
fn has_devices(workspace: &v1::Workspace) -> bool {
    workspace.remote.as_ref().is_some_and(|remote| remote.devices.iter().any(|device| device.code_expires_ms == 0))
}

/// The App Store review box's one exception to `HAS_DEVICES` (#365 R3,
/// Fugo's ruling): there a program (review-pair, as the `review` user) may
/// allow a device proven through the Claktory's own signed-in account, so a
/// reviewer can add an iPad after their iPhone. Only while root has put
/// `MARKER` there, only for a CLI that isn't root (root could put it there
/// itself), only on Linux, and still with the device's four digits, which
/// the backend checks. Everywhere else, and for code-paired devices, the
/// rule holds as it is.
mod review_box {
    // Off the Linux box only the tests use the check.
    #![cfg_attr(not(target_os = "linux"), allow(dead_code))]
    use std::path::Path;

    /// Root's switch, on the review box alone. Never from the environment,
    /// the settings or an argument.
    pub(super) const MARKER: &str = "/etc/claktory/review-allow-account";
    /// The folders above it, each of which must be root's alone too.
    pub(super) const FOLDERS: [&str; 2] = ["/etc/claktory", "/etc"];

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) enum Kind {
        File,
        Folder,
        /// A link (never followed), or anything else.
        Other,
    }

    /// What lstat says of a path.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub(super) struct Stat {
        pub kind: Kind,
        pub uid: u32,
        pub mode: u32,
    }

    /// lstat: a link is a link, not what it points to. None when it isn't there.
    pub(super) fn stat(path: &Path) -> Option<Stat> {
        use std::os::unix::fs::MetadataExt;
        let meta = std::fs::symlink_metadata(path).ok()?;
        let kind = match meta.file_type() {
            kind if kind.is_file() => Kind::File,
            kind if kind.is_dir() => Kind::Folder,
            _ => Kind::Other,
        };
        Some(Stat { kind, uid: meta.uid(), mode: meta.mode() })
    }

    /// Root's, and writable by no one else.
    fn roots(stat: Option<Stat>, kind: Kind) -> bool {
        stat.is_some_and(|stat| stat.kind == kind && stat.uid == 0 && stat.mode & 0o022 == 0)
    }

    /// Whether a program may allow this waiting device although the
    /// Claktory has devices: it proved it's the Claktory's own account's
    /// (`signed_in`, from the backend), the marker and every folder above it
    /// are root's alone, and this CLI isn't root.
    pub(super) fn lets_in(signed_in: bool, marker: Option<Stat>, folders: &[Option<Stat>], euid: u32) -> bool {
        signed_in && euid != 0 && roots(marker, Kind::File) && !folders.is_empty() && folders.iter().all(|folder| roots(*folder, Kind::Folder))
    }

    /// `lets_in` on this machine.
    #[cfg(target_os = "linux")]
    pub(super) fn here(signed_in: bool) -> bool {
        let folders = FOLDERS.map(|folder| stat(Path::new(folder)));
        // SAFETY: geteuid has no preconditions and can't fail.
        let euid = unsafe { libc::geteuid() };
        lets_in(signed_in, stat(Path::new(MARKER)), &folders, euid)
    }

    /// Off: the exception is the Linux review box's alone.
    #[cfg(not(target_os = "linux"))]
    pub(super) fn here(_signed_in: bool) -> bool {
        false
    }

    /// One line in the system log (and on stderr) for each device it let in.
    pub(super) fn log(id: &str) {
        let line = format!("allowed {id} under the review box's account exception");
        eprintln!("factoryd: {line}");
        #[cfg(target_os = "linux")]
        if let Ok(line) = std::ffi::CString::new(line) {
            // Named, so the journal says "factoryd[pid]:" as for the
            // backend's own lines: musl's syslog has no name of its own.
            // SAFETY: a static identifier, then a constant format with one C
            // string argument.
            unsafe {
                libc::openlog(c"factoryd".as_ptr(), libc::LOG_PID, libc::LOG_AUTH);
                libc::syslog(libc::LOG_AUTH | libc::LOG_NOTICE, c"%s".as_ptr(), line.as_ptr());
                libc::closelog();
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        const ROOTS_FILE: Stat = Stat { kind: Kind::File, uid: 0, mode: 0o100644 };
        const ROOTS_FOLDER: Stat = Stat { kind: Kind::Folder, uid: 0, mode: 0o040755 };
        const USER: u32 = 1000;

        fn folders() -> [Option<Stat>; 2] {
            [Some(ROOTS_FOLDER), Some(ROOTS_FOLDER)]
        }

        #[test]
        fn only_roots_marker_for_an_account_proven_device_and_a_cli_that_isnt_root() {
            // Every condition holds: on.
            assert!(lets_in(true, Some(ROOTS_FILE), &folders(), USER));
            // No marker.
            assert!(!lets_in(true, None, &folders(), USER));
            // A code-paired device, never.
            assert!(!lets_in(false, Some(ROOTS_FILE), &folders(), USER));
            // A root CLI proves nothing.
            assert!(!lets_in(true, Some(ROOTS_FILE), &folders(), 0));
            // Someone else's, a link, a folder.
            assert!(!lets_in(true, Some(Stat { uid: USER, ..ROOTS_FILE }), &folders(), USER));
            assert!(!lets_in(true, Some(Stat { kind: Kind::Other, mode: 0o120777, ..ROOTS_FILE }), &folders(), USER));
            assert!(!lets_in(true, Some(Stat { kind: Kind::Folder, ..ROOTS_FILE }), &folders(), USER));
            // Writable by its group or by anyone.
            for mode in [0o100664, 0o100646, 0o100666] {
                assert!(!lets_in(true, Some(Stat { mode, ..ROOTS_FILE }), &folders(), USER), "{mode:o}");
            }
            // A folder above it that isn't root's alone, or isn't there.
            for bad in [
                Some(Stat { mode: 0o040775, ..ROOTS_FOLDER }),
                Some(Stat { mode: 0o041777, ..ROOTS_FOLDER }),
                Some(Stat { uid: USER, ..ROOTS_FOLDER }),
                Some(Stat { kind: Kind::Other, ..ROOTS_FOLDER }),
                None,
            ] {
                assert!(!lets_in(true, Some(ROOTS_FILE), &[Some(ROOTS_FOLDER), bad], USER), "{bad:?}");
                assert!(!lets_in(true, Some(ROOTS_FILE), &[bad, Some(ROOTS_FOLDER)], USER), "{bad:?}");
            }
            assert!(!lets_in(true, Some(ROOTS_FILE), &[], USER));
        }

        /// What a test (or anyone but root) can make: a marker of its own,
        /// in a folder of its own, plain or a link to a root's file. Off.
        #[test]
        fn a_marker_anyone_but_root_can_make_is_off() {
            let dir = std::env::temp_dir().join(format!("claktory-review-box-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            let marker = dir.join("review-allow-account");
            std::fs::write(&marker, "").unwrap();
            let euid = unsafe { libc::geteuid() };
            let mine = stat(&marker).unwrap();
            assert_eq!(mine.kind, Kind::File);
            let roots_folders = folders();
            if euid != 0 {
                assert_ne!(mine.uid, 0);
                assert!(!lets_in(true, Some(mine), &roots_folders, euid));
            }
            // A link to a file of root's is a link, not that file.
            let link = dir.join("link");
            std::os::unix::fs::symlink("/etc/hosts", &link).unwrap();
            let linked = stat(&link).unwrap();
            assert_eq!(linked.kind, Kind::Other);
            assert!(!lets_in(true, Some(linked), &roots_folders, USER));
            // The folder it's in is the test's.
            assert!(!lets_in(true, Some(ROOTS_FILE), &[stat(&dir)], USER) || euid == 0);
            assert_eq!(stat(&dir.join("missing")), None);
            std::fs::remove_dir_all(&dir).unwrap();
            // And the real switch isn't there on a machine that isn't the review box.
            if stat(Path::new(MARKER)).is_none() {
                assert!(!here(true));
            }
            assert!(!here(false));
        }
    }
}

async fn decide_device(client: &mut Client, verb: &str, wanted: &str, flags: &Flags) -> Answer {
    let number = flags.number.as_deref();
    let device = match find_device(&client.workspace, wanted) {
        Ok(device) => device,
        Err(answer) => return answer,
    };
    // As the device calls itself, so without what could reach the terminal.
    let name = crate::remote::shown_name(&device.name);
    let pending =
        client.workspace.remote.as_ref().and_then(|remote| remote.pending_devices.iter().find(|pending| pending.device_id == device.id));
    let waiting = pending.is_some();
    // Only the first device may be let in by a program (a setup over SSH),
    // but on the review box (`review_box`).
    let mut review_exception = false;
    if verb == "allow" && waiting && (flags.json || flags.no_wait || !flags.interactive) && has_devices(&client.workspace) {
        if !pending.is_some_and(|pending| review_box::here(pending.signed_in)) {
            return Answer::refused("has_devices", HAS_DEVICES);
        }
        review_exception = true;
    }
    // Allowed only with the digits the person reads off the device's own
    // screen, which the backend checks (#352 P4, Fugo): never read here.
    // A person at a terminal who left them out is asked for them (Matt's
    // `factoryd pair` over SSH, Fugo's ruling 35297); a program isn't.
    let mut number = number.map(str::trim).unwrap_or_default().to_owned();
    if verb == "allow" && waiting && number.is_empty() && flags.interactive && !flags.json && !flags.no_wait {
        let prompt = format!("Type the 4 digits shown on {name}: ");
        number = match &flags.typed {
            Some(typed) => typed.trim().to_owned(),
            None => ask_terminal(&prompt).unwrap_or_default(),
        };
    }
    if verb == "allow" && waiting && number.is_empty() {
        return Answer::refused("needs_number", waiting_sentence(&device.id, &name));
    }
    let id = device.id.clone();
    let request = match verb {
        // On a terminal, a person compared the digits: the device is the
        // owner's by this Allow; from a program it shows the owner's
        // assertion first (#527 S3).
        "allow" if waiting => client_message::Body::ApproveDevice(v1::ApproveDevice {
            device_id: id.clone(),
            number,
            from_terminal: flags.interactive && std::io::stdout().is_terminal(),
        }),
        "allow" if device.code_expires_ms > crate::transcript::now_ms() => {
            return Answer::fail(
                Exit::NeedsHuman,
                "not_waiting",
                format!("{name} hasn't used its code yet: open the code on the device, then allow it."),
            );
        }
        "allow" if device.code_expires_ms == 0 => {
            return Answer::ok(json!({"allowed": id, "already": true}), format!("{name} is already allowed."));
        }
        "allow" => return Answer::refused("code_expired", format!("{name}'s code expired: make a new one with factoryd pair.")),
        "deny" if waiting => client_message::Body::DenyDevice(v1::DenyDevice { device_id: id.clone() }),
        "deny" => return Answer::refused("not_waiting", format!("{name} isn't waiting to be allowed.")),
        _ => client_message::Body::UnpairDevice(v1::UnpairDevice { device_id: id.clone() }),
    };
    match client.request(request).await {
        Ok(_) => match verb {
            "allow" if review_exception => {
                review_box::log(&id);
                Answer::ok(json!({"allowed": id, "exception": "review_box_account"}), format!("Allowed {name}: it can connect now."))
            }
            "allow" => Answer::ok(json!({"allowed": id}), format!("Allowed {name}: it can connect now.")),
            "deny" => Answer::ok(json!({"denied": id}), format!("Not allowed. {name} can't connect.")),
            _ => Answer::ok(json!({"unpaired": id}), format!("Removed {name}: it can't connect any more.")),
        },
        // A wrong number, as the backend says it (tries left, or turned away).
        Err(answer) if verb == "allow" && answer.json["error"].as_str() == Some("invalid") => {
            Answer::refused("wrong_number", answer.text.clone())
        }
        Err(answer) => answer,
    }
}

// ---- providers ----

fn find_provider<'a>(workspace: &'a v1::Workspace, wanted: &str) -> Result<&'a v1::Provider, Answer> {
    workspace.providers.iter().find(|provider| provider.id == wanted || provider.name.eq_ignore_ascii_case(wanted)).ok_or_else(|| {
        let ids: Vec<&str> = workspace.providers.iter().map(|provider| provider.id.as_str()).collect();
        Answer::refused("not_found", format!("No provider {wanted}; these are: {}.", ids.join(", ")))
    })
}

/// How long a sign-in may take to ask its question or finish.
const LOGIN_WAIT: Duration = Duration::from_secs(60);

async fn provider_key(client: &mut Client, provider: &str, flags: &Flags, piped: &str) -> Answer {
    if !flags.stdin {
        return Answer::refused("usage", "The key comes on stdin, never in an argument: factoryd provider key PROVIDER --stdin < key.txt");
    }
    let key = piped.trim().to_owned();
    if key.is_empty() {
        return Answer::refused("no_key", "Nothing came on stdin: pipe the key in.");
    }
    let provider = match find_provider(&client.workspace, provider) {
        Ok(provider) => provider.clone(),
        Err(answer) => return answer,
    };
    if !provider.methods.iter().any(|method| method.id == "api_key") {
        return Answer::refused("no_api_key", format!("{} doesn't take an API key.", provider.name));
    }
    let before: Vec<String> = provider.accounts.iter().map(|account| account.id.clone()).collect();
    let start = v1::StartLogin { provider_id: provider.id.clone(), method_id: "api_key".to_owned(), add_account: true };
    if let Err(answer) = client.request(client_message::Body::StartLogin(start)).await {
        return answer;
    }
    let asked = client
        .until(LOGIN_WAIT, |workspace| {
            let flow = workspace.login.as_ref()?;
            if !flow.error.is_empty() {
                return Some(Err(flow.error.clone()));
            }
            flow.prompt.as_ref().map(|_| Ok(()))
        })
        .await;
    match asked {
        Some(Ok(())) => {}
        Some(Err(error)) => return Answer::refused("sign_in_failed", error),
        None => return Answer::fail(Exit::Unreachable, "timeout", "The sign-in didn't ask for the key in time."),
    }
    if let Err(answer) = client.request(client_message::Body::AnswerLogin(v1::AnswerLogin { text: key })).await {
        return answer;
    }
    let provider_id = provider.id.clone();
    let done = client
        .until(LOGIN_WAIT, |workspace| match &workspace.login {
            Some(flow) if !flow.error.is_empty() => Some(Err(flow.error.clone())),
            Some(_) => None,
            // Done: the account it added, when the provider lists its accounts.
            None => Some(Ok(workspace
                .providers
                .iter()
                .find(|provider| provider.id == provider_id)
                .and_then(|provider| provider.accounts.iter().find(|account| !before.contains(&account.id)).cloned())
                .unwrap_or_default())),
        })
        .await;
    let account = match done {
        Some(Ok(account)) => account,
        Some(Err(error)) => {
            let _ = client.request(client_message::Body::CancelLogin(v1::CancelLogin {})).await;
            return Answer::refused("sign_in_failed", error);
        }
        None => return Answer::fail(Exit::Unreachable, "timeout", "The key wasn't taken in time."),
    };
    let mut label = account.label.clone();
    if let Some(wanted) = flags.label.as_ref().filter(|_| !account.id.is_empty()) {
        let update = v1::UpdateAccount {
            provider_id: provider.id.clone(),
            account_id: account.id.clone(),
            label: Some(wanted.clone()),
            retry: false,
        };
        if let Err(answer) = client.request(client_message::Body::UpdateAccount(update)).await {
            return answer;
        }
        label = wanted.clone();
    }
    Answer::ok(
        json!({"provider": provider.id, "account_id": account.id, "label": label}),
        format!("Added the API key {label} to {}.", provider.name),
    )
}

/// Signs in to a provider in a browser: prints where to go, reads the code
/// or the address the browser landed on from stdin (unless the redirect
/// reached the backend by itself), and says how it ended.
async fn login(client: &mut Client, provider: &str, json: bool) -> Answer {
    let provider = match find_provider(&client.workspace, provider) {
        Ok(provider) => provider.clone(),
        Err(answer) => return answer,
    };
    if !provider.methods.iter().any(|method| method.id == "oauth") {
        return Answer::refused("no_oauth", format!("{} has no sign-in in a browser; add an API key instead.", provider.name));
    }
    let start = v1::StartLogin { provider_id: provider.id.clone(), method_id: "oauth".to_owned(), add_account: true };
    if let Err(answer) = client.request(client_message::Body::StartLogin(start)).await {
        return answer;
    }
    let flow = client
        .until(LOGIN_WAIT, |workspace| workspace.login.as_ref().filter(|flow| !flow.url.is_empty() || !flow.error.is_empty()).cloned())
        .await;
    let Some(flow) = flow else { return Answer::fail(Exit::Unreachable, "timeout", "The sign-in didn't start in time.") };
    if !flow.error.is_empty() {
        return Answer::refused("sign_in_failed", flow.error);
    }
    let url = json!({"ok": true, "state": "open_url", "url": flow.url, "instructions": flow.instructions});
    if json {
        println!("{url}");
    } else {
        println!("Open this in a browser and sign in:\n\n{}\n\nThen paste the code or the address the browser shows here.", flow.url);
    }
    // The pasted answer, read off the async thread; the redirect may reach
    // the backend by itself meanwhile.
    let (pasted, mut answered) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        if std::io::stdin().lock().read_line(&mut line).is_ok() && !line.trim().is_empty() {
            let _ = pasted.send(line.trim().to_owned());
        }
    });
    let provider_id = provider.id.clone();
    let before: Vec<String> = provider.accounts.iter().map(|account| account.id.clone()).collect();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(15 * 60);
    loop {
        match &client.workspace.login {
            Some(flow) if !flow.error.is_empty() => return Answer::refused("sign_in_failed", flow.error.clone()),
            Some(_) => {}
            None => {
                let added = client
                    .workspace
                    .providers
                    .iter()
                    .find(|provider| provider.id == provider_id)
                    .and_then(|provider| provider.accounts.iter().find(|account| !before.contains(&account.id)).cloned());
                return match added {
                    Some(account) => Answer::ok(
                        json!({"provider": provider_id, "account_id": account.id, "label": account.label}),
                        format!("Signed in to {} as {}.", provider.name, account.label),
                    ),
                    None => Answer::refused("cancelled", "The sign-in was cancelled."),
                };
            }
        }
        tokio::select! {
            text = answered.recv(), if !answered.is_closed() => {
                if let Some(text) = text
                    && let Err(answer) = client.request(client_message::Body::AnswerLogin(v1::AnswerLogin { text })).await
                {
                    return answer;
                }
            }
            received = client.receive(deadline) => match received {
                Ok(Some(_)) => {}
                Ok(None) => return Answer::fail(Exit::NeedsHuman, "timeout", "The sign-in wasn't finished in time."),
                Err(answer) => return answer,
            },
        }
    }
}

// ---- receive --json ----

/// `factoryd receive --json` (main.rs runs it): one JSON line per event,
/// as an agent driving it reads them. `{"event":"code",…}` with the code to
/// paste on the old computer (again, with a new code, after the digits were
/// turned down), `{"event":"progress","message":…}` as it goes, and
/// `{"event":"confirm","digits":…}` when the old computer shows its digits:
/// the person compares them, and the answer, yes or no, comes on stdin, a
/// line at a time. Then [`receive_json::end`]'s last line, `{"ok":true,…}`
/// or `{"ok":false,"error":…,"message":…}`.
pub mod receive_json {
    use std::sync::Arc;

    use serde_json::{Value, json};
    use tokio::sync::{Mutex, mpsc::UnboundedReceiver};

    use crate::move_import::ImportError;
    use crate::move_import::receive::Ui;
    use crate::move_wire::ReceiveCode;

    /// The code's line: the code to paste into Claktory on the old
    /// computer (Move to another computer…), and this computer as it shows
    /// there.
    pub fn code_line(code: &ReceiveCode) -> Value {
        json!({
            "event": "code",
            "code": code.text(),
            "machine": code.machine,
            "version": code.version,
            "platform": format!("{}-{}", code.os, code.arch),
            "message": "On the computer you're moving from, open Claktory and choose Move to another computer… (Settings → General), then paste this code. Keep this running until the move is done.",
        })
    }

    /// The last line, and the exit code: 0 moved, 3 refused or called off
    /// by the other computer (nothing to fix but what it says), 1 failed.
    pub fn end(done: &Result<String, ImportError>) -> (Value, u8) {
        match done {
            Ok(text) => (json!({"ok": true, "event": "done", "message": text}), 0),
            Err(error @ ImportError::Refused(_)) => (json!({"ok": false, "error": "refused", "message": error.to_string()}), 3),
            // Not moved, nothing changed here: the other computer stopped it.
            Err(error @ ImportError::CalledOff(_)) => (json!({"ok": false, "error": "called_off", "message": error.to_string()}), 3),
            Err(error) => (json!({"ok": false, "error": "failed", "message": error.to_string()}), 1),
        }
    }

    /// The last line when it's stopped (Ctrl-C, or a TERM signal), exit 130.
    pub fn stopped() -> Value {
        json!({"ok": false, "error": "stopped", "message": "Stopped. Run factoryd receive again to carry on."})
    }

    /// Says yes or no to the digits: a line of stdin's.
    fn answer(line: &str) -> Option<bool> {
        match line.trim().to_lowercase().as_str() {
            "yes" | "y" => Some(true),
            "no" | "n" => Some(false),
            _ => None,
        }
    }

    /// The Ui that writes each event with `emit`, and reads the person's
    /// answers from `lines` (stdin's lines; those sent before it asks are
    /// dropped, and stdin closing is a no).
    pub fn ui(emit: Arc<dyn Fn(Value) + Send + Sync>, lines: Arc<Mutex<UnboundedReceiver<String>>>) -> Ui {
        let (shown, say) = (emit.clone(), emit.clone());
        Ui {
            code: Arc::new(move |code| shown(code_line(code))),
            say: Arc::new(move |text| say(json!({"event": "progress", "message": text}))),
            confirm: Arc::new(move |asked| {
                let (emit, lines) = (emit.clone(), lines.clone());
                Box::pin(async move {
                    let line = json!({
                        "event": "confirm",
                        "digits": asked.digits,
                        "machine": asked.machine,
                        "platform": asked.platform,
                        "message": format!(
                            "Ask the person whether {} shows {}. Answer yes on stdin only if they say it does, or no.",
                            asked.machine, asked.digits
                        ),
                    });
                    emit(line.clone());
                    let mut lines = lines.lock().await;
                    while lines.try_recv().is_ok() {}
                    loop {
                        match lines.recv().await {
                            Some(text) => match answer(&text) {
                                Some(yes) => return yes,
                                None => emit(line.clone()),
                            },
                            None => return false,
                        }
                    }
                })
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn only_these_commands_are_taken_and_the_old_ones_keep_working() {
        assert!(Flags::parse(&args("status")).is_some());
        assert!(Flags::parse(&args("--dev account sign-in --email a@b.c --json")).is_some_and(|flags| flags.dev && flags.json));
        // As before without --json: waits in the terminal, works offline.
        assert!(Flags::parse(&args("pair Matt's iPhone")).is_none());
        assert!(Flags::parse(&args("allow d-1")).is_some(), "the running Claktory checks the number");
        assert!(Flags::parse(&args("login anthropic")).is_none());
        assert!(Flags::parse(&args("pair iPhone --no-wait")).is_some_and(|flags| flags.words == ["pair", "iPhone"]));
        assert!(Flags::parse(&args("supervise")).is_none());
        assert!(Flags::parse(&args("")).is_none());
        let bad = Flags::parse(&args("account sign-in --email")).expect("ours");
        assert_eq!(bad.bad.as_deref(), Some("--email needs a value"));
        // Nothing starting with "-" is ever a name (Arjun's #191 test).
        let help = Flags::parse(&args("pair --help")).expect("ours");
        assert!(help.help && help.words == ["pair"]);
        assert!(Flags::parse(&args("allow -x")).is_some_and(|flags| flags.bad.is_some()));
        // --number, after the device or before it, and never in its name (Arjun's finding).
        for line in ["allow new-device-11 --number 1934", "allow --number 1934 new-device-11", "--dev allow new-device-11 --number 1934"] {
            let flags = Flags::parse(&args(line)).expect(line);
            assert_eq!(
                (flags.words, flags.number.as_deref()),
                (vec!["allow".to_owned(), "new-device-11".to_owned()], Some("1934")),
                "{line}"
            );
        }
        assert!(Flags::parse(&args("pair Phone --host 10.0.0.2")).is_none(), "as before");
        assert!(Flags::parse(&args("-h")).is_some_and(|flags| flags.help));
        assert!(Flags::parse(&args("--dev")).is_none(), "serves");
        assert!(
            Flags::parse(&args("import --abandon")).is_none()
                && Flags::parse(&args("import b --map /a=/b --passphrase-stdin")).is_none()
                && Flags::parse(&args("receive --committed")).is_none()
        );
    }

    #[test]
    fn help_is_for_the_command_asked_about() {
        let pair = usage(Some("pair"));
        assert!(pair.contains("pair [NAME]") && pair.contains("gives its own name") && !pair.contains("provider key"), "{pair}");
        assert!(pair.starts_with("usage: factoryd"), "{pair}");
        assert_eq!(usage(Some("frobnicate")), USAGE);
    }

    #[test]
    fn emails_are_masked() {
        assert_eq!(mask("matt@example.com"), "m…@example.com");
        assert_eq!(mask("nobody"), "nobody");
    }
}
