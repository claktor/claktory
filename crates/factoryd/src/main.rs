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

const USAGE: &str =
    "usage: factoryd [--dev] [supervise | login [PROVIDER] | pair NAME | allow DEVICE | deny DEVICE | unpair DEVICE | devices]";

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
    #[error("login failed: {0}")]
    Login(#[from] factory_agent::Error),
    #[error("FACTORY_PORT must be a port number: {0}")]
    Port(#[from] std::num::ParseIntError),
    #[error("could not listen for signals: {0}")]
    Signal(std::io::Error),
    #[error(transparent)]
    Remote(#[from] factoryd::remote::RemoteError),
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
    if let Some(at) = args.iter().position(|arg| arg != "--dev")
        && (args[at] == "install" || args[at] == "uninstall")
    {
        let mut flags: Vec<String> = args[..at].to_vec();
        flags.extend_from_slice(&args[at + 1..]);
        return if args[at] == "install" { factoryd::install::main(&flags) } else { factoryd::uninstall::main(&flags) };
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
    // Colors only for a terminal; log files stay plain text.
    tracing_subscriber::fmt().with_env_filter(filter).with_ansi(std::io::stdout().is_terminal()).init();
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
        [pair, name @ ..] if pair == "pair" && !name.is_empty() => {
            // `--host ADDRESS`, from before the private tunnel: devices find
            // this machine by its key now, so it's left out.
            let name = match name {
                [name @ .., flag, _] if flag == "--host" => name,
                name => name,
            };
            if name.is_empty() {
                return Err(MainError::Usage);
            }
            let (code, device) = factoryd::remote::pair_offline(&instance.home, &name.join(" "))?;
            let minutes = factoryd::remote::CODE_LIFETIME.as_secs() / 60;
            // A terminal gets the code as a QR code too, to scan from the
            // screen; piped elsewhere, just the text.
            use std::io::IsTerminal;
            let qr = if std::io::stdout().is_terminal() { factoryd::terminal_qr::lines(&code) } else { None };
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
        [allow, device @ ..] if allow == "allow" && !device.is_empty() => {
            let device = factoryd::remote::approve(&instance.home, &device.join(" "))?;
            println!("Allowed {}: it can connect now.", device.name);
            Ok(())
        }
        [deny, device @ ..] if deny == "deny" && !device.is_empty() => {
            let device = factoryd::remote::deny(&instance.home, &device.join(" "))?;
            println!("Not allowed. {} can't connect. To try again, run factoryd pair.", device.name);
            Ok(())
        }
        [unpair, device @ ..] if unpair == "unpair" && !device.is_empty() => {
            let device = factoryd::remote::unpair(&instance.home, &device.join(" "))?;
            println!("Unpaired {} ({}): it can't connect any more.", device.name, device.id);
            Ok(())
        }
        [devices] if devices == "devices" => {
            for device in factoryd::remote::devices(&instance.home)?.into_iter().filter(|device| !device.local) {
                let seen = match device.last_seen.map(jiff::Timestamp::from_millisecond) {
                    _ if device.is_pending(now_ms()) => {
                        let claktory = factoryd::remote::secret_key(&instance.home)?.public().to_string();
                        let number = factoryd::remote::approval_number(&claktory, &device.pending_endpoint_id, &device.token_sha256);
                        format!("waiting to be allowed: allow it with factoryd allow {} if it shows {number}", device.id)
                    }
                    Some(Ok(seen)) => format!("last connected {seen}"),
                    _ => "never connected".to_owned(),
                };
                println!("{}  {}  {seen}", device.id, device.name);
            }
            Ok(())
        }
        // From before the private tunnel, which needs no switch.
        [remote, ..] if remote == "remote" => {
            println!("Remote access is on while devices are paired: pair one with factoryd pair NAME.");
            Ok(())
        }
        _ => Err(MainError::Usage),
    }
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
                println!("Allowed in Claktory. {} can connect now.", known.name);
                return Ok(());
            }
            Some(known) if known.is_pending(now) => {
                let number = remote::approval_number(&claktory, &known.pending_endpoint_id, &known.token_sha256);
                print!("\nA device wants to connect as \"{}\". Its screen should show {number}. Allow it? [y/N] ", known.name);
                std::io::stdout().flush().map_err(remote::RemoteError::from)?;
                let answer = tokio::task::spawn_blocking(move || {
                    let mut line = String::new();
                    std::io::BufReader::new(tty).read_line(&mut line).map(|_| line)
                });
                let answer = tokio::select! {
                    answer = answer => answer.map_err(std::io::Error::other).and_then(|line| line).map_err(remote::RemoteError::from)?,
                    _ = tokio::signal::ctrl_c() => {
                        let _ = remote::cancel(home, &device.id);
                        println!("\nCancelled. The code no longer works.");
                        std::process::exit(130);
                    }
                };
                let allowed = if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
                    remote::approve(home, &device.id).map(|_| true)
                } else {
                    remote::deny(home, &device.id).map(|_| false)
                };
                match allowed {
                    Ok(true) => println!("Allowed. {} can connect now.", known.name),
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
/// undone. Stops it on SIGTERM or SIGINT.
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

async fn serve(instance: Instance, handed: Option<factoryd::vault::handoff::Handed>) -> Result<(), MainError> {
    let Instance { home, addr } = instance;
    let listener = TcpListener::bind(addr).await.map_err(|source| MainError::Bind { addr, source })?;
    let models = Arc::new(Registry::new(home.auth_path()));
    let factory = Factory::open(home.clone(), models).await?;
    if let Some(handed) = handed {
        factory.take_vault_key(handed);
    }
    // Factories installed from published releases download newer ones.
    if let Some(url) = factoryd::channel::url() {
        tokio::spawn(factoryd::channel::poll(home.clone(), url));
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
        }
    }
    Ok(())
}
