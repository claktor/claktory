//! `claktory-qa`: throwaway Claktories to test features in, headless, and a
//! driver that speaks the protocol to them. See README.md; scripts/qa.sh
//! builds it, with the factoryd and claktory-account it runs, from a commit.

mod driver;
mod model;
mod sandbox;

use std::process::ExitCode;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use driver::Connection;
use sandbox::Sandbox;

const USAGE: &str = "\
claktory-qa: throwaway Claktories for testing, and a driver for them
Every command prints one JSON line: {\"ok\":true,…} or {\"ok\":false,\"error\":…,\"message\":…}.
Exit 0 done, 1 failed, 2 usage, 3 refused (by the sandbox, or as unsafe), 4 the sandbox isn't running.

Sandboxes
  up [NAME] [--no-account] [--port PORT] [--model] [--script FILE]
                                           start one: a fresh Claktory on a free port (7500 up), with its
                                           own account service whose emails land in its mail folder; with
                                           --model (or --script), its agents run on qa-model, a scripted model
  down NAME… | --all [--keep]              end them and delete their folders (--keep keeps the folders)
  list                                     the sandboxes, and whether each is running
  restart NAME [--kill]                    end its factoryd and start it again, environment and all (--kill:
                                           kill -9 first, as in a crash); its data, account service and qa-model stay
  env NAME                                 export lines with factoryd's whole environment (eval \"$(claktory-qa env qa1)\")
  cli NAME ARGS…                           factoryd ARGS against the sandbox (status --json, account …, settings set …)

Driving one
  workspace NAME [PATH]                    the Workspace as JSON, or what PATH names in it
  send NAME REQUEST [JSON] [--then PATH]   send any request (its fields as JSON, proto names); prints the answer,
                                           and with --then, what PATH names in the Workspace after it
  wait NAME PATH [VALUE] [--secs N]        wait (30 s) until PATH names VALUE, or anything if VALUE is left out
  watch NAME [--secs N] [--path PATH]      print what the sandbox sends for N s (10): each message, and the
                                           Workspace's PATH when it changes
  ask NAME AGENT TEXT [--secs N]          message an agent as the person and wait (60 s) until it's done:
                                           the entries its run added, and its last reply
  transcript NAME AGENT [--last N] [--from SEQ]
                                           an agent's entries, oldest first: the latest (as the app opens it),
                                           the last N (1000 at most), or every one from SEQ on
  sign-in NAME [EMAIL]                     sign in to its account service (qa@example.test), code and all
  code NAME [EMAIL]                        the newest sign-in code it emailed
  requests [REQUEST]                       every request's name, or one's fields

qa-model (README: the script's rules)
  script NAME FILE                         check a script and make it the sandbox's (read for every request)
  model NAME [--agent A] [--last N] [--system]
                                           what qa-model was asked and answered: each request's last message,
                                           tools, the rule that fit and the answer (--system: the system prompt)

PATH: account.state, agents[0].name, agents[name=Rivet].status, pipes[name=Launch].state
Root: CLAKTORY_QA_ROOT, or claktory-qa in the temporary folder. Never ~/.factory or ~/.factory-dev,
never ports 7420 or 7421, never claktory.com: claktory-qa refuses them.";

/// Why a command didn't do what it was asked, and how it exits.
#[derive(Debug)]
pub struct Failure {
    exit: u8,
    code: String,
    message: String,
}

impl Failure {
    pub fn failed(message: impl Into<String>) -> Failure {
        Failure { exit: 1, code: "failed".to_owned(), message: message.into() }
    }

    pub fn usage(message: impl Into<String>) -> Failure {
        Failure { exit: 2, code: "usage".to_owned(), message: message.into() }
    }

    pub fn refused(message: impl Into<String>) -> Failure {
        Failure { exit: 3, code: "refused".to_owned(), message: message.into() }
    }

    pub fn unreachable(message: impl Into<String>) -> Failure {
        Failure { exit: 4, code: "not_running".to_owned(), message: message.into() }
    }

    /// The sandbox's own error, with its code: `invalid_request`, `not_found`…
    pub fn backend(error: &factory_protocol::v1::Error) -> Failure {
        let code = error.code().as_str_name().trim_start_matches("ERROR_CODE_").to_lowercase();
        Failure { exit: 3, code, message: error.message.clone() }
    }
}

type Outcome = Result<Value, Failure>;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => return finish(Err(Failure::failed(error.to_string()))),
    };
    finish(runtime.block_on(run(&args)))
}

fn finish(outcome: Outcome) -> ExitCode {
    match outcome {
        Ok(Value::Null) => ExitCode::SUCCESS,
        Ok(Value::Object(fields)) => {
            // `ok` first, for people reading it.
            let mut out = serde_json::Map::new();
            out.insert("ok".to_owned(), Value::Bool(true));
            out.extend(fields);
            println!("{}", Value::Object(out));
            ExitCode::SUCCESS
        }
        Ok(other) => {
            println!("{}", json!({ "ok": true, "value": other }));
            ExitCode::SUCCESS
        }
        Err(failure) => {
            println!("{}", json!({ "ok": false, "error": failure.code, "message": failure.message }));
            ExitCode::from(failure.exit)
        }
    }
}

/// `--name value` options taken out of `args`, the rest left in order.
struct Args {
    rest: Vec<String>,
    options: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], with_value: &[&str], flags: &[&str]) -> Result<Args, Failure> {
        let mut parsed = Args { rest: Vec::new(), options: Vec::new() };
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            if with_value.contains(&arg.as_str()) {
                let value = iter.next().ok_or_else(|| Failure::usage(format!("{arg} needs a value")))?;
                parsed.options.push((arg.clone(), Some(value.clone())));
            } else if flags.contains(&arg.as_str()) {
                parsed.options.push((arg.clone(), None));
            } else if arg.starts_with("--") {
                return Err(Failure::usage(format!("Unknown option {arg}.\n\n{USAGE}")));
            } else {
                parsed.rest.push(arg.clone());
            }
        }
        Ok(parsed)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.options.iter().find(|(key, _)| key == name).and_then(|(_, value)| value.as_deref())
    }

    fn flag(&self, name: &str) -> bool {
        self.options.iter().any(|(key, _)| key == name)
    }

    fn secs(&self, default: u64) -> Result<Duration, Failure> {
        match self.value("--secs") {
            None => Ok(Duration::from_secs(default)),
            Some(secs) => secs.parse().map(Duration::from_secs).map_err(|_| Failure::usage("--secs takes a number of seconds")),
        }
    }

    fn name(&self) -> Result<Sandbox, Failure> {
        let name = self.rest.first().ok_or_else(|| Failure::usage(format!("Which sandbox?\n\n{USAGE}")))?;
        sandbox::find(name)
    }
}

async fn run(args: &[String]) -> Outcome {
    let Some((command, args)) = args.split_first() else { return Err(Failure::usage(USAGE)) };
    match command.as_str() {
        "up" => {
            let args = Args::parse(args, &["--port", "--script"], &["--no-account", "--model"])?;
            let port = match args.value("--port") {
                Some(port) => Some(port.parse().map_err(|_| Failure::usage("--port takes a port number"))?),
                None => None,
            };
            let model = match args.value("--script") {
                Some(script) => Some(Some(std::path::PathBuf::from(script))),
                None => args.flag("--model").then_some(None),
            };
            let sandbox = sandbox::up(sandbox::Up { name: args.rest.first().cloned(), port, account: !args.flag("--no-account"), model })?;
            Ok(sandbox.to_json())
        }
        "down" => {
            let args = Args::parse(args, &[], &["--all", "--keep"])?;
            let sandboxes = if args.flag("--all") {
                sandbox::all()?
            } else if args.rest.is_empty() {
                return Err(Failure::usage("Which sandbox? Name it, or --all."));
            } else {
                args.rest.iter().map(|name| sandbox::find(name)).collect::<Result<Vec<_>, _>>()?
            };
            let mut ended = Vec::new();
            for sandbox in &sandboxes {
                sandbox::down(sandbox, args.flag("--keep"))?;
                ended.push(sandbox.name.clone());
            }
            Ok(json!({ "ended": ended }))
        }
        "list" => Ok(json!({ "sandboxes": sandbox::all()?.iter().map(Sandbox::to_json).collect::<Vec<_>>() })),
        "restart" => {
            let args = Args::parse(args, &[], &["--kill"])?;
            let mut sandbox = args.name()?;
            sandbox::restart(&mut sandbox, args.flag("--kill"))?;
            Ok(sandbox.to_json())
        }
        "env" => {
            let sandbox = Args::parse(args, &[], &[])?.name()?;
            print!("{}", sandbox::env_lines(&sandbox));
            Ok(Value::Null)
        }
        "cli" => {
            let Some((name, rest)) = args.split_first() else { return Err(Failure::usage("cli NAME ARGS…")) };
            let sandbox = sandbox::find(name)?;
            let code = sandbox::cli(&sandbox, rest)?;
            std::process::exit(code);
        }
        "workspace" => {
            let args = Args::parse(args, &[], &[])?;
            let sandbox = args.name()?;
            let connection = Connection::open(&sandbox).await?;
            let workspace = driver::workspace_json(&connection.workspace);
            pick(&workspace, args.rest.get(1).map(String::as_str))
        }
        "send" => {
            let args = Args::parse(args, &["--then"], &[])?;
            let sandbox = args.name()?;
            let request = args
                .rest
                .get(1)
                .ok_or_else(|| Failure::usage("send NAME REQUEST [JSON]: which request? `claktory-qa requests` lists them."))?;
            let body: Value = match args.rest.get(2) {
                Some(text) => serde_json::from_str(text).map_err(|error| Failure::usage(format!("That isn't JSON: {error}")))?,
                None => json!({}),
            };
            let mut connection = Connection::open(&sandbox).await?;
            let answer = connection.request(request, &body).await?;
            let mut out = json!({ "answer": answer });
            if let Some(path) = args.value("--then") {
                out["then"] = driver::select(&driver::workspace_json(&connection.workspace), path).cloned().unwrap_or(Value::Null);
            }
            Ok(out)
        }
        "wait" => {
            let args = Args::parse(args, &["--secs"], &[])?;
            let sandbox = args.name()?;
            let path = args.rest.get(1).ok_or_else(|| Failure::usage("wait NAME PATH [VALUE]"))?;
            let wanted = args.rest.get(2);
            let mut connection = Connection::open(&sandbox).await?;
            let deadline = Instant::now() + args.secs(30)?;
            loop {
                let workspace = driver::workspace_json(&connection.workspace);
                let found = driver::select(&workspace, path).filter(|value| !value.is_null());
                let done = match (found, wanted) {
                    (Some(value), Some(wanted)) => driver::plain(value) == *wanted,
                    (found, None) => found.is_some(),
                    (None, Some(_)) => false,
                };
                if done {
                    return Ok(json!({ "value": found.cloned().unwrap_or(Value::Null) }));
                }
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() || connection.receive(left).await?.is_none() {
                    let now = found.map_or_else(|| "nothing".to_owned(), driver::plain);
                    return Err(Failure::failed(format!("{path} still names {now} after {} s.", args.secs(30)?.as_secs())));
                }
            }
        }
        "watch" => {
            let args = Args::parse(args, &["--secs", "--path"], &[])?;
            let sandbox = args.name()?;
            let mut connection = Connection::open(&sandbox).await?;
            let path = args.value("--path");
            let mut last = path.and_then(|path| driver::select(&driver::workspace_json(&connection.workspace), path).cloned());
            if let Some(path) = path {
                println!("{}", json!({ "path": path, "value": last }));
            }
            let deadline = Instant::now() + args.secs(10)?;
            loop {
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(message) = (if left.is_zero() { None } else { connection.receive(left).await? }) else { return Ok(Value::Null) };
                if matches!(message.body, Some(factory_protocol::v1::server_message::Body::Workspace(_))) {
                    if let Some(path) = path {
                        let now = driver::select(&driver::workspace_json(&connection.workspace), path).cloned();
                        if now != last {
                            println!("{}", json!({ "path": path, "value": now }));
                            last = now;
                        }
                    }
                    continue;
                }
                use prost::Message as _;
                println!("{}", driver::body_of(driver::to_json("ServerMessage", &message.encode_to_vec())));
            }
        }
        "ask" => {
            let args = Args::parse(args, &["--secs"], &[])?;
            let sandbox = args.name()?;
            let (Some(agent), Some(text)) = (args.rest.get(1), args.rest.get(2)) else { return Err(Failure::usage("ask NAME AGENT TEXT")) };
            ask(&sandbox, agent, text, args.secs(60)?).await
        }
        "transcript" => {
            let args = Args::parse(args, &["--last", "--from"], &[])?;
            let sandbox = args.name()?;
            let agent = args.rest.get(1).ok_or_else(|| Failure::usage("transcript NAME AGENT"))?;
            let number = |name: &str| -> Result<Option<u64>, Failure> {
                args.value(name).map(|value| value.parse().map_err(|_| Failure::usage(format!("{name} takes a number")))).transpose()
            };
            transcript(&sandbox, agent, number("--last")?, number("--from")?).await
        }
        "code" => {
            let args = Args::parse(args, &[], &[])?;
            let sandbox = args.name()?;
            let (code, to, file) = sandbox::code(&sandbox, args.rest.get(1).map(String::as_str))
                .ok_or_else(|| Failure::failed(format!("No sign-in code in {} yet.", sandbox.mail().display())))?;
            Ok(json!({ "code": code, "to": to, "file": file }))
        }
        "sign-in" => {
            let args = Args::parse(args, &[], &[])?;
            let sandbox = args.name()?;
            if sandbox.account_port.is_none() {
                return Err(Failure::refused(format!("{} was started with --no-account: it has no account service.", sandbox.name)));
            }
            let email = args.rest.get(1).map_or("qa@example.test", String::as_str);
            let before = sandbox::mail_count(&sandbox);
            let mut connection = Connection::open(&sandbox).await?;
            connection.request("AccountSignIn", &json!({ "email": email })).await?;
            let deadline = Instant::now() + Duration::from_secs(20);
            let code = loop {
                if sandbox::mail_count(&sandbox) > before
                    && let Some((code, ..)) = sandbox::code(&sandbox, Some(email))
                {
                    break code;
                }
                if Instant::now() > deadline {
                    return Err(Failure::failed("The account service sent no code in 20 s."));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            };
            connection.request("AccountVerify", &json!({ "code": code })).await?;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(20);
            while connection
                .workspace
                .account
                .as_ref()
                .is_none_or(|account| account.state() != factory_protocol::v1::ClaktoryAccountState::SignedIn)
            {
                let left = deadline.saturating_duration_since(tokio::time::Instant::now());
                if left.is_zero() || connection.receive(left).await?.is_none() {
                    return Err(Failure::failed("Verified, but the sandbox didn't say it's signed in within 20 s."));
                }
            }
            pick(&driver::workspace_json(&connection.workspace), Some("account"))
        }
        "serve-model" => {
            let args = Args::parse(args, &["--port", "--script", "--log"], &[])?;
            let port = args.value("--port").and_then(|port| port.parse().ok()).ok_or_else(|| Failure::usage("serve-model --port PORT"))?;
            sandbox::check_port(port)?;
            let script = args.value("--script").ok_or_else(|| Failure::usage("serve-model --script FILE"))?;
            let log = args.value("--log").ok_or_else(|| Failure::usage("serve-model --log FILE"))?;
            model::serve(port, script.into(), log.into()).await?;
            Ok(Value::Null)
        }
        "script" => {
            let args = Args::parse(args, &[], &[])?;
            let sandbox = args.name()?;
            if sandbox.model_port.is_none() {
                return Err(Failure::refused(format!("{} has no qa-model: start a sandbox with --model.", sandbox.name)));
            }
            let file = args.rest.get(1).ok_or_else(|| Failure::usage("script NAME FILE"))?;
            let text = std::fs::read_to_string(file).map_err(|error| Failure::usage(format!("Can't read {file}: {error}")))?;
            model::check_script(&text).map_err(Failure::usage)?;
            std::fs::write(sandbox.model_script(), text).map_err(|error| Failure::failed(error.to_string()))?;
            Ok(json!({ "script": sandbox.model_script() }))
        }
        "model" => {
            let args = Args::parse(args, &["--agent", "--last"], &["--system"])?;
            let sandbox = args.name()?;
            let text = std::fs::read_to_string(sandbox.model_log()).unwrap_or_default();
            let mut requests: Vec<Value> = text
                .lines()
                .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                .filter(|request| args.value("--agent").is_none_or(|agent| request["agent"] == agent))
                .map(|mut request| {
                    if !args.flag("--system")
                        && let Some(fields) = request.as_object_mut()
                    {
                        fields.remove("system");
                    }
                    request
                })
                .collect();
            if let Some(last) = args.value("--last") {
                let last: usize = last.parse().map_err(|_| Failure::usage("--last takes a number"))?;
                requests = requests.split_off(requests.len().saturating_sub(last));
            }
            Ok(json!({ "requests": requests }))
        }
        "requests" => match args.first() {
            Some(name) => driver::fields(name),
            None => Ok(json!({ "requests": driver::requests() })),
        },
        "help" | "--help" | "-h" => {
            println!("{USAGE}");
            Ok(Value::Null)
        }
        other => Err(Failure::usage(format!("Unknown command {other}.\n\n{USAGE}"))),
    }
}

/// Sends `text` to `agent` as the person, waits until its run is over, and
/// returns the entries it added (by seq, their last version) and its last
/// reply.
async fn ask(sandbox: &Sandbox, agent: &str, text: &str, wait: Duration) -> Outcome {
    use factory_protocol::v1::{AgentStatus, server_message::Body};
    use prost::Message as _;
    let mut connection = Connection::open(sandbox).await?;
    let agent_id = agent_id(&connection, sandbox, agent)?;
    connection.request("SelectAgent", &json!({ "agent_id": agent_id })).await?;
    connection.request("SendMessage", &json!({ "agent_id": agent_id, "text": text })).await?;
    let busy = |connection: &Connection| {
        connection
            .workspace
            .agents
            .iter()
            .find(|candidate| candidate.id == agent_id)
            .is_some_and(|found| found.status() != AgentStatus::Idle || found.queued_messages > 0)
    };
    let mut entries = std::collections::BTreeMap::new();
    let mut started = busy(&connection);
    let deadline = Instant::now() + wait;
    while !started || busy(&connection) {
        let left = deadline.saturating_duration_since(Instant::now());
        let message = if left.is_zero() { None } else { connection.receive(left).await? };
        let Some(message) = message else {
            return Err(Failure::failed(format!("{agent} wasn't done after {} s.", wait.as_secs())));
        };
        if let Some(Body::Entry(update)) = &message.body
            && update.agent_id == agent_id
            && let Some(entry) = &update.entry
        {
            entries.insert(entry.seq, entry.clone());
            started = true;
        }
        // Streamed text: appended to its entry, as the app does.
        if let Some(Body::Delta(delta)) = &message.body
            && delta.agent_id == agent_id
            && let Some(entry) = entries.get_mut(&delta.seq)
        {
            match &mut entry.kind {
                Some(factory_protocol::v1::entry::Kind::Assistant(assistant)) => assistant.text.push_str(&delta.text),
                Some(factory_protocol::v1::entry::Kind::Thinking(thinking)) => thinking.text.push_str(&delta.text),
                _ => {}
            }
        }
        started |= busy(&connection);
    }
    let reply = entries.values().rev().find_map(|entry| match &entry.kind {
        Some(factory_protocol::v1::entry::Kind::Assistant(assistant)) if !assistant.text.is_empty() => Some(assistant.text.clone()),
        _ => None,
    });
    let entries: Vec<Value> = entries.values().map(|entry| driver::to_json("Entry", &entry.encode_to_vec())).collect();
    Ok(json!({ "reply": reply, "entries": entries }))
}

/// The id of the agent `agent` names, by id or name.
fn agent_id(connection: &Connection, sandbox: &Sandbox, agent: &str) -> Result<String, Failure> {
    connection
        .workspace
        .agents
        .iter()
        .find(|candidate| candidate.id == agent || candidate.name.eq_ignore_ascii_case(agent))
        .map(|found| found.id.clone())
        .ok_or_else(|| Failure::refused(format!("There's no agent {agent} in {}.", sandbox.name)))
}

/// An agent's entries, oldest first: the Transcript the app gets when it
/// opens the agent, or with `last` or `from`, a LoadEntries page ending
/// where that Transcript ends (LoadEntries's `before_seq` is an exclusive
/// end, so 0 is an empty page).
async fn transcript(sandbox: &Sandbox, agent: &str, last: Option<u64>, from: Option<u64>) -> Outcome {
    use factory_protocol::v1::server_message::Body;
    use prost::Message as _;
    let mut connection = Connection::open(sandbox).await?;
    let agent_id = agent_id(&connection, sandbox, agent)?;
    connection.request("SelectAgent", &json!({ "agent_id": agent_id })).await?;
    let deadline = Instant::now() + Duration::from_secs(30);
    let opened = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let message = if left.is_zero() { None } else { connection.receive(left).await? };
        let Some(message) = message else { return Err(Failure::failed(format!("{agent}'s transcript didn't come in 30 s."))) };
        if let Some(Body::Transcript(transcript)) = message.body
            && transcript.agent_id == agent_id
        {
            break transcript;
        }
    };
    if last.is_none() && from.is_none() {
        let entries: Vec<Value> = opened.entries.iter().map(|entry| driver::to_json("Entry", &entry.encode_to_vec())).collect();
        return Ok(json!({ "entries": entries, "has_earlier": opened.has_earlier }));
    }
    let end = opened.entries.last().map_or(0, |entry| entry.seq + 1);
    let mut request = json!({ "agent_id": agent_id, "before_seq": end, "limit": last.unwrap_or(0) });
    if let Some(from) = from {
        request["from_seq"] = json!(from);
    }
    let page = connection.request("LoadEntries", &request).await?;
    let page = page.get("entry_page").cloned().unwrap_or(Value::Null);
    Ok(
        json!({ "entries": page.get("entries").cloned().unwrap_or(json!([])), "has_earlier": page.get("has_earlier").cloned().unwrap_or(json!(false)) }),
    )
}

/// What `path` names in `value`, or all of it.
fn pick(value: &Value, path: Option<&str>) -> Outcome {
    match path {
        None => Ok(json!({ "workspace": value })),
        Some(path) => {
            driver::select(value, path).map(|found| json!({ "value": found })).ok_or_else(|| Failure::failed(format!("Nothing at {path}.")))
        }
    }
}
