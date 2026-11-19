//! `factoryd [--dev]` serves a Factory; `factoryd [--dev] supervise` keeps
//! one running and undoes updates that don't start (see `update`);
//! `factoryd [--dev] login [PROVIDER]` signs it in to a model provider;
//! `factoryd [--dev] pair NAME [--host ADDRESS]` pairs a device on another
//! machine and prints its pairing code (naming this machine by ADDRESS),
//! `unpair DEVICE` unpairs one, `devices` lists them, and `remote on [PORT]`
//! and `remote off` turn the listener for them on and off (see `remote`).
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
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

const USAGE: &str = "usage: factoryd [--dev] [supervise | login [PROVIDER] | pair NAME [--host ADDRESS] | unpair DEVICE | devices | remote on [PORT] | remote off]";

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

#[tokio::main]
async fn main() -> ExitCode {
    // One crypto provider for TLS code that asks rustls for the process default:
    // with two in the build (ring for the providers, aws-lc-rs for remote
    // access) there's none, and rustls panics. Factory's own clients name theirs.
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
            let home = instance.home.clone();
            let served = serve(instance).await;
            if let Err(error) = &served {
                update::trial_failed(&home, error);
            }
            served
        }
        [supervise] if supervise == "supervise" => supervise_backend(instance.home, dev).await,
        [login] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), None).await?),
        [login, provider] if login == "login" => Ok(factory_agent::login_terminal(&instance.home.auth_path(), Some(provider)).await?),
        [pair, name @ ..] if pair == "pair" && !name.is_empty() => {
            // `--host ADDRESS`: the address the device reaches this machine by.
            let (name, host) = match name {
                [name @ .., flag, host] if flag == "--host" => (name, host.as_str()),
                name => (name, ""),
            };
            if name.is_empty() {
                return Err(MainError::Usage);
            }
            let (code, _) = factoryd::remote::pair(&instance.home, &name.join(" "), host)?;
            println!("Paste this pairing code into Factory on the other device (it's shown once, and holds the device's key):\n\n{code}");
            Ok(())
        }
        [unpair, device @ ..] if unpair == "unpair" && !device.is_empty() => {
            let device = factoryd::remote::unpair(&instance.home, &device.join(" "))?;
            println!("Unpaired {} ({}): it can't connect any more.", device.name, device.id);
            Ok(())
        }
        [devices] if devices == "devices" => {
            for device in factoryd::remote::devices(&instance.home)? {
                let seen = match device.last_seen.map(jiff::Timestamp::from_millisecond) {
                    Some(Ok(seen)) => format!("last connected {seen}"),
                    _ => "never connected".to_owned(),
                };
                println!("{}  {}  {seen}", device.id, device.name);
            }
            Ok(())
        }
        [remote, on, port @ ..] if remote == "remote" && (on == "on" || on == "off") && port.len() <= usize::from(on == "on") => {
            let port = port.first().map(|port| port.parse::<u16>()).transpose().map_err(|_| MainError::Usage)?;
            match factoryd::remote::set_remote(&instance.home, on == "on", port)? {
                Some(remote) => println!("Remote access is on, on port {} (devices pair with: factoryd pair NAME).", remote.port),
                None => println!("Remote access is off."),
            }
            Ok(())
        }
        _ => Err(MainError::Usage),
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
    tracing::info!(backend = %home.backend().display(), version = update::VERSION, "supervising");
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
        let ended = match command.spawn() {
            Ok(mut child) => {
                let status = tokio::select! {
                    status = child.wait() => status,
                    _ = terminate.recv() => return stop(child).await,
                    _ = interrupt.recv() => return stop(child).await,
                };
                match status {
                    Ok(status) if status.success() => None,
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

async fn stop(mut child: tokio::process::Child) -> Result<(), MainError> {
    if let Err(error) = child.kill().await {
        tracing::warn!(%error, "could not stop the backend");
    }
    Ok(())
}

async fn serve(instance: Instance) -> Result<(), MainError> {
    let Instance { home, addr } = instance;
    let listener = TcpListener::bind(addr).await.map_err(|source| MainError::Bind { addr, source })?;
    let models = Arc::new(Registry::new(home.auth_path()));
    let factory = Factory::open(home.clone(), models).await?;
    // Factories installed from published releases download newer ones.
    if let Some(url) = factoryd::channel::url() {
        tokio::spawn(factoryd::channel::poll(home.clone(), url));
    }
    // Older installs' LaunchAgent gets what installers write now, once.
    tokio::spawn(factoryd::launch_agent::update(home.clone()));
    tracing::info!(%addr, home = %home.root().display(), version = factoryd::update::VERSION, "factoryd listening");
    factoryd::serve(listener, factory).await;
    Ok(())
}
