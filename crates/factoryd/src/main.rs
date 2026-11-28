//! `factoryd [--dev]` serves a Factory; `factoryd [--dev] supervise` keeps
//! one running and undoes updates that don't start (see `update`);
//! `factoryd [--dev] login [PROVIDER]` signs it in to a model provider;
//! `factoryd [--dev] pair NAME` pairs a device on another machine: prints
//! its pairing code and, in a terminal, asks whether to allow the device
//! that uses it; `allow DEVICE` and `deny DEVICE` answer a device waiting to
//! be allowed, `unpair DEVICE` unpairs one and `devices` lists them; remote
//! access is on while any is paired (see `remote`).
//!
//! `FACTORY_HOME` and `FACTORY_PORT` override where the Factory lives and
//! which port it serves, for tests and side-by-side Factories.

use std::io::IsTerminal;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

use factoryd::Factory;
use factoryd::factory::OpenError;
use factoryd::home::{self, Home, Instance};
use factoryd::models::Registry;
use factoryd::update;

/// How long the supervisor gives the backend to stop on SIGTERM.
const STOP_WITHIN: Duration = Duration::from_secs(10);
/// Quick failures in a row, outside an update's trial, after which the
/// supervisor puts the previous backend back.
const FAILURES_TO_FALL_BACK: u32 = 5;
#[cfg(unix)]
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const USAGE: &str = factoryd::cli::USAGE;

#[derive(Debug, thiserror::Error)]
enum MainError {
    #[error("{USAGE}")]
    Usage,
    #[error("can't find your home directory")]
    NoHome,
    #[cfg(windows)]
    #[error("Couldn't prepare Claktory's folder: {0}")]
    Windows(std::io::Error),
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error("could not listen on {addr}: {source}")]
    Bind { addr: SocketAddr, source: std::io::Error },
    /// The port is taken, most likely by another user's Claktory on this
    /// computer: said plainly, since it's what the person can act on.
    #[cfg(unix)]
    #[error(
        "Another Claktory on this computer already uses port {0} (another user's, say), so this one can't start. Stop that one, or start this one on another port with FACTORY_PORT."
    )]
    PortInUse(u16),
    /// On Windows: the port is held, by whom when Windows says
    /// (`windows::listener::taken_by`).
    #[cfg(windows)]
    #[error("{0}")]
    PortTaken(String),
    #[error("login failed: {0}")]
    Login(#[from] factory_agent::Error),
    #[error("FACTORY_PORT must be a port number: {0}")]
    Port(#[from] std::num::ParseIntError),
    #[error("could not listen for signals: {0}")]
    Signal(std::io::Error),
    #[error(transparent)]
    Remote(#[from] factoryd::remote::RemoteError),
    #[error(transparent)]
    Import(#[from] factoryd::move_import::ImportError),
    /// An import into this home hasn't finished: it doesn't start half made.
    #[error("{0}")]
    ImportUnfinished(String),
    /// QA builds only: FACTORY_QA_ANTHROPIC_URL that the guard refuses.
    #[cfg(feature = "qa-provider")]
    #[error("{0}")]
    QaProvider(String),
    /// QA builds only: FACTORY_QA_CLAIM_* that the guard refuses.
    #[cfg(feature = "qa-lies")]
    #[error("{0}")]
    QaLies(String),
}

fn main() -> ExitCode {
    // Claktory's browser gate, before anything else: it runs as the
    // browser's user for whoever can use its sudo line, so it reads no
    // settings, environment or logs before it has checked factoryd's answer.
    if std::env::args_os().nth(1).is_some_and(|arg| arg == "browser-gate") {
        return factoryd::browser_runner::gate::main(&std::env::args_os().collect::<Vec<_>>());
    }
    // On Windows, before anything else (logs included) could start a
    // process: the vault's handover pipes, if the supervisor gave this
    // backend any, no longer inheritable.
    #[cfg(windows)]
    factoryd::windows::handoff::seal();
    // A command's log filter (see factory_agent::secrets::filter_main): one
    // per command with secrets, so it starts lean, without a runtime or logs.
    if std::env::args().nth(1).as_deref() == Some("redact") {
        return factory_agent::secrets::filter_main();
    }
    // Claktory's browser runner (#435): its stdout is the wire to factoryd,
    // and it makes its own namespaces while it's still a single thread.
    if std::env::args().nth(1).as_deref() == Some("browser-runner") {
        let args: Vec<String> = std::env::args().skip(2).collect();
        return factoryd::browser_runner::main(&args);
    }
    // `factoryd install` and `uninstall`, run by the Mac app: their answer
    // is one JSON line on stdout, so no logs there.
    let args: Vec<String> = std::env::args().skip(1).collect();
    // What the supervisor in this binary does, as a number, for updates to
    // tell whether the installed one needs replacing (see
    // update::SUPERVISOR_VERSION).
    if args.iter().map(String::as_str).filter(|arg| *arg != "--dev").eq(["supervisor-version"]) {
        println!("{}", update::SUPERVISOR_VERSION);
        return ExitCode::SUCCESS;
    }
    if let Some(at) = args.iter().position(|arg| arg != "--dev")
        && (args[at] == "install" || args[at] == "uninstall")
    {
        let mut flags: Vec<String> = args[..at].to_vec();
        flags.extend_from_slice(&args[at + 1..]);
        #[cfg(windows)]
        return if args[at] == "install" {
            factoryd::windows::install::install_main(&flags)
        } else {
            factoryd::windows::install::uninstall_main(&flags)
        };
        #[cfg(not(windows))]
        return if args[at] == "install" { factoryd::install::main(&flags) } else { factoryd::uninstall::main(&flags) };
    }
    // Moving isn't on Windows yet: its commands say so, and do nothing.
    #[cfg(windows)]
    if let Some(at) = args.iter().position(|arg| arg != "--dev")
        && matches!(args[at].as_str(), "export" | "move" | "import" | "receive")
    {
        eprintln!("{}", factoryd::move_run::NOT_ON_WINDOWS);
        return ExitCode::from(2);
    }
    // Moving a Claktory to another computer (#217): no logs on stdout,
    // which says what happened (one JSON line with --json).
    if let Some(at) = args.iter().position(|arg| arg != "--dev")
        && (args[at] == "export" || args[at] == "move")
    {
        let dev = args.iter().any(|arg| arg == "--dev");
        let rest: Vec<String> = args[at + 1..].iter().filter(|arg| *arg != "--dev").cloned().collect();
        return if args[at] == "export" { factoryd::move_export::main(dev, &rest) } else { factoryd::move_source::main(dev, &rest) };
    }
    // Backing the vault up off this computer, and restoring it: the backup
    // goes to stdout, so no logs there.
    if let Some(at) = args.iter().position(|arg| arg != "--dev")
        && args[at] == "vault"
    {
        let dev = args.iter().any(|arg| arg == "--dev");
        let rest: Vec<String> = args[at + 1..].iter().filter(|arg| *arg != "--dev").cloned().collect();
        return factoryd::vault::backup::main(dev, &rest);
    }
    // The commands for setting Claktory up from a shell, with --json: they
    // talk to the running backend, and print one JSON line, so no logs.
    if let Some(code) = factoryd::cli::dispatch(&args) {
        return code;
    }
    serve_main()
}

#[tokio::main]
async fn serve_main() -> ExitCode {
    // One crypto provider for TLS code that asks rustls for the process default:
    // with two in the build (ring for the providers and the tunnel, aws-lc-rs
    // through other crates) there's none, and rustls panics. Factory's own clients name theirs.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    // Colors only for a terminal; log files stay plain text. `receive
    // --json` keeps stdout for its JSON lines: its logs go to stderr.
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|arg| arg == "receive") && args.iter().any(|arg| arg == "--json") {
        tracing_subscriber::fmt().with_env_filter(filter).with_ansi(false).with_writer(std::io::stderr).init();
    } else {
        // A line logged over and over is counted rather than written each
        // time (factoryd::log_file).
        tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_ansi(std::io::stdout().is_terminal())
            .with_writer(factoryd::log_file::Quieted)
            .init();
    }
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            tracing::error!(%error, "factoryd stopped");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> Result<(), MainError> {
    let mut dev = false;
    let mut command = Vec::new();
    for arg in std::env::args().skip(1) {
        if arg == "--dev" {
            dev = true;
        } else {
            command.push(arg);
        }
    }
    let mut instance = home::instance_in(dev, home::instance_root(dev).ok_or(MainError::NoHome)?);
    if let Some(root) = std::env::var_os("FACTORY_HOME") {
        instance.home = Home::new(root.into());
    }
    // Private before anything is written into it, as it's 0700 on a Mac
    // (Fugo 45257, condition 1): `windows::prepare_home`, for every
    // command, never a fallback.
    #[cfg(windows)]
    factoryd::windows::prepare_home(instance.home.root()).map_err(MainError::Windows)?;
    if let Ok(port) = std::env::var("FACTORY_PORT") {
        instance.addr.set_port(port.parse()?);
    }
    // Browser control stays out of sight until its security proof (B6a):
    // only a Claktory started with this, the dev one say, has it. No
    // installer, LaunchAgent or release script ever sets it.
    factoryd::browser::set_preview(std::env::var_os("FACTORY_BROWSER_PREVIEW").is_some_and(|on| on == "1"));
    // A Dev Claktory's app socket takes the Dev app pinned by its
    // cdhash; a real one, only the Developer ID app.
    factoryd::authority::set_dev(dev);
    // So do agents in direct messages, until People ships: the same rule.
    factoryd::user_apps::dms::set_preview(std::env::var_os("FACTORY_DMS_PREVIEW").is_some_and(|on| on == "1"));
    match command.as_slice() {
        [] => {
            // It holds the vault's key: no core dumps, and on Linux no other
            // process of this user reads its memory.
            factoryd::vault::memory::harden();
            // launchd's 256 open files, or systemd's 1024, ran out under
            // load: as many as the system allows, for agents' commands too.
            factoryd::open_files::raise();
            // Before anything starts: the open vault's key from the backend
            // before this one, at an update.
            let handed = factoryd::vault::handoff::receive();
            if let Ok(factoryd) = std::env::current_exe() {
                factoryd::set_filter_executable(factoryd);
            }
            let home = instance.home.clone();
            // On Windows, before any command runs: the private home and its
            // records folder, and the commands' jobs an earlier backend left,
            // found again (`windows::start`, prepare_home first).
            #[cfg(windows)]
            factoryd::windows::start(home.root()).map_err(MainError::Windows)?;
            let served = serve(instance, handed).await;
            if let Err(error) = &served {
                update::trial_failed(&home, error);
            }
            served
        }
        [supervise] if supervise == "supervise" => supervise_backend(instance.home, dev).await,
        [login] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), None).await?),
        [login, provider] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), Some(provider)).await?),
        // `pair --show-code` (`cli` has every other `pair`): the installer's
        // one code for a new install's first device, at its own terminal,
        // before any agent could run a command.
        [pair, words @ ..] if pair == "pair" => {
            let mut name: Vec<&str> = Vec::new();
            let mut words = words.iter();
            while let Some(word) = words.next() {
                match word.as_str() {
                    "--show-code" | "--dev" => {}
                    // From before the private tunnel: devices find this
                    // machine by its key now.
                    "--host" => drop(words.next()),
                    word => name.push(word),
                }
            }
            if std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty").is_err() {
                eprintln!("factoryd pair --show-code shows a code only at a terminal. Without one, run factoryd account authorize --email <your email>, then approve this Claktory on one of your devices.");
                std::process::exit(3);
            }
            if let Some(why) = factoryd::remote::first_code_refused(&instance.home) {
                eprintln!("{why}");
                std::process::exit(3);
            }
            let (code, device) = factoryd::remote::pair_offline(&instance.home, &name.join(" "))?;
            let minutes = factoryd::remote::CODE_LIFETIME.as_secs() / 60;
            // A terminal gets the code as a QR code too, to scan from the
            // screen; piped elsewhere, just the text.
            use std::io::IsTerminal;
            // The QR code holds the code as a link, once links are on: a
            // phone's camera opens it in Claktory, or in the store without it.
            let link = factoryd::remote::pairing_link(&code);
            let scanned = if link.is_empty() { &code } else { &link };
            let qr = if std::io::stdout().is_terminal() { factoryd::terminal_qr::lines(scanned) } else { None };
            match qr {
                Some(lines) => {
                    println!("Scan this with Claktory on your phone or iPad:\n");
                    for line in lines {
                        println!("  {line}");
                    }
                    println!(
                        "\nOr, on a Mac, click Add a Claktory (+) at the far left of Claktory's window and paste this code:\n\n{code}\n"
                    );
                }
                None => println!(
                    "Paste this code into Claktory on your other device (on a Mac: Add a Claktory, the + at the far left of the window):\n\n{code}\n"
                ),
            }
            // What the device shows before it connects, to compare (MC7).
            println!("The device you connect shows this too: {}\n", factoryd::remote::code_fingerprint(&instance.home));
            println!("It works once, for {minutes} minutes. You'll be asked to allow the device that uses it.");
            allow_in_terminal(&instance.home, &device).await
        }
        [command, rest @ ..] if command == "import" => import(&instance, rest),
        [command, rest @ ..] if command == "receive" => receive(&instance, rest).await,
        // Never a waiting device's number (#352 P4, Fugo): the person reads
        // it off the device's own screen. Names as `shown_name` shows them,
        // since a device chooses its own (no escape sequences reach the
        // terminal).
        [devices] if devices == "devices" => {
            for device in factoryd::remote::devices(&instance.home)?.into_iter().filter(|device| !device.local) {
                let name = factoryd::remote::shown_name(&device.name);
                if device.is_pending(now_ms()) {
                    println!("{}  {}", device.id, factoryd::cli::waiting_sentence(&device.id, &name));
                    continue;
                }
                let seen = match device.last_seen.map(jiff::Timestamp::from_millisecond) {
                    Some(Ok(seen)) => format!("last connected {seen}"),
                    _ => "never connected".to_owned(),
                };
                println!("{}  {name}  {seen}", device.id);
            }
            Ok(())
        }
        // From before the private tunnel, which needs no switch.
        [remote, ..] if remote == "remote" => {
            println!("Other devices can connect once you've added one: run factoryd pair NAME.");
            Ok(())
        }
        _ => Err(MainError::Usage),
    }
}

/// `factoryd import BUNDLE [--map FROM=TO]… [--passphrase-stdin]`, or
/// `factoryd import --abandon`.
fn import(instance: &Instance, args: &[String]) -> Result<(), MainError> {
    use factoryd::move_import::{ImportError, Request};
    let mut request = Request { bundle: PathBuf::new(), passphrase: None, map: Vec::new(), same_identity: false };
    let (mut abandon, mut stdin) = (false, false);
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!(
                    "usage: factoryd import BUNDLE [--map FROM=TO]… [--passphrase-stdin] [--same-identity] | factoryd import --abandon"
                );
                return Ok(());
            }
            "--abandon" => abandon = true,
            "--passphrase-stdin" => stdin = true,
            "--same-identity" => request.same_identity = true,
            "--map" => {
                let pair = args.next().ok_or_else(|| ImportError::Refused("--map takes FROM=TO".to_owned()))?;
                request.map.push(factoryd::move_import::parse_pair(pair).map_err(ImportError::Refused)?);
            }
            flag if flag.starts_with('-') => return Err(ImportError::Refused(format!("{flag} isn't an option of import")).into()),
            bundle if request.bundle.as_os_str().is_empty() => request.bundle = PathBuf::from(bundle),
            _ => return Err(MainError::Usage),
        }
    }
    if !abandon && request.bundle.as_os_str().is_empty() {
        return Err(MainError::Usage);
    }
    let done = if abandon {
        factoryd::move_import::abandon(&instance.home)
    } else {
        // A folder is a bundle already unpacked; a file needs its passphrase.
        (|| {
            if factoryd::move_import::needs_passphrase(&instance.home, &request.bundle) {
                request.passphrase = Some(if stdin {
                    let mut line = String::new();
                    std::io::stdin()
                        .read_line(&mut line)
                        .map_err(|error| ImportError::Failed(format!("Can't read the passphrase: {error}")))?;
                    line.trim().to_owned()
                } else {
                    factoryd::move_import::ask_passphrase().map_err(ImportError::Refused)?
                });
            }
            factoryd::move_import::import(&instance.home, instance.addr, &request).map(|imported| imported.text)
        })()
    };
    match done {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        // Said plainly, for the person or the installer: 3 when it's
        // refused (nothing to fix but what it says), 1 when it failed.
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code().into());
        }
    }
}

/// `factoryd receive [--committed] [--json]`: shows a code, and takes in the
/// Claktory a source moves here with it (docs/MOVE.md, "The handshake").
/// With --json, for an agent: one JSON line per event (`cli::receive_json`).
async fn receive(instance: &Instance, args: &[String]) -> Result<(), MainError> {
    use factoryd::cli::receive_json;
    use factoryd::move_import::ImportError;
    use factoryd::move_import::receive::{self, Options, Ui};
    let mut options = Options::default();
    let json = args.iter().any(|arg| arg == "--json");
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("usage: factoryd receive [--email EMAIL [--service URL]] [--committed] [--json]");
                return Ok(());
            }
            "--committed" => options.committed = true,
            "--json" => {}
            // A move with the account: no code, no digits.
            "--email" | "--service" if args.len() > 0 => {
                let value = args.next().cloned();
                if arg == "--email" { options.email = value } else { options.service = value }
            }
            other => {
                let refused = ImportError::Refused(format!("{other} isn't an option of receive"));
                if json {
                    println!("{}", receive_json::end(&Err(refused)).0);
                    std::process::exit(3);
                }
                return Err(refused.into());
            }
        }
    }
    // The person's answers, a line at a time; lines typed when nothing was
    // asked are dropped before asking.
    let (typed, lines) = tokio::sync::mpsc::unbounded_channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            if typed.send(line).is_err() {
                break;
            }
        }
    });
    let lines = Arc::new(tokio::sync::Mutex::new(lines));
    let terminal = std::io::stdout().is_terminal();
    let ui = if json {
        receive_json::ui(Arc::new(|line| println!("{line}")), lines)
    } else {
        Ui {
            code: Arc::new(move |code| {
                // A move with the account shows no code.
                if code.secret.is_empty() {
                    return;
                }
                let text = code.text();
                if let Some(lines) = terminal.then(|| factoryd::terminal_qr::lines(&text)).flatten() {
                    println!("\nScan this with Claktory on your phone or iPad, or paste the code below:\n");
                    for line in lines {
                        println!("  {line}");
                    }
                }
                println!(
                    "\nOn the computer you're moving from, open Claktory and choose Move to another computer… (Settings → General), then paste this code:\n\n{text}\n\nKeep this running until the move is done."
                );
            }),
            say: Arc::new(|text| println!("{text}")),
            confirm: Arc::new(move |asked| {
                let lines = lines.clone();
                Box::pin(async move {
                    println!(
                        "\n{} ({}) wants to move its Claktory here. Check that it shows these digits:\n\n    {}\n\nType yes if they match, or no:",
                        asked.machine, asked.platform, asked.digits
                    );
                    let mut lines = lines.lock().await;
                    while lines.try_recv().is_ok() {}
                    loop {
                        match lines.recv().await.map(|line| line.trim().to_lowercase()) {
                            Some(answer) if answer == "yes" || answer == "y" => return true,
                            Some(answer) if answer == "no" || answer == "n" => return false,
                            Some(_) => println!("Type yes or no:"),
                            None => return false,
                        }
                    }
                })
            }),
        }
    };
    // Stopped by Ctrl-C, or by a TERM signal (an agent stopping its task):
    // a code no source has is forgotten, and running it again carries on.
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(MainError::Signal)?;
    // On Windows: the stop event, Ctrl-Break, the console closing, signing out.
    #[cfg(windows)]
    let mut terminate = factoryd::windows::stop::listen().map_err(MainError::Signal)?;
    let done = tokio::select! {
        done = receive::receive(&instance.home, instance.addr, &options, &ui) => done,
        _ = tokio::signal::ctrl_c() => stop_receiving(&instance.home, json),
        _ = terminate.recv() => stop_receiving(&instance.home, json),
    };
    if json {
        let (line, code) = receive_json::end(&done);
        println!("{line}");
        std::process::exit(code.into());
    }
    match done {
        Ok(text) => {
            println!("{text}");
            Ok(())
        }
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(error.exit_code().into());
        }
    }
}

/// `factoryd receive` stopped by the person or a signal: says so, and exits 130.
fn stop_receiving(home: &Home, json: bool) -> ! {
    factoryd::move_import::receive::forget_unclaimed(home);
    if json {
        println!("{}", factoryd::cli::receive_json::stopped());
    } else {
        println!("\nStopped. Run factoryd receive again to carry on.");
    }
    std::process::exit(130);
}

fn now_ms() -> i64 {
    jiff::Timestamp::now().as_millisecond()
}

/// After `factoryd pair --show-code` prints a code in a terminal: waits for
/// the device that uses it, and asks whether to allow it, with the number it
/// shows. Ctrl-C ends the code. With no terminal it returns at once: the
/// device is allowed in the app.
async fn allow_in_terminal(home: &Home, device: &factoryd::remote::Device) -> Result<(), MainError> {
    use factoryd::remote;
    use std::io::{BufRead, Write};
    let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        // Over SSH without -t, say (Matt's case): nothing here can ask for
        // the digits, so it says how to be asked.
        println!(
            "This terminal can't ask for the 4 digits {} shows. Allow it in Settings → Devices on a device connected to this Claktory, or, over SSH, connect with ssh -t and run factoryd pair --show-code again.",
            device.id
        );
        return Ok(());
    };
    let claktory = remote::secret_key(home)?.public().to_string();
    println!("\nWaiting for a device to use the code… (Ctrl-C cancels it)");
    loop {
        let now = now_ms();
        match remote::devices(home)?.into_iter().find(|known| known.id == device.id) {
            Some(known) if !known.endpoint_id.is_empty() => {
                println!("Allowed in Claktory. {} can connect now.", remote::shown_name(&known.name));
                return Ok(());
            }
            Some(known) if known.is_pending(now) => {
                // The person types the digits the device's own screen shows
                // (#352 P4, Fugo): never printed here. Three wrong, or none,
                // and it's turned away.
                let number = remote::approval_number(&claktory, &known.pending_endpoint_id, &known.token_sha256);
                let name = remote::shown_name(&known.name);
                println!("\nA device calling itself \"{name}\" wants to connect. Allow it only if its screen shows the number you type.");
                let mut allowed = Ok(false);
                for tries_left in (0..3).rev() {
                    print!("Type the 4 digits on its screen (Enter to refuse): ");
                    std::io::stdout().flush().map_err(remote::RemoteError::from)?;
                    let reading = tty.try_clone().map_err(remote::RemoteError::from)?;
                    let answer = tokio::task::spawn_blocking(move || {
                        let mut line = String::new();
                        std::io::BufReader::new(reading).read_line(&mut line).map(|_| line)
                    });
                    let answer = tokio::select! {
                        answer = answer => answer.map_err(std::io::Error::other).and_then(|line| line).map_err(remote::RemoteError::from)?,
                        _ = tokio::signal::ctrl_c() => {
                            let _ = remote::cancel(home, &device.id);
                            println!("\nCancelled. The code no longer works.");
                            std::process::exit(130);
                        }
                    };
                    let typed = answer.trim();
                    if typed == number {
                        allowed = remote::approve(home, &device.id).map(|_| true);
                        break;
                    }
                    if typed.is_empty() || tries_left == 0 {
                        allowed = remote::deny(home, &device.id).map(|_| false);
                        break;
                    }
                    let tries = if tries_left == 1 { "1 try left".to_owned() } else { format!("{tries_left} tries left") };
                    println!("That's not the number its screen shows. {tries}.");
                }

                match allowed {
                    Ok(true) => println!("Allowed. {name} can connect now."),
                    Ok(false) => println!("Not allowed. To try again, run factoryd pair --show-code."),
                    Err(_) => println!("The code expired or was cancelled. Run factoryd pair --show-code for a new one."),
                }
                return Ok(());
            }
            Some(known) if known.code_expires.is_some_and(|expires| expires > now) => {}
            _ => {
                let _ = remote::cancel(home, &device.id);
                println!("The code expired or was cancelled. Run factoryd pair --show-code for a new one.");
                return Ok(());
            }
        }
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(500)) => {}
            _ = tokio::signal::ctrl_c() => {
                let _ = remote::cancel(home, &device.id);
                println!("\nCancelled. The code no longer works.");
                return Ok(());
            }
        }
    }
}

/// Runs the backend at `<home>/bin/factoryd` and keeps it running: an exit
/// that hands over to an update starts the new one right away, a crash starts
/// it again after a pause, and an update that dies before passing its trial is
/// undone. Stops it on SIGTERM or SIGINT. A change to what it does needs
/// update::SUPERVISOR_VERSION bumped, or installed Claktories keep the old.
/// On Windows it stops on its stop event or a console's (`windows::stop`),
/// and hands the vault's key over through a pair of anonymous pipes only
/// the backend inherits (`windows::handoff`); its backend there is
/// `bin\factoryd.exe` (`home::program`).
async fn supervise_backend(home: Home, dev: bool) -> Result<(), MainError> {
    #[cfg(unix)]
    use tokio::signal::unix::{SignalKind, signal};
    #[cfg(unix)]
    let mut terminate = signal(SignalKind::terminate()).map_err(MainError::Signal)?;
    #[cfg(unix)]
    let mut interrupt = signal(SignalKind::interrupt()).map_err(MainError::Signal)?;
    #[cfg(windows)]
    let mut terminate = factoryd::windows::stop::listen().map_err(MainError::Signal)?;
    let mut pause = Duration::from_millis(500);
    let mut failures = 0;
    // The sign-in task gives it no output anyone reads.
    #[cfg(windows)]
    if let Err(error) = factoryd::windows::log::to_log(&home.root().join("factoryd.log")) {
        tracing::warn!(%error, "could not open factoryd.log");
    }
    tracing::info!(backend = %home.backend().display(), version = update::VERSION, "supervising");
    // It may hold the vault's key between two backends.
    factoryd::vault::memory::harden();
    // The backends it starts inherit it. Each backend raises its own as it
    // starts too, so a supervisor installed before this one (which doesn't)
    // needs no replacing: no SUPERVISOR_VERSION bump for it.
    factoryd::open_files::raise();
    let mut holder = factoryd::vault::handoff::Holder::default();
    loop {
        if let Err(error) = update::start_trial(&home) {
            tracing::error!(%error, "could not start the update's trial");
        }
        let started = Instant::now();
        // On Windows the backend is started with the attribute list that
        // hands it only its own ends of the vault's handover
        // (`windows::handoff::spawn`), not by tokio's Command.
        #[cfg(windows)]
        let spawned = {
            let args: Vec<std::ffi::OsString> = if dev { vec!["--dev".into()] } else { Vec::new() };
            let held = holder.for_next(update::on_trial(&home));
            factoryd::windows::handoff::spawn(
                &home.backend(),
                &args,
                &[(update::SUPERVISED, "1")],
                held.as_deref().map(|held| held.as_slice()),
            )
        };
        #[cfg(unix)]
        let mut command = tokio::process::Command::new(home.backend());
        #[cfg(unix)]
        if dev {
            command.arg("--dev");
        }
        #[cfg(unix)]
        command.env(update::SUPERVISED, "1").kill_on_drop(true);
        // The vault's key goes from one backend to the next through a pair.
        #[cfg(unix)]
        let channel = match holder.channel(update::on_trial(&home)) {
            Ok((ours, theirs)) => {
                command.env(factoryd::vault::handoff::FD_ENV, std::os::fd::AsRawFd::as_raw_fd(&theirs).to_string());
                Some((ours, theirs))
            }
            Err(error) => {
                tracing::warn!(%error, "could not make the vault's handover; an update locks it");
                None
            }
        };
        #[cfg(unix)]
        let spawned = command.spawn();
        // Only the backend holds its end now, so the pair ends with it.
        #[cfg(unix)]
        let mut ours = channel.and_then(|(ours, theirs)| {
            drop(theirs);
            ours.set_nonblocking(true).ok()?;
            tokio::net::UnixStream::from_std(ours).ok()
        });
        let ended = match spawned {
            #[cfg(windows)]
            Ok(factoryd::windows::handoff::Started { mut child, mut written }) => {
                let status = loop {
                    tokio::select! {
                        status = child.wait() => break status,
                        _ = terminate.recv() => return stop(child).await,
                        bytes = async { match written.as_mut() { Some(written) => written.recv().await, None => std::future::pending().await } } => match bytes {
                            Some(bytes) => holder.read(&bytes),
                            None => written = None,
                        },
                    }
                };
                // What it wrote just before it exited.
                if let Some(written) = written.as_mut() {
                    while let Ok(Some(bytes)) = tokio::time::timeout(Duration::from_millis(200), written.recv()).await {
                        holder.read(&bytes);
                    }
                }
                match status {
                    Ok(status) if status.success() && update::on_trial(&home) => None,
                    Ok(status) if status.success() => Some("It stopped by itself.".to_owned()),
                    Ok(status) => Some(format!("It stopped ({status}).")),
                    Err(error) => Some(format!("It stopped: {error}.")),
                }
            }
            #[cfg(unix)]
            Ok(mut child) => {
                let mut buffer = zeroize::Zeroizing::new([0u8; 256]);
                let status = loop {
                    tokio::select! {
                        status = child.wait() => break status,
                        _ = terminate.recv() => return stop(child).await,
                        _ = interrupt.recv() => return stop(child).await,
                        read = async { match ours.as_mut() { Some(ours) => ours.read(buffer.as_mut_slice()).await, None => std::future::pending().await } } => match read {
                            Ok(0) | Err(_) => ours = None,
                            Ok(read) => holder.read(&buffer[..read]),
                        },
                    }
                };
                // What it wrote just before it exited.
                if let Some(ours) = ours.as_mut() {
                    while let Ok(Ok(read)) = tokio::time::timeout(Duration::from_millis(200), ours.read(buffer.as_mut_slice())).await {
                        if read == 0 {
                            break;
                        }
                        holder.read(&buffer[..read]);
                    }
                }
                match status {
                    // Handing over to an update leaves its trial behind;
                    // exiting cleanly without one (a backend a power cut
                    // left empty runs as an empty script) counts as stopping.
                    Ok(status) if status.success() && update::on_trial(&home) => None,
                    Ok(status) if status.success() => Some("It stopped by itself.".to_owned()),
                    Ok(status) => Some(format!("It stopped ({status}).")),
                    Err(error) => Some(format!("It stopped: {error}.")),
                }
            }
            Err(error) => Some(format!("It couldn't be started: {error}.")),
        };
        // A disk that can't be written (friend bug #36) isn't the backend's
        // fault: going back to an earlier one would only fail there too, and
        // write over it once the disk works. It's started again as it is.
        let disk = ended.as_ref().and_then(|_| factoryd::disk::check(home.root()).err()).and_then(|error| factoryd::disk::fault_of(&error));
        if let Some(fault) = disk {
            tracing::error!(?fault, "the disk can't be written; not going back to an earlier backend");
        }
        if disk.is_none() && update::update_failed(&home) {
            match update::roll_back(&home, ended.as_deref().unwrap_or("It stopped.")) {
                Ok(()) => continue,
                Err(error) => tracing::error!(%error, "could not go back to the previous backend"),
            }
        }
        failures = match &ended {
            Some(_) if started.elapsed() <= Duration::from_secs(60) => failures + 1,
            _ => 0,
        };
        // Broken outside a trial (a power cut during an update, say): the
        // one before, once.
        if failures >= FAILURES_TO_FALL_BACK
            && disk.is_none()
            && let Some(how) = &ended
            && update::fall_back(&home, how)
        {
            failures = 0;
            continue;
        }
        match ended {
            // Exited on purpose, to hand over to an update.
            None => pause = Duration::from_millis(500),
            Some(how) => {
                tracing::error!(%how, "backend stopped; starting it again");
                // Crashing over and over: wait longer each time, up to 10s.
                pause = if started.elapsed() > Duration::from_secs(60) {
                    Duration::from_millis(500)
                } else {
                    (pause * 2).min(Duration::from_secs(10))
                };
                tokio::time::sleep(pause).await;
            }
        }
    }
}

/// Stops the backend: SIGTERM, so it can finish what it's writing, then
/// SIGKILL if it hasn't stopped within STOP_WITHIN (launchd allows the
/// supervisor 20 s).
/// On Windows its stop event stands for SIGTERM, and TerminateProcess for
/// SIGKILL.
async fn stop(mut child: Backend) -> Result<(), MainError> {
    if let Some(pid) = child.id() {
        // SAFETY: kill only sends a signal to the backend, our child.
        #[cfg(unix)]
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        #[cfg(unix)]
        let asked = true;
        // A backend that has no stop event yet is stopped at once, as one
        // sent SIGTERM before its handler is.
        #[cfg(windows)]
        let asked = factoryd::windows::stop::ask_to_stop(pid)
            .inspect_err(|error| tracing::warn!(%error, "could not ask the backend to stop; stopping it now"))
            .is_ok();
        if asked && tokio::time::timeout(STOP_WITHIN, child.wait()).await.is_ok() {
            return Ok(());
        }
        tracing::warn!("the backend didn't stop within {}s; killing it", STOP_WITHIN.as_secs());
    }
    if let Err(error) = child.kill().await {
        tracing::warn!(%error, "could not stop the backend");
    }
    Ok(())
}

/// The backend the supervisor started.
#[cfg(unix)]
type Backend = tokio::process::Child;
#[cfg(windows)]
type Backend = factory_agent::windows::child::Child;

/// The models and sign-ins factoryd uses. Built for QA (the `qa-provider`
/// feature, claktory-qa's sandboxes only), FACTORY_QA_ANTHROPIC_URL points
/// the Anthropic subscription at qa-model on this computer, and
/// FACTORY_QA_OPENROUTER_MODELS_URL OpenRouter's model list, within Fugo's
/// guard; anything the guard refuses stops factoryd, saying why.
fn registry(home: &Home, port: u16) -> Result<Registry, MainError> {
    #[cfg(feature = "qa-provider")]
    {
        use factory_agent::providers::anthropic::qa;
        // OpenRouter's model list from qa-model (#185 U4), within the same guard.
        let list = std::env::var_os(qa::MODELS_URL_VAR).map(|url| url.to_string_lossy().into_owned());
        if let Some(url) = qa::models_url_for(list.as_deref(), home.root(), port).map_err(MainError::QaProvider)? {
            tracing::warn!("QA OpenRouter model list override: {url}");
            factory_agent::providers::catalog::use_qa_models_url(url);
        }
        let url = std::env::var_os(qa::URL_VAR).map(|url| url.to_string_lossy().into_owned());
        if let Some((base, anthropic)) = qa::provider_for(url.as_deref(), home.root(), port).map_err(MainError::QaProvider)? {
            tracing::warn!("QA provider override: {base}");
            let mut providers = factory_agent::providers::models::default_providers();
            if let Some(slot) = providers.iter_mut().find(|provider| provider.id == anthropic.id) {
                *slot = anthropic;
            }
            let store = factory_agent::credentials::FileCredentialStore::new(home.auth_path());
            // Images go to qa-model's Files API as they would to Anthropic's (#382).
            let files = home.auth_path().with_file_name(factory_agent::providers::models::IMAGE_FILES);
            return Ok(Registry::with_providers(store, factory_agent::auth::process_env(), providers).keeping_image_files(Some(files)));
        }
    }
    #[cfg(not(feature = "qa-provider"))]
    let _ = port;
    let store = factory_agent::credentials::FileCredentialStore::new(home.auth_path());
    let files = home.auth_path().with_file_name(factory_agent::providers::models::IMAGE_FILES);
    Ok(Registry::with_providers(store, factory_agent::auth::process_env(), factory_agent::providers::models::default_providers()).keeping_image_files(Some(files)))
}

/// The backend's listener. On Windows it's bound for this process alone
/// (SO_EXCLUSIVEADDRUSE, not inherited), and a backend that can't bind it
/// so doesn't serve: never a plain bind instead (Fugo 48392 c, Jotaro 48933).
async fn listen(addr: SocketAddr) -> std::io::Result<TcpListener> {
    #[cfg(windows)]
    {
        TcpListener::from_std(factoryd::windows::listener::bind_exclusive(addr)?)
    }
    #[cfg(not(windows))]
    {
        TcpListener::bind(addr).await
    }
}

async fn serve(instance: Instance, handed: Option<factoryd::vault::handoff::Handed>) -> Result<(), MainError> {
    let Instance { home, addr } = instance;
    // Never half started: an import that didn't finish is finished first.
    if let Some(why) = factoryd::move_import::unfinished(&home) {
        return Err(MainError::ImportUnfinished(why));
    }
    // Before it listens: a QA override the guard refuses stops it here.
    #[cfg(feature = "qa-lies")]
    factoryd::qa_lies::init(home.root(), addr.port()).map_err(MainError::QaLies)?;
    let models = Arc::new(registry(&home, addr.port())?);
    // A helper an older backend's update left out goes in beside this one.
    factoryd::update::heal_helpers(&home);
    let listener = listen(addr).await.map_err(|source| match source.kind() {
        // An exclusive bind of a port another socket holds is refused
        // as in use, or as access denied.
        #[cfg(windows)]
        std::io::ErrorKind::AddrInUse | std::io::ErrorKind::PermissionDenied => {
            MainError::PortTaken(factoryd::windows::listener::taken_by(addr.port()))
        }
        #[cfg(unix)]
        std::io::ErrorKind::AddrInUse => MainError::PortInUse(addr.port()),
        _ => MainError::Bind { addr, source },
    })?;
    // The app's token for this start, once the port is this backend's alone
    // and before anything is served (Fugo 48927); without one, no Hello
    // matches it.
    #[cfg(windows)]
    let app_token = factoryd::windows::app_token::rotate(home.root())
        .inspect_err(|error| tracing::error!(%error, "could not write the app's token"))
        .ok();
    // Moving to another computer, frozen for its last part: only the
    // move's progress is served, before anything else (moved.json included).
    if let Some(saved) = factoryd::move_run::frozen(&home) {
        factoryd::move_run::serve_frozen(listener, &home, saved, handed).await;
        return Ok(());
    }
    // Moved to another computer: nothing of it opens here.
    if let Some(moved) = factoryd::move_import::moved::read(&home) {
        factoryd::move_import::moved::serve(listener, &home, moved).await;
        return Ok(());
    }
    let factory = Factory::open(home.clone(), models).await?;
    #[cfg(windows)]
    factory.set_app_token(app_token.clone());
    if let Some(handed) = handed {
        factory.take_vault_key(handed);
    }
    factoryd::backend_started(&factory);
    // A move a restart interrupted while copying carries on.
    factoryd::move_run::resume(&factory);
    // Factories installed from published releases download newer ones.
    if let Some(url) = factoryd::channel::url() {
        tracing::info!(%url, "checking for releases");
        factory.use_channel(&url);
        tokio::spawn(factoryd::channel::poll(factory.clone()));
    }
    // Older installs' LaunchAgent gets what installers write now, once.
    tokio::spawn(factoryd::launch_agent::update(home.clone(), addr.port()));
    // On Windows, the supervisor's own copy, when its version went up.
    #[cfg(windows)]
    tokio::spawn(factoryd::windows::supervisor_copy::update(home.clone()));
    tracing::info!(%addr, home = %home.root().display(), version = factoryd::update::VERSION, "factoryd listening");
    // SIGTERM (the supervisor stopping, at a restart or shutdown): what's
    // being written is finished first. On Windows, the supervisor's stop
    // event, a console's Ctrl+C or close, signing out or shutting down.
    #[cfg(unix)]
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(MainError::Signal)?;
    #[cfg(unix)]
    let stop = terminate.recv();
    #[cfg(windows)]
    let stop = factoryd::windows::stop::listen().map_err(MainError::Signal)?.requested();
    tokio::select! {
        () = factoryd::serve(listener, factory.clone()) => {}
        _ = stop => {
            tracing::info!("stopping");
            factory.finish_writes(Duration::from_secs(5)).await;
            factory.stopping();
            factory.close_tunnel(Duration::from_secs(2)).await;
            factoryd::backend_stopped(&home);
            #[cfg(windows)]
            if let Some(token) = &app_token {
                factoryd::windows::app_token::forget(home.root(), token);
            }
        }
    }
    Ok(())
}
