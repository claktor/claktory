//! Sandboxes: throwaway Claktories, each a factoryd of its own with a fresh
//! home, a free port and, unless left out, its own account service whose
//! emails land in a folder. They live under the QA root (`CLAKTORY_QA_ROOT`,
//! or `claktory-qa` in the temporary folder), one folder each:
//!
//! ```text
//! <root>/sandboxes/<name>/sandbox.json   what runs: ports, processes, binaries
//!                         home/          FACTORY_HOME
//!                         user/          HOME for factoryd and its agents
//!                         mail/          the account service's emails, one file each
//!                         account/       the account service's database and keys
//!                         model.json     qa-model's script (with --model), read for every request
//!                         home/models/speech/  the speech model (with --speech), copied in
//!                         model.jsonl    what qa-model was asked and answered, a line each
//!                         openrouter-models.json  OpenRouter's model list as qa-model serves it,
//!                                        if written (else a built-in one), read for every fetch
//!                         claude.json    until when the fake Claude sign-in is at its limit (with --claude)
//!                         claude-files/  the images uploaded to qa-model's Files API (with --claude), one file each
//!                         home/auth.json the fake Claude sign-in (with --claude): fake tokens only
//!                         factoryd.log, account.log, model.out
//! ```
//!
//! Nothing here may reach the person's Claktories or claktory.com: the root,
//! the binaries and the ports are checked, and refused, before anything
//! starts (`check_root`, `check_binaries`, `check_port`), and a sandbox's
//! settings name only addresses on this computer.

use std::io::Write as _;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::Failure;

/// The real Claktory's port and the dev Claktory's: never a sandbox's.
pub const REAL_PORTS: [u16; 2] = [7420, 7421];
/// Where free ports are looked for.
const PORTS: std::ops::Range<u16> = 7500..8000;
/// An address nothing answers on: the relays and, without an account
/// service, the service, so nothing leaves this computer.
pub const NOWHERE: &str = "http://127.0.0.1:9";
/// How long a sandbox may take to start.
const START_WAIT: Duration = Duration::from_secs(60);

/// The folder every sandbox lives in, created and checked: `sandboxes` in
/// the QA root (where scripts/qa.sh also keeps its builds).
pub fn root() -> Result<PathBuf, Failure> {
    let root = match std::env::var_os("CLAKTORY_QA_ROOT") {
        Some(root) if !root.is_empty() => PathBuf::from(root),
        _ => std::env::temp_dir().join("claktory-qa"),
    };
    let root = resolved(&root.join("sandboxes"));
    // Checked before anything is made there.
    check_root(&root)?;
    std::fs::create_dir_all(&root).map_err(|error| Failure::failed(format!("Can't make {}: {error}", root.display())))?;
    Ok(root)
}

/// `path` with its links resolved, as far as it exists.
fn resolved(path: &Path) -> PathBuf {
    let mut existing = path.to_owned();
    let mut rest = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name().map(ToOwned::to_owned) else { break };
        rest.push(name);
        if !existing.pop() {
            break;
        }
    }
    let mut path = canonical(&existing).unwrap_or(existing);
    path.extend(rest.into_iter().rev());
    path
}

/// The person's Claktories' folders: `~/.factory` and `~/.factory-dev`,
/// and on Windows `%LOCALAPPDATA%\Claktory\factory` and `factory-dev`,
/// with the programs folder the app and its backend are installed in.
fn real_homes() -> Vec<PathBuf> {
    let homes = [false, true].into_iter().filter_map(factoryd::home::instance_root);
    #[cfg(windows)]
    let homes = homes.chain(factoryd::windows::dirs::programs().ok());
    homes.map(|path| canonical(&path).unwrap_or(path)).collect()
}

/// `path` with its links resolved; on Windows without the `\\?\` it
/// puts first, so paths compare as they're written.
fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    return factoryd::windows::fs::canonical(path);
    #[cfg(not(windows))]
    path.canonicalize()
}

/// The real Claktory whose own files `path` is among: its settings, data,
/// binaries and releases. Its agents' folders and checkouts (`agents`,
/// `trees`, `workspace`) are theirs to work in, and don't count.
fn real_owner(path: &Path) -> Option<PathBuf> {
    real_homes()
        .into_iter()
        .find(|real| path.starts_with(real) && !["agents", "trees", "workspace"].iter().any(|theirs| path.starts_with(real.join(theirs))))
}

/// Refuses a root among a real Claktory's own files, or one that is the
/// home folder or `/` itself (`down` deletes what's under it).
pub fn check_root(root: &Path) -> Result<(), Failure> {
    if let Some(real) = real_owner(root) {
        return Err(Failure::refused(format!(
            "{} is among the files of the real Claktory in {}. Sandboxes live elsewhere: unset CLAKTORY_QA_ROOT, or point it at a folder of their own.",
            root.display(),
            real.display()
        )));
    }
    let home = std::env::home_dir().map(|home| canonical(&home).unwrap_or(home));
    if root.parent().is_none() || home.as_deref() == Some(root) {
        return Err(Failure::refused(format!("{} is too broad a root for sandboxes: give them a folder of their own.", root.display())));
    }
    Ok(())
}

/// Refuses binaries from a real Claktory's folder: a sandbox runs what was
/// built for it.
pub fn check_binaries(dir: &Path) -> Result<(), Failure> {
    let dir = canonical(dir).unwrap_or_else(|_| dir.to_owned());
    if let Some(real) = real_owner(&dir) {
        return Err(Failure::refused(format!(
            "claktory-qa runs from the files of the real Claktory in {}. Run the one scripts/qa.sh builds.",
            real.display()
        )));
    }
    Ok(())
}

/// Refuses the real Claktories' ports and the system's.
pub fn check_port(port: u16) -> Result<(), Failure> {
    if REAL_PORTS.contains(&port) {
        return Err(Failure::refused(format!("Port {port} is a real Claktory's. A sandbox takes one of its own.")));
    }
    if port < 1024 {
        return Err(Failure::refused(format!("Port {port} is the system's. Choose one from 1024 up.")));
    }
    Ok(())
}

/// A sandbox's name: letters, digits and dashes.
pub fn check_name(name: &str) -> Result<(), Failure> {
    let fine = !name.is_empty()
        && name.len() <= 32
        && name.chars().all(|character| character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-')
        && !name.starts_with('-');
    if fine { Ok(()) } else { Err(Failure::usage(format!("\"{name}\" can't name a sandbox: use lowercase letters, digits and dashes."))) }
}

/// A port nothing listens on, from `PORTS`, other than `taken`. The search
/// starts somewhere random, so sandboxes started at once from different
/// roots (tests running side by side) rarely pick the same.
fn free_port(taken: &[u16]) -> Result<u16, Failure> {
    let span = PORTS.end - PORTS.start;
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |since| since.subsec_nanos());
    let offset = u16::try_from((nanos ^ std::process::id().wrapping_mul(2_654_435_761)) % u32::from(span)).unwrap_or(0);
    (0..span)
        .map(|step| PORTS.start + (offset + step) % span)
        .filter(|port| !taken.contains(port) && !REAL_PORTS.contains(port))
        .find(|port| TcpListener::bind(("127.0.0.1", *port)).is_ok() && TcpStream::connect(("127.0.0.1", *port)).is_err())
        .ok_or_else(|| Failure::failed(format!("No free port between {} and {}.", PORTS.start, PORTS.end)))
}

/// A sandbox as `sandbox.json` records it.
#[derive(Clone, Debug)]
pub struct Sandbox {
    pub name: String,
    pub dir: PathBuf,
    pub port: u16,
    pub pid: i32,
    pub account_port: Option<u16>,
    pub account_pid: Option<i32>,
    pub model_port: Option<u16>,
    pub model_pid: Option<i32>,
    pub bin: PathBuf,
    pub commit: String,
    /// The machine name its Claktory shows (devices, the footer, move
    /// codes): never this computer's own.
    pub machine_name: String,
    /// It recognizes speech (`up --speech`): its factoryd runs the
    /// commit's claktory-speech, on the model copied into its home.
    pub speech: bool,
    /// Its Claude is signed in, fake (`up --claude`): Anthropic's side of
    /// qa-model, through a QA build of factoryd, with its agents on Claude.
    pub claude: bool,
    /// A second fake Claude account is signed in after the first
    /// (`--claude-accounts 2`).
    pub claude_second: bool,
    /// The account it claims to be (`up --claim-email`, `--claim-claktory-id`):
    /// a QA build of factoryd lies about whose it is (E11 row 6).
    pub claim_email: Option<String>,
    pub claim_claktory_id: Option<String>,
    /// Who made it (`caller`), and when (ms since 1970): `down --all` ends
    /// only the caller's own. Empty and 0 for sandboxes from before.
    pub owner: String,
    pub created_ms: i64,
}

impl Sandbox {
    pub fn home(&self) -> PathBuf {
        self.dir.join("home")
    }

    pub fn mail(&self) -> PathBuf {
        self.dir.join("mail")
    }

    pub fn factoryd(&self) -> PathBuf {
        self.bin.join(program("factoryd"))
    }

    pub fn account_service(&self) -> String {
        self.account_port.map_or_else(String::new, |port| format!("http://127.0.0.1:{port}"))
    }

    pub fn model_script(&self) -> PathBuf {
        self.dir.join("model.json")
    }

    pub fn model_log(&self) -> PathBuf {
        self.dir.join("model.jsonl")
    }

    /// Until when its fake Claude sign-in is at its usage limit (`limit`).
    pub fn claude_limit(&self) -> PathBuf {
        self.dir.join("claude.json")
    }

    pub fn running(&self) -> bool {
        alive(self.pid, &self.factoryd())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "name": self.name,
            "running": self.running(),
            "port": self.port,
            "home": self.home(),
            "folder": self.dir,
            "account_service": self.account_service(),
            "mail": self.account_port.map(|_| self.mail()),
            "model": self.model_port.map(|_| if self.claude { crate::model::claude::MODEL } else { crate::model::MODEL }),
            "model_script": self.model_port.map(|_| self.model_script()),
            "model_log": self.model_port.map(|_| self.model_log()),
            "openrouter_models": self.model_port.map(|_| self.dir.join(crate::model::MODELS_FILE)),
            "log": self.dir.join("factoryd.log"),
            "commit": self.commit,
            "machine_name": self.machine_name,
            "speech": self.speech,
            "claims": (self.claim_email.is_some() || self.claim_claktory_id.is_some())
                .then(|| json!({"email": self.claim_email, "claktory_id": self.claim_claktory_id})),
            "claude": self.claude.then(|| json!({
                "account": crate::model::claude::EMAIL,
                "limited_until_ms": crate::model::claude::limited_until(&self.claude_limit()),
                "reads_full_until_ms": crate::model::claude::reads_full_until(&self.claude_limit()),
                "second": self.claude_second.then(|| {
                    let limit = crate::model::claude::limit_file(&self.claude_limit(), 2);
                    json!({
                        "account": crate::model::claude::EMAIL_2,
                        "limited_until_ms": crate::model::claude::limited_until(&limit),
                        "reads_full_until_ms": crate::model::claude::reads_full_until(&limit),
                    })
                }),
            })),
            "owner": self.owner,
            "created_ms": self.created_ms,
        })
    }

    fn record(&self) -> Value {
        json!({
            "name": self.name,
            "port": self.port,
            "pid": self.pid,
            "account_port": self.account_port,
            "account_pid": self.account_pid,
            "model_port": self.model_port,
            "model_pid": self.model_pid,
            "bin": self.bin,
            "commit": self.commit,
            "machine_name": self.machine_name,
            "speech": self.speech,
            "claude": self.claude,
            "claude_second": self.claude_second,
            "claim_email": self.claim_email,
            "claim_claktory_id": self.claim_claktory_id,
            "owner": self.owner,
            "created_ms": self.created_ms,
        })
    }

    fn save(&self) -> Result<(), Failure> {
        let text = serde_json::to_string_pretty(&self.record()).unwrap_or_default();
        std::fs::write(self.dir.join("sandbox.json"), text).map_err(|error| Failure::failed(format!("Can't save the sandbox: {error}")))
    }

    fn load(dir: &Path) -> Option<Sandbox> {
        let record: Value = serde_json::from_str(&std::fs::read_to_string(dir.join("sandbox.json")).ok()?).ok()?;
        let number = |key: &str| record.get(key).and_then(Value::as_i64);
        let name = record.get("name")?.as_str()?.to_owned();
        Some(Sandbox {
            // Sandboxes from before it had one: the default.
            machine_name: record.get("machine_name").and_then(Value::as_str).map_or_else(|| default_machine_name(&name), str::to_owned),
            name,
            dir: dir.to_owned(),
            port: u16::try_from(number("port")?).ok()?,
            pid: i32::try_from(number("pid")?).ok()?,
            account_port: number("account_port").and_then(|port| u16::try_from(port).ok()),
            account_pid: number("account_pid").and_then(|pid| i32::try_from(pid).ok()),
            model_port: number("model_port").and_then(|port| u16::try_from(port).ok()),
            model_pid: number("model_pid").and_then(|pid| i32::try_from(pid).ok()),
            bin: PathBuf::from(record.get("bin")?.as_str()?),
            commit: record.get("commit").and_then(Value::as_str).unwrap_or_default().to_owned(),
            speech: record.get("speech").and_then(Value::as_bool).unwrap_or(false),
            claude: record.get("claude").and_then(Value::as_bool).unwrap_or(false),
            claude_second: record.get("claude_second").and_then(Value::as_bool).unwrap_or(false),
            claim_email: record.get("claim_email").and_then(Value::as_str).map(str::to_owned),
            claim_claktory_id: record.get("claim_claktory_id").and_then(Value::as_str).map(str::to_owned),
            owner: record.get("owner").and_then(Value::as_str).unwrap_or_default().to_owned(),
            created_ms: number("created_ms").unwrap_or(0),
        })
    }
}

/// Who's running claktory-qa, so each sandbox records who made it and
/// `down --all` ends only the caller's own (the QA root is shared by every
/// agent on this computer): `CLAKTORY_QA_OWNER`, else `FACTORY_AGENT_ID`,
/// else the agent whose checkout scripts/qa.sh ran from
/// (`…/trees/<id>/…`), else that checkout's path.
pub fn caller() -> String {
    for key in ["CLAKTORY_QA_OWNER", "FACTORY_AGENT_ID"] {
        if let Some(owner) = std::env::var(key).ok().map(|owner| owner.trim().to_owned()).filter(|owner| !owner.is_empty()) {
            return owner;
        }
    }
    let repo = std::env::var_os("CLAKTORY_QA_REPO").map(PathBuf::from).or_else(|| std::env::current_dir().ok()).unwrap_or_default();
    owner_of(&repo)
}

/// The agent a checkout at `repo` belongs to (`…/trees/<id>/…`), or its path.
fn owner_of(repo: &Path) -> String {
    let parts: Vec<&std::ffi::OsStr> = repo.iter().collect();
    parts
        .windows(2)
        .rev()
        .find(|pair| pair[0] == "trees")
        .and_then(|pair| pair[1].to_str())
        .map_or_else(|| repo.display().to_string(), str::to_owned)
}

/// The machine name a sandbox shows unless `up --machine-name` chose one.
fn default_machine_name(sandbox: &str) -> String {
    format!("qa-{sandbox}")
}

/// The sandbox called `name`.
pub fn find(name: &str) -> Result<Sandbox, Failure> {
    check_name(name)?;
    let dir = root()?.join(name);
    Sandbox::load(&dir).ok_or_else(|| Failure::refused(format!("There's no sandbox called {name}: `claktory-qa up {name}` makes one.")))
}

/// Every sandbox under the root.
pub fn all() -> Result<Vec<Sandbox>, Failure> {
    let root = root()?;
    let mut sandboxes: Vec<Sandbox> = std::fs::read_dir(&root)
        .map_err(|error| Failure::failed(format!("Can't read {}: {error}", root.display())))?
        .filter_map(|entry| Sandbox::load(&entry.ok()?.path()))
        .collect();
    sandboxes.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(sandboxes)
}

/// What `up` is asked for.
pub struct Up {
    pub name: Option<String>,
    pub port: Option<u16>,
    pub account: bool,
    /// qa-model for its agents, with this script (or the built-in answers).
    pub model: Option<Option<PathBuf>>,
    /// The machine name it shows; "qa-<name>" when left out.
    pub machine_name: Option<String>,
    /// Speech recognition, on this model (a GGUF with its `.receipt`
    /// beside it, as a Claktory keeps it in models/speech).
    pub speech: Option<PathBuf>,
    /// Claude signed in, fake, on qa-model (which this starts), with every
    /// agent on Claude: `limit` puts it at its usage limit.
    pub claude: bool,
    /// With `claude`, a second fake account after the first.
    pub claude_second: bool,
    /// The account its factoryd claims (a QA build's lie, E11 row 6).
    pub claim_email: Option<String>,
    pub claim_claktory_id: Option<String>,
}

/// Starts a sandbox: its account service (unless left out), its settings,
/// then factoryd, and waits until it answers.
pub fn up(request: Up) -> Result<Sandbox, Failure> {
    let bin = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_owned))
        .ok_or_else(|| Failure::failed("Can't tell where claktory-qa is."))?;
    check_binaries(&bin)?;
    if let Some(Some(script)) = &request.model {
        let text = std::fs::read_to_string(script).map_err(|error| Failure::usage(format!("Can't read {}: {error}", script.display())))?;
        crate::model::check_script(&text).map_err(Failure::usage)?;
    }
    if request.speech.is_some() && !bin.join(program("claktory-speech")).is_file() {
        return Err(Failure::failed(format!(
            "There's no claktory-speech beside claktory-qa in {}: run `scripts/qa.sh up --speech MODEL`, which builds it.",
            bin.display()
        )));
    }
    let speech = request.speech.as_deref().map(speech_model).transpose()?;
    for binary in ["factoryd", "claktory-account"] {
        if !bin.join(program(binary)).is_file() {
            return Err(Failure::failed(format!(
                "There's no {binary} beside claktory-qa in {}: run claktory-qa through scripts/qa.sh, which builds them.",
                bin.display()
            )));
        }
    }
    let root = root()?;
    let taken: Vec<Sandbox> = all()?;
    let name = match request.name {
        Some(name) => name,
        None => (1..).map(|number| format!("qa{number}")).find(|name| !root.join(name).exists()).unwrap_or_default(),
    };
    check_name(&name)?;
    let machine_name = match request.machine_name.map(|chosen| chosen.trim().to_owned()) {
        Some(chosen) if chosen.is_empty() || chosen.chars().count() > 64 || chosen.chars().any(char::is_control) => {
            return Err(Failure::usage("--machine-name takes a name of up to 64 characters."));
        }
        Some(chosen) => chosen,
        None => default_machine_name(&name),
    };
    let dir = root.join(&name);
    if dir.exists() {
        return Err(Failure::refused(format!("There's already a sandbox called {name}: `claktory-qa down {name}` ends it.")));
    }
    let ports: Vec<u16> =
        taken.iter().flat_map(|sandbox| [Some(sandbox.port), sandbox.account_port, sandbox.model_port]).flatten().collect();
    let port = match request.port {
        Some(port) => {
            check_port(port)?;
            port
        }
        None => free_port(&ports)?,
    };
    for folder in ["home", "user", "mail", "account"] {
        std::fs::create_dir_all(dir.join(folder)).map_err(|error| Failure::failed(format!("Can't make {}: {error}", dir.display())))?;
    }
    let mut sandbox = Sandbox {
        name,
        dir: dir.clone(),
        port,
        pid: 0,
        account_port: None,
        account_pid: None,
        model_port: None,
        model_pid: None,
        bin: bin.clone(),
        commit: std::env::var("CLAKTORY_QA_COMMIT").unwrap_or_default(),
        machine_name,
        speech: speech.is_some(),
        claude: request.claude,
        claude_second: request.claude && request.claude_second,
        claim_email: request.claim_email.clone(),
        claim_claktory_id: request.claim_claktory_id.clone(),
        owner: caller(),
        created_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |since| i64::try_from(since.as_millis()).unwrap_or(i64::MAX)),
    };
    let started = speech.map_or(Ok(()), |(model, receipt)| copy_speech_model(&sandbox.home(), &model, &receipt)).and_then(|()| {
        // Claude's side is qa-model's: it starts with the built-in answers unless given a script.
        let model = if request.claude { Some(request.model.clone().unwrap_or(None)) } else { request.model.clone() };
        start(&mut sandbox, request.account, model.as_ref(), &ports)
    });
    if let Err(failure) = started {
        // Whatever started goes again, and the folder with it.
        stop(&sandbox);
        let _ = std::fs::remove_dir_all(&dir);
        return Err(failure);
    }
    Ok(sandbox)
}

fn start(sandbox: &mut Sandbox, account: bool, model: Option<&Option<PathBuf>>, taken: &[u16]) -> Result<(), Failure> {
    let dir = sandbox.dir.clone();
    let mut taken = taken.to_vec();
    taken.push(sandbox.port);
    if let Some(script) = model {
        let text = match script {
            Some(script) => {
                std::fs::read_to_string(script).map_err(|error| Failure::usage(format!("Can't read {}: {error}", script.display())))?
            }
            None => "{\"rules\": []}\n".to_owned(),
        };
        std::fs::write(sandbox.model_script(), text)
            .map_err(|error| Failure::failed(format!("Can't write the model's script: {error}")))?;
        let port = free_port(&taken)?;
        taken.push(port);
        let log = log_file(&dir.join("model.out"))?;
        let mut serve = command(&sandbox.bin.join(program("claktory-qa")), sandbox);
        serve
            .args(["serve-model", "--port", &port.to_string(), "--script"])
            .arg(sandbox.model_script())
            .arg("--log")
            .arg(sandbox.model_log());
        if sandbox.claude {
            for account in [1, 2] {
                crate::model::claude::set_limit(&crate::model::claude::limit_file(&sandbox.claude_limit(), account), None, false)
                    .map_err(|error| Failure::failed(format!("Can't write Claude's limit: {error}")))?;
            }
            serve.arg("--claude").arg(sandbox.claude_limit());
        }
        serve.stdin(Stdio::null()).stdout(log.try_clone().map_err(|error| Failure::failed(error.to_string()))?).stderr(log);
        let child = spawn_detached(&mut serve).map_err(|error| Failure::failed(format!("Can't start qa-model: {error}")))?;
        sandbox.model_port = Some(port);
        sandbox.model_pid = i32::try_from(child.id()).ok();
        drop(child);
        sandbox.save()?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        wait_for(
            START_WAIT,
            || TcpStream::connect(address).is_ok(),
            || alive(sandbox.model_pid.unwrap_or(0), &sandbox.bin.join(program("claktory-qa"))),
        )
        .map_err(|why| Failure::failed(format!("qa-model didn't start ({why}). Its output: {}", dir.join("model.out").display())))?;
    }
    if account {
        let port = free_port(&taken)?;
        let secrets = dir.join("account/secrets.json");
        let made = command(&sandbox.bin.join(program("claktory-account")), sandbox)
            .arg("init-secrets")
            .arg(&secrets)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map_err(|error| Failure::failed(format!("Can't run claktory-account: {error}")))?;
        if !made.success() {
            return Err(Failure::failed("claktory-account couldn't make its keys."));
        }
        let log = log_file(&dir.join("account.log"))?;
        let mut serve = command(&sandbox.bin.join(program("claktory-account")), sandbox);
        serve
            .arg("serve")
            .arg("--db")
            .arg(dir.join("account/account.db"))
            .arg("--secrets")
            .arg(&secrets)
            .args(["--listen", &format!("127.0.0.1:{port}"), "--email", "folder", "--mail-dir"])
            .arg(sandbox.mail())
            .stdin(Stdio::null())
            .stdout(log.try_clone().map_err(|error| Failure::failed(error.to_string()))?)
            .stderr(log);
        let child = spawn_detached(&mut serve).map_err(|error| Failure::failed(format!("Can't start claktory-account: {error}")))?;
        sandbox.account_port = Some(port);
        sandbox.account_pid = i32::try_from(child.id()).ok();
        drop(child);
        sandbox.save()?;
        let address = SocketAddr::from(([127, 0, 0, 1], port));
        wait_for(
            START_WAIT,
            || TcpStream::connect(address).is_ok(),
            || alive(sandbox.account_pid.unwrap_or(0), &sandbox.bin.join(program("claktory-account"))),
        )
        .map_err(|why| {
            Failure::failed(format!("The account service didn't start ({why}). Its log: {}", dir.join("account.log").display()))
        })?;
    }
    let model = match (sandbox.model_port, sandbox.claude) {
        (Some(_), true) => Some(crate::model::claude::MODEL),
        (Some(_), false) => Some(crate::model::MODEL),
        (None, _) => None,
    };
    write_settings(&sandbox.home(), sandbox.account_port, model)?;
    if sandbox.claude {
        write_claude_sign_in(&sandbox.home(), sandbox.claude_second)?;
    }
    launch(sandbox)
}

/// The fake Claude sign-in, straight into the sandbox's auth.json (only
/// the sandbox's: its home was checked like the rest), with the fixed fake
/// tokens qa-model alone accepts.
fn write_claude_sign_in(home: &Path, second: bool) -> Result<(), Failure> {
    let mut options = std::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file =
        options.open(home.join("auth.json")).map_err(|error| Failure::failed(format!("Can't write the fake Claude sign-in: {error}")))?;
    file.write_all(crate::model::claude::credentials(second).to_string().as_bytes())
        .map_err(|error| Failure::failed(format!("Can't write the fake Claude sign-in: {error}")))
}

/// Starts the sandbox's factoryd, with the environment it always has, and
/// waits until it answers.
fn launch(sandbox: &mut Sandbox) -> Result<(), Failure> {
    let dir = sandbox.dir.clone();
    let log = log_file(&dir.join("factoryd.log"))?;
    let mut factoryd = command(&sandbox.factoryd(), sandbox);
    factoryd.envs(environment(sandbox));
    // Kept across restarts, as factoryd keeps its token: the old one is gone
    // by now, so the port answering is the new one. The QA person's token,
    // or, from a commit before it, this machine's pairing code.
    let pairing = sandbox.home().join("qa-person-token");
    let old_pairing = sandbox.home().join("remote/local-pairing");
    let logged = std::fs::metadata(dir.join("factoryd.log")).map_or(0, |log| log.len());
    factoryd.stdin(Stdio::null()).stdout(log.try_clone().map_err(|error| Failure::failed(error.to_string()))?).stderr(log);
    let child = spawn_detached(&mut factoryd).map_err(|error| Failure::failed(format!("Can't start factoryd: {error}")))?;
    sandbox.pid = i32::try_from(child.id()).unwrap_or(0);
    drop(child);
    sandbox.save()?;
    let address = SocketAddr::from(([127, 0, 0, 1], sandbox.port));
    wait_for(START_WAIT, || (pairing.is_file() || old_pairing.is_file()) && TcpStream::connect(address).is_ok(), || sandbox.running())
        .map_err(|why| Failure::failed(format!("factoryd didn't start ({why}). Its log: {}", dir.join("factoryd.log").display())))?;
    // A factoryd built without the QA provider override (a release's, or
    // not through scripts/qa.sh) would send the fake sign-in to Anthropic:
    // only one that says it took the override goes on.
    if sandbox.claude {
        let log = std::fs::read(dir.join("factoryd.log")).unwrap_or_default();
        let since = String::from_utf8_lossy(log.get(usize::try_from(logged).unwrap_or(0)..).unwrap_or_default()).into_owned();
        if !since.contains("QA provider override: http://127.0.0.1:") {
            return Err(Failure::failed(
                "This factoryd wasn't built with the QA provider override (factoryd's qa-provider feature), so its Claude can't be qa-model: run it through scripts/qa.sh, which builds it so.",
            ));
        }
    }
    Ok(())
}

/// A speech model to copy in: the GGUF and its receipt, both files.
fn speech_model(model: &Path) -> Result<(PathBuf, PathBuf), Failure> {
    let is_model = model.extension().is_some_and(|extension| extension == "gguf");
    if !is_model || !model.is_file() {
        return Err(Failure::usage(format!("--speech takes a speech model's .gguf file; {} isn't one.", model.display())));
    }
    let receipt = model.with_extension("receipt");
    if !receipt.is_file() {
        return Err(Failure::usage(format!(
            "There's no {} beside the model: give the one a Claktory keeps in models/speech, receipt and all.",
            receipt.display()
        )));
    }
    Ok((model.to_owned(), receipt))
}

/// Copies the speech model and its receipt into the sandbox's models/speech,
/// where its factoryd looks. A copy (a clone, on APFS), never a link: the
/// sandbox can delete or replace it without touching the original, which is
/// only read. The receipt names the model's modified time, so the copy keeps
/// it.
fn copy_speech_model(home: &Path, model: &Path, receipt: &Path) -> Result<(), Failure> {
    let dir = home.join("models/speech");
    std::fs::create_dir_all(&dir).map_err(|error| Failure::failed(format!("Can't make {}: {error}", dir.display())))?;
    for file in [model, receipt] {
        let Some(name) = file.file_name() else { continue };
        let copy = dir.join(name);
        let copied = std::fs::copy(file, &copy).and_then(|_| {
            let modified = std::fs::metadata(file)?.modified()?;
            std::fs::File::options().write(true).open(&copy)?.set_modified(modified)
        });
        copied.map_err(|error| Failure::failed(format!("Can't copy {} into the sandbox: {error}", file.display())))?;
    }
    Ok(())
}

/// A command for one of the sandbox's binaries, with nothing of this
/// environment but what any program needs: no keys, no FACTORY_*, and a
/// home folder of its own. PATH leaves out the real Claktories' binaries.
fn command(program: &Path, sandbox: &Sandbox) -> Command {
    let mut command = Command::new(program);
    command.env_clear();
    #[cfg(unix)]
    let kept = ["LANG", "LC_ALL", "USER", "LOGNAME", "SHELL", "TMPDIR", "TERM"].as_slice();
    // What Windows' own programs need, and where Claktory's Git Bash is
    // when it isn't beside factoryd (`factory_agent::windows::shell`).
    #[cfg(windows)]
    let kept = [
        "SystemRoot",
        "SystemDrive",
        "windir",
        "ComSpec",
        "PATHEXT",
        "TEMP",
        "TMP",
        "USERNAME",
        "NUMBER_OF_PROCESSORS",
        "PROCESSOR_ARCHITECTURE",
        factory_agent::windows::shell::TOOLS_VARIABLE,
    ]
    .as_slice();
    for &key in kept {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    let real = real_homes();
    let mut path = vec![sandbox.bin.clone()];
    if let Some(inherited) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&inherited).filter(|dir| !real.iter().any(|real| dir.starts_with(real))));
    }
    if let Ok(joined) = std::env::join_paths(path) {
        command.env("PATH", joined);
    }
    command.env("HOME", sandbox.dir.join("user"));
    command.current_dir(&sandbox.dir);
    command
}

/// The settings a sandbox starts with: Claktory's defaults, with the account
/// service its own (or nowhere) and the tunnel's relays nowhere, so pipes
/// and devices connect directly on this computer; with qa-model, every
/// agent on it, Rivet included.
fn write_settings(home: &Path, account_port: Option<u16>, model: Option<&str>) -> Result<(), Failure> {
    let mut config = factoryd::config::Config::default();
    if let Some(model) = model {
        config.default_model = model.to_owned();
        config.default_effort = "off".to_owned();
        for agent in &mut config.agents {
            agent.model = model.to_owned();
            agent.effort = "off".to_owned();
        }
    }
    match account_port {
        Some(port) => config.account.service = format!("http://127.0.0.1:{port}"),
        None => {
            config.account.enabled = false;
            config.account.service = NOWHERE.to_owned();
        }
    }
    config.remote = Some(factoryd::config::RemoteConfig { relays: vec![format!("{NOWHERE}/")], ..Default::default() });
    let text = config.to_json(home).map_err(|error| Failure::failed(error.to_string()))?;
    std::fs::write(home.join("config.json"), text).map_err(|error| Failure::failed(format!("Can't write the settings: {error}")))
}

fn log_file(path: &Path) -> Result<std::fs::File, Failure> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|error| Failure::failed(format!("Can't open {}: {error}", path.display())))
}

/// Waits until `ready`, while `alive`.
fn wait_for(wait: Duration, mut ready: impl FnMut() -> bool, mut alive: impl FnMut() -> bool) -> Result<(), String> {
    let deadline = Instant::now() + wait;
    loop {
        if ready() {
            return Ok(());
        }
        if !alive() {
            return Err("it stopped".to_owned());
        }
        if Instant::now() > deadline {
            return Err(format!("not ready after {} s", wait.as_secs()));
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// `pid` is running, and is `program` (not another process that took the
/// number since): asked of the system itself, never of `ps`, which an
/// agent's sandbox may not let run (it's setuid on a Mac).
#[cfg(unix)]
fn alive(pid: i32, program: &Path) -> bool {
    // SAFETY: signal 0 sends nothing; it only asks whether `pid` exists.
    if pid <= 0 || unsafe { libc::kill(pid, 0) } != 0 {
        return false;
    }
    let program = canonical(program).unwrap_or_else(|_| program.to_owned());
    running_program(pid).is_some_and(|running| running == program)
}

/// The file `pid` runs, links resolved; None once it has ended (a zombie
/// has none).
#[cfg(target_os = "macos")]
fn running_program(pid: i32) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStringExt as _;
    let mut buffer = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `buffer` has room for the size given.
    let written = unsafe { libc::proc_pidpath(pid, buffer.as_mut_ptr().cast(), buffer.len() as u32) };
    let written = usize::try_from(written).ok().filter(|written| *written > 0)?;
    buffer.truncate(written);
    let path = PathBuf::from(std::ffi::OsString::from_vec(buffer));
    Some(canonical(&path).unwrap_or(path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn running_program(pid: i32) -> Option<PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}

/// As above, on Windows, where a process's file is its image, links
/// resolved and case aside.
#[cfg(windows)]
fn alive(pid: i32, program: &Path) -> bool {
    use factory_agent::windows::system::{image_path, open_process};
    let Ok(pid) = u32::try_from(pid) else { return false };
    let Ok(process) = open_process(pid) else { return false };
    let image = image_path(&process);
    let program = canonical(program).unwrap_or_else(|_| program.to_owned());
    !image.is_empty() && image.eq_ignore_ascii_case(&program.to_string_lossy())
}

/// The program's own file name: `.exe` on Windows.
fn program(name: &str) -> String {
    factoryd::home::program(name)
}

/// Started apart from claktory-qa, so its end (or a Ctrl-C meant for it)
/// doesn't end the sandbox: a process group of its own.
#[cfg(unix)]
fn spawn_detached(command: &mut Command) -> std::io::Result<std::process::Child> {
    use std::os::unix::process::CommandExt as _;
    command.process_group(0).spawn()
}

/// On Windows also with no console of its own, and without claktory-qa's
/// own output, which it would otherwise hold open for whatever reads that.
#[cfg(windows)]
fn spawn_detached(command: &mut Command) -> std::io::Result<std::process::Child> {
    use std::os::windows::process::CommandExt as _;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    factory_agent::windows::spawn::keeping_standard_handles(|| command.spawn())
}

/// Asks `pid`, with what it started, to stop.
#[cfg(unix)]
fn ask_to_end(pid: i32) {
    // SAFETY: kill(2) on a process group this sandbox started.
    unsafe { libc::kill(-pid, libc::SIGTERM) };
}

/// On Windows only factoryd can be asked, by its stop event; the others end at once.
#[cfg(windows)]
fn ask_to_end(pid: i32) {
    let asked = u32::try_from(pid).is_ok_and(|pid| factoryd::windows::stop::ask_to_stop(pid).is_ok());
    if !asked {
        end(pid);
    }
}

/// Ends `pid` and what it started, at once.
#[cfg(unix)]
fn end(pid: i32) {
    // SAFETY: kill(2) on a process group this sandbox started.
    unsafe { libc::kill(-pid, libc::SIGKILL) };
}

/// On Windows the process alone: the helpers factoryd starts end with it
/// (their job ends when it does), and the agents' commands, in jobs of
/// their own, are left as a restart would leave them.
#[cfg(windows)]
fn end(pid: i32) {
    if let Ok(pid) = u32::try_from(pid) {
        factory_agent::windows::system::terminate_process(pid);
    }
}

/// Ends a sandbox's processes, each with its process group (what factoryd
/// started), asking first and insisting after 10 s.
pub fn stop(sandbox: &Sandbox) {
    let processes = [
        (sandbox.pid, sandbox.factoryd()),
        (sandbox.account_pid.unwrap_or(0), sandbox.bin.join(program("claktory-account"))),
        (sandbox.model_pid.unwrap_or(0), sandbox.bin.join(program("claktory-qa"))),
    ];
    for (pid, program) in &processes {
        if alive(*pid, program) {
            ask_to_end(*pid);
        }
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while processes.iter().any(|(pid, program)| alive(*pid, program)) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(100));
    }
    for (pid, program) in &processes {
        if alive(*pid, program) {
            end(*pid);
        }
    }
}

/// What factoryd's environment always holds, beyond what any program
/// needs: its home and port, the machine name it shows (so no sandbox
/// shows this computer's), and with qa-model, OpenRouter pointed at it
/// with a key that opens nothing; with `--claude`, Anthropic too, for a QA
/// build of factoryd.
fn environment(sandbox: &Sandbox) -> Vec<(&'static str, String)> {
    let mut environment = vec![
        ("FACTORY_HOME", sandbox.home().display().to_string()),
        ("FACTORY_PORT", sandbox.port.to_string()),
        ("FACTORY_MACHINE_NAME", sandbox.machine_name.clone()),
    ];
    // Read only by a QA build of factoryd with qa-lies (E11 row 6).
    if let Some(email) = &sandbox.claim_email {
        environment.push(("FACTORY_QA_CLAIM_EMAIL", email.clone()));
    }
    if let Some(id) = &sandbox.claim_claktory_id {
        environment.push(("FACTORY_QA_CLAIM_CLAKTORY_ID", id.clone()));
    }
    if sandbox.speech {
        environment.push(("FACTORY_SPEECH", sandbox.bin.join(program("claktory-speech")).display().to_string()));
    }
    if let Some(port) = sandbox.model_port {
        environment.push(("OPENROUTER_API_KEY", "qa-model".to_owned()));
        environment.push(("OPENROUTER_BASE_URL", format!("http://127.0.0.1:{port}/v1")));
        // OpenRouter's model list from qa-model too: read only by a QA build
        // of factoryd, and only for an address on this computer (#185 U4).
        environment.push(("FACTORY_QA_OPENROUTER_MODELS_URL", format!("http://127.0.0.1:{port}/api/v1/models")));
        if sandbox.claude {
            // Read only by a QA build of factoryd, and only for an address on this computer.
            environment.push(("FACTORY_QA_ANTHROPIC_URL", format!("http://127.0.0.1:{port}")));
        }
    }
    environment
}

/// Ends the sandbox's factoryd and starts it again, as an update or a
/// restart of the computer would: asked to stop (and made to after 10 s),
/// or with `kill`, killed at once, as in a crash. Its account service and
/// qa-model stay up; its settings and data stay as they are.
pub fn restart(sandbox: &mut Sandbox, kill: bool) -> Result<(), Failure> {
    let factoryd = sandbox.factoryd();
    let pid = sandbox.pid;
    if alive(pid, &factoryd) {
        if kill {
            end(pid);
        } else {
            ask_to_end(pid);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while alive(pid, &factoryd) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(100));
        }
        if alive(pid, &factoryd) {
            end(pid);
            wait_for(Duration::from_secs(10), || !alive(pid, &factoryd), || true)
                .map_err(|why| Failure::failed(format!("factoryd didn't stop ({why}).")))?;
        }
    }
    launch(sandbox)
}

/// Ends a sandbox and, unless `keep`, deletes its folder.
pub fn down(sandbox: &Sandbox, keep: bool) -> Result<(), Failure> {
    stop(sandbox);
    if !keep {
        // The folder is the root's child, checked by `root`.
        std::fs::remove_dir_all(&sandbox.dir)
            .map_err(|error| Failure::failed(format!("Can't delete {}: {error}", sandbox.dir.display())))?;
    }
    Ok(())
}

/// `factoryd ARGS` against the sandbox, as a shell on its computer would
/// run it; refused for what reaches outside it.
pub fn cli(sandbox: &Sandbox, args: &[String]) -> Result<i32, Failure> {
    match args.first().map(String::as_str) {
        Some("login" | "provider") => {
            return Err(Failure::refused(
                "A sandbox has no real model: an agent in it would run with this computer's files. Script it instead (Theo's L1 rule).",
            ));
        }
        Some("supervise" | "--dev") | None => {
            return Err(Failure::usage("Give a factoryd command: status, account …, settings set …, pair …"));
        }
        _ => {}
    }
    if args.iter().any(|arg| arg == "--dev") {
        return Err(Failure::refused("--dev is the dev Claktory's. The sandbox's is chosen for you."));
    }
    if let Some(at) = args.iter().position(|arg| arg == "--service") {
        let url = args.get(at + 1).map_or("", String::as_str);
        crate::driver::check_local(url, "the account service")?;
    }
    let status = command(&sandbox.factoryd(), sandbox)
        .envs(environment(sandbox))
        .args(args)
        .status()
        .map_err(|error| Failure::failed(format!("Can't run factoryd: {error}")))?;
    Ok(status.code().unwrap_or(1))
}

/// The newest sign-in code the sandbox's account service emailed, to `to`
/// if given, with its address and file.
pub fn code(sandbox: &Sandbox, to: Option<&str>) -> Option<(String, String, PathBuf)> {
    let mut mails: Vec<PathBuf> = std::fs::read_dir(sandbox.mail()).ok()?.filter_map(|entry| Some(entry.ok()?.path())).collect();
    // Named by when they were written, and numbered: the newest sorts last.
    mails.sort();
    mails.into_iter().rev().find_map(|path| {
        let text = std::fs::read_to_string(&path).ok()?;
        let (address, code) = parse_mail(&text)?;
        if to.is_some_and(|to| !address.eq_ignore_ascii_case(to)) {
            return None;
        }
        Some((code, address, path))
    })
}

/// The recipient and code of a sign-in email.
fn parse_mail(text: &str) -> Option<(String, String)> {
    let header = |name: &str| text.lines().take_while(|line| !line.is_empty()).find_map(|line| line.strip_prefix(name)).map(str::trim);
    let to = header("To:")?.to_owned();
    let code = header("Subject:")?.rsplit(' ').next()?.to_owned();
    (code.len() == 6 && code.chars().all(|character| character.is_ascii_digit())).then_some((to, code))
}

/// The number of emails the service has written.
pub fn mail_count(sandbox: &Sandbox) -> usize {
    std::fs::read_dir(sandbox.mail()).map(Iterator::count).unwrap_or(0)
}

/// `export` lines for a shell with factoryd's whole environment, so its own
/// commands reach the sandbox, and a factoryd started by hand has its model.
pub fn env_lines(sandbox: &Sandbox) -> String {
    let mut out = Vec::new();
    for (key, value) in environment(sandbox) {
        let _ = writeln!(out, "export {key}='{value}'");
    }
    String::from_utf8(out).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sandbox_is_owned_by_the_agent_whose_checkout_made_it() {
        assert_eq!(owner_of(Path::new("/Users/me/.factory/trees/backend-2/factory")), "backend-2");
        assert_eq!(owner_of(Path::new("/Users/me/.factory/trees/qa/factory/crates")), "qa");
        assert_eq!(owner_of(Path::new("/Users/me/factory")), "/Users/me/factory", "the main checkout is its own");
    }

    #[test]
    fn real_folders_ports_and_broad_roots_are_refused() {
        let home = std::env::home_dir().expect("home");
        let real = factoryd::home::instance_root(false).expect("the real Claktory's folder");
        let dev = factoryd::home::instance_root(true).expect("the dev Claktory's folder");
        for real in [real.clone(), real.join("releases/v0.1.13"), dev] {
            assert!(check_root(&real).is_err(), "{}", real.display());
            assert!(check_binaries(&real.join("bin")).is_err(), "{}", real.display());
        }
        #[cfg(windows)]
        assert!(check_binaries(&factoryd::windows::dirs::programs().expect("programs")).is_err(), "the installed app's folder");
        // Agents' own folders and checkouts are theirs.
        assert!(check_root(&real.join("agents/qa/scratch")).is_ok());
        assert!(check_binaries(&real.join("trees/qa/factory/target/debug")).is_ok());
        assert!(check_root(&home).is_err());
        assert!(check_root(Path::new("/")).is_err());
        assert!(check_root(&std::env::temp_dir().join("claktory-qa")).is_ok());
        assert!(check_port(7420).is_err() && check_port(7421).is_err() && check_port(80).is_err());
        assert!(check_port(7500).is_ok());
        assert!(check_name("qa1").is_ok() && check_name("pipes-a").is_ok());
        assert!(check_name("../x").is_err() && check_name("").is_err() && check_name("QA").is_err());
    }

    #[test]
    fn a_process_is_alive_only_while_it_runs_the_program_it_was_started_as() {
        let sleep =
            canonical(Path::new(if cfg!(windows) { "C:\\Windows\\System32\\PING.EXE" } else { "/bin/sleep" })).expect("the program");
        let mut child = Command::new(&sleep)
            .args(if cfg!(windows) { ["-n", "30", "127.0.0.1"].as_slice() } else { ["30"].as_slice() })
            .stdout(Stdio::null())
            .spawn()
            .expect("started");
        let pid = i32::try_from(child.id()).expect("pid");
        assert!(alive(pid, &sleep), "it runs");
        assert!(!alive(pid, &std::env::current_exe().expect("this test")), "it isn't another program");
        child.kill().expect("killed");
        child.wait().expect("ended");
        // On Windows a process lasts while a handle to it is open.
        drop(child);
        assert!(!alive(pid, &sleep), "it ended");
        assert!(!alive(0, &sleep) && !alive(-1, &sleep));
    }

    #[test]
    fn a_code_is_read_from_its_email() {
        let mail =
            "From: Claktory <code@auth.claktory.com>\nTo: qa@example.test\nSubject: Your Claktory code: 162531\n\nYour code:\n\n162531\n";
        assert_eq!(parse_mail(mail), Some(("qa@example.test".to_owned(), "162531".to_owned())));
        assert_eq!(parse_mail("To: a@b\nSubject: Welcome\n\nHi"), None);
    }
}
