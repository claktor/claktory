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
    #[error(transparent)]
    Open(#[from] OpenError),
    #[error("could not listen on {addr}: {source}")]
    Bind { addr: SocketAddr, source: std::io::Error },
    /// The port is taken, most likely by another user's Claktory on this
    /// computer: said plainly, since it's what the person can act on.
    #[error(
        "Another Claktory on this computer already uses port {0} (another user's, say), so this one can't start. Stop that one, or start this one on another port with FACTORY_PORT."
    )]
    PortInUse(u16),
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
}

fn main() -> ExitCode {
    // A command's log filter (see factory_agent::secrets::filter_main): one
    // per command with secrets, so it starts lean, without a runtime or logs.
    if std::env::args().nth(1).as_deref() == Some("redact") {
        return factory_agent::secrets::filter_main();
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
        return if args[at] == "install" { factoryd::install::main(&flags) } else { factoryd::uninstall::main(&flags) };
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
        tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
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
    let user_home = std::env::home_dir().ok_or(MainError::NoHome)?;
    let mut instance = home::instance(dev, &user_home);
    if let Some(root) = std::env::var_os("FACTORY_HOME") {
        instance.home = Home::new(root.into());
    }
    if let Ok(port) = std::env::var("FACTORY_PORT") {
        instance.addr.set_port(port.parse()?);
    }
    // Browser control stays out of sight until its security proof (B6a):
    // only a Claktory started with this, the dev one say, has it. No
    // installer, LaunchAgent or release script ever sets it.
    factoryd::browser::set_preview(std::env::var_os("FACTORY_BROWSER_PREVIEW").is_some_and(|on| on == "1"));
    // So do agents in direct messages, until People ships: the same rule.
    factoryd::user_apps::dms::set_preview(std::env::var_os("FACTORY_DMS_PREVIEW").is_some_and(|on| on == "1"));
    match command.as_slice() {
        [] => {
            // It holds the vault's key: no core dumps, and on Linux no other
            // process of this user reads its memory.
            factoryd::vault::memory::harden();
            // Before anything starts: the open vault's key from the backend
            // before this one, at an update.
            let handed = factoryd::vault::handoff::receive();
            if let Ok(factoryd) = std::env::current_exe() {
                factoryd::set_filter_executable(factoryd);
            }
            let home = instance.home.clone();
            let served = serve(instance, handed).await;
            if let Err(error) = &served {
                update::trial_failed(&home, error);
            }
            served
        }
        [supervise] if supervise == "supervise" => supervise_backend(instance.home, dev).await,
        [login] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), None).await?),
        [login, provider] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), Some(provider)).await?),
        [pair, name @ ..] if pair == "pair" => {
            // `--host ADDRESS`, from before the private tunnel: devices find
            // this machine by its key now, so it's left out. Without a name,
            // the device that uses the code gives its own.
            let name = match name {
                [name @ .., flag, _] if flag == "--host" => name,
                name => name,
            };
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
                    println!("Scan this with Claktory on your iPhone:\n");
                    for line in lines {
                        println!("  {line}");
                    }
                    println!(
                        "\nOr, on your Mac, click Add a Claktory (+) at the far left of Claktory's window and paste this code:\n\n{code}\n"
                    );
                }
                None => println!(
                    "Paste this code into Claktory on your other device (on a Mac: Add a Claktory, the + at the far left of the window):\n\n{code}\n"
                ),
            }
            println!("It works once, for {minutes} minutes. You'll be asked to allow the device that uses it.");
            allow_in_terminal(&instance.home, &device).await
        }
        // `allow` goes to the running Claktory (cli), which checks the
        // number typed (#352 P4); never allowed here, without it.
        [allow, device @ ..] if allow == "allow" && !device.is_empty() => {
            eprintln!("Run factoryd allow {} --number <the 4 digits on its screen> while Claktory runs.", device.join(" "));
            std::process::exit(2);
        }
        [command, rest @ ..] if command == "import" => import(&instance, rest),
        [command, rest @ ..] if command == "receive" => receive(&instance, rest).await,
        [deny, device @ ..] if deny == "deny" && !device.is_empty() => {
            let device = factoryd::remote::deny(&instance.home, &device.join(" "))?;
            println!("Not allowed. {} can't connect. To try again, run factoryd pair.", factoryd::remote::shown_name(&device.name));
            Ok(())
        }
        [unpair, device @ ..] if unpair == "unpair" && !device.is_empty() => {
            let device = factoryd::remote::unpair(&instance.home, &device.join(" "))?;
            println!("Removed {} ({}): it can't connect any more.", factoryd::remote::shown_name(&device.name), device.id);
            Ok(())
        }
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
    for arg in args {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("usage: factoryd receive [--committed] [--json]");
                return Ok(());
            }
            "--committed" => options.committed = true,
            "--json" => {}
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
                let text = code.text();
                if let Some(lines) = terminal.then(|| factoryd::terminal_qr::lines(&text)).flatten() {
                    println!("\nScan this with Claktory on your iPhone, or paste the code below:\n");
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
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(MainError::Signal)?;
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

/// After `factoryd pair` prints a code in a terminal: waits for the device
/// that uses it, and asks whether to allow it, with the number it shows.
/// Ctrl-C ends the code. With no terminal it returns at once: the device
/// is allowed in the app, or with `factoryd allow`.
async fn allow_in_terminal(home: &Home, device: &factoryd::remote::Device) -> Result<(), MainError> {
    use factoryd::remote;
    use std::io::{BufRead, Write};
    let Ok(tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else {
        println!("Allow the device in Claktory when it asks, or run factoryd allow {}.", device.id);
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
                    Ok(false) => println!("Not allowed. To try again, run factoryd pair."),
                    Err(_) => println!("The code expired or was cancelled. Run factoryd pair for a new one."),
                }
                return Ok(());
            }
            Some(known) if known.code_expires.is_some_and(|expires| expires > now) => {}
            _ => {
                let _ = remote::cancel(home, &device.id);
                println!("The code expired or was cancelled. Run factoryd pair for a new one.");
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
async fn supervise_backend(home: Home, dev: bool) -> Result<(), MainError> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut terminate = signal(SignalKind::terminate()).map_err(MainError::Signal)?;
    let mut interrupt = signal(SignalKind::interrupt()).map_err(MainError::Signal)?;
    let mut pause = Duration::from_millis(500);
    let mut failures = 0;
    tracing::info!(backend = %home.backend().display(), version = update::VERSION, "supervising");
    // It may hold the vault's key between two backends.
    factoryd::vault::memory::harden();
    let mut holder = factoryd::vault::handoff::Holder::default();
    loop {
        if let Err(error) = update::start_trial(&home) {
            tracing::error!(%error, "could not start the update's trial");
        }
        let started = Instant::now();
        let mut command = tokio::process::Command::new(home.backend());
        if dev {
            command.arg("--dev");
        }
        command.env(update::SUPERVISED, "1").kill_on_drop(true);
        // The vault's key goes from one backend to the next through a pair.
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
        let spawned = command.spawn();
        // Only the backend holds its end now, so the pair ends with it.
        let mut ours = channel.and_then(|(ours, theirs)| {
            drop(theirs);
            ours.set_nonblocking(true).ok()?;
            tokio::net::UnixStream::from_std(ours).ok()
        });
        let ended = match spawned {
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
        if update::update_failed(&home) {
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
async fn stop(mut child: tokio::process::Child) -> Result<(), MainError> {
    if let Some(pid) = child.id() {
        // SAFETY: kill only sends a signal to the backend, our child.
        unsafe { libc::kill(pid as i32, libc::SIGTERM) };
        if tokio::time::timeout(STOP_WITHIN, child.wait()).await.is_ok() {
            return Ok(());
        }
        tracing::warn!("the backend didn't stop within {}s; killing it", STOP_WITHIN.as_secs());
    }
    if let Err(error) = child.kill().await {
        tracing::warn!(%error, "could not stop the backend");
    }
    Ok(())
}

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
            return Ok(Registry::with_providers(store, factory_agent::auth::process_env(), providers));
        }
    }
    #[cfg(not(feature = "qa-provider"))]
    let _ = port;
    Ok(Registry::new(home.auth_path()))
}

async fn serve(instance: Instance, handed: Option<factoryd::vault::handoff::Handed>) -> Result<(), MainError> {
    let Instance { home, addr } = instance;
    // Never half started: an import that didn't finish is finished first.
    if let Some(why) = factoryd::move_import::unfinished(&home) {
        return Err(MainError::ImportUnfinished(why));
    }
    // Before it listens: a QA override the guard refuses stops it here.
    let models = Arc::new(registry(&home, addr.port())?);
    // A helper an older backend's update left out goes in beside this one.
    factoryd::update::heal_helpers(&home);
    let listener = TcpListener::bind(addr).await.map_err(|source| match source.kind() {
        std::io::ErrorKind::AddrInUse => MainError::PortInUse(addr.port()),
        _ => MainError::Bind { addr, source },
    })?;
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
    tracing::info!(%addr, home = %home.root().display(), version = factoryd::update::VERSION, "factoryd listening");
    // SIGTERM (the supervisor stopping, at a restart or shutdown): what's
    // being written is finished first.
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).map_err(MainError::Signal)?;
    tokio::select! {
        () = factoryd::serve(listener, factory.clone()) => {}
        _ = terminate.recv() => {
            tracing::info!("stopping");
            factory.finish_writes(Duration::from_secs(5)).await;
            factoryd::backend_stopped(&home);
        }
    }
    Ok(())
}
