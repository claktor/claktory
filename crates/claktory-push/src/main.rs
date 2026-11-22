//! `claktory-push`: Claktory's push relay, as one binary.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use claktory_push::server::{self, Config, Service};
use claktory_push::{Gateways, Transport, apns, fcm, local};

const USAGE: &str = "\
claktory-push: Claktory's push relay

Usage:
  claktory-push [serve] --db FILE [options]

Options:
  --listen ADDRESS          where to listen (127.0.0.1:8090)
  --trust-proxy             behind a TLS proxy such as Caddy: the client's IP is
                            the last of X-Forwarded-For
  --apns-key FILE           the APNs auth key (.p8)
  --apns-key-id ID          its key id
  --apns-team ID            the Apple developer team's id
  --apns-topic BUNDLE       the app's bundle id (com.claktory.app)
  --fcm-service-account FILE
                            the Firebase project's service account (JSON)
  --local FILE              for tests on this computer: no keys, and nothing
                            goes to Apple or Google; each push is logged and
                            appended to FILE as a JSON line instead

The keys are read once, at start. Under systemd, give them as credentials
(LoadCredentialEncrypted=apns.p8:… and fcm.json:…): with no --apns-key or
--fcm-service-account, the relay reads apns.p8 and fcm.json from
$CREDENTIALS_DIRECTORY when they're there, so they never sit in a file
on disk. Without an APNs key it can't push to iPhones; without a service
account, not to Android phones.
";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.first().is_some_and(|arg| !arg.starts_with('-')) { args.remove(0) } else { "serve".to_owned() };
    let result = match command.as_str() {
        "serve" => serve(&args),
        "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("claktory-push: {error}");
            ExitCode::FAILURE
        }
    }
}

/// `--name value` pairs and `--flag`s.
struct Options(Vec<String>);

impl Options {
    fn value(&self, name: &str) -> Result<Option<String>, String> {
        match self.0.iter().position(|arg| arg == name) {
            None => Ok(None),
            Some(at) => {
                self.0.get(at + 1).filter(|value| !value.starts_with("--")).cloned().map(Some).ok_or(format!("{name} needs a value"))
            }
        }
    }

    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|arg| arg == name)
    }

    fn check(&self, known: &[&str], flags: &[&str]) -> Result<(), String> {
        let mut at = 0;
        while at < self.0.len() {
            let arg = self.0[at].as_str();
            if flags.contains(&arg) {
                at += 1;
            } else if known.contains(&arg) {
                at += 2;
            } else {
                return Err(format!("unknown option {arg}\n\n{USAGE}"));
            }
        }
        Ok(())
    }
}

/// The file named by `option`, or systemd's credential `name` if there is one.
fn key_file(options: &Options, option: &str, name: &str) -> Result<Option<PathBuf>, String> {
    if let Some(path) = options.value(option)? {
        return Ok(Some(PathBuf::from(path)));
    }
    Ok(std::env::var_os("CREDENTIALS_DIRECTORY").map(|dir| Path::new(&dir).join(name)).filter(|path| path.exists()))
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|error| format!("can't read {}: {error}", path.display()))
}

fn serve(args: &[String]) -> Result<(), String> {
    let options = Options(args.to_vec());
    options.check(
        &["--db", "--listen", "--apns-key", "--apns-key-id", "--apns-team", "--apns-topic", "--fcm-service-account", "--local"],
        &["--trust-proxy"],
    )?;
    let db = options.value("--db")?.ok_or(format!("--db is needed\n\n{USAGE}"))?;
    let listen = options.value("--listen")?.unwrap_or_else(|| "127.0.0.1:8090".to_owned());
    let transport: Arc<dyn Transport> = match options.value("--local")? {
        Some(file) => {
            if ["--apns-key", "--apns-key-id", "--apns-team", "--apns-topic", "--fcm-service-account"].iter().any(|key| options.flag(key)) {
                return Err("--local sends nothing to Apple or Google, so it takes no keys".to_owned());
            }
            tracing::info!(%listen, held = %file, "claktory-push starting, local: pushes are held in a file, never sent");
            Arc::new(local::Local::new(Path::new(&file)))
        }
        None => gateways(&options, &listen)?,
    };
    let config = Config { trust_proxy: options.flag("--trust-proxy"), ..Config::default() };
    let runtime = tokio::runtime::Runtime::new().map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let service = Service::new(Path::new(&db), transport, config, Arc::new(claktory_push::now))?;
        service.spawn_upkeep();
        let listener = tokio::net::TcpListener::bind(&listen).await.map_err(|error| format!("can't listen on {listen}: {error}"))?;
        tokio::select! {
            result = server::serve(listener, service) => result.map_err(|error| error.to_string()),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    })
}

/// Apple's and Google's gateways, with the keys given.
fn gateways(options: &Options, listen: &str) -> Result<Arc<dyn Transport>, String> {
    let apns = match key_file(options, "--apns-key", "apns.p8")? {
        Some(path) => {
            let key_id = options.value("--apns-key-id")?.ok_or("--apns-key-id is needed with an APNs key")?;
            let team = options.value("--apns-team")?.ok_or("--apns-team is needed with an APNs key")?;
            let topic = options.value("--apns-topic")?.unwrap_or_else(|| "com.claktory.app".to_owned());
            Some(apns::Apns::new(&read(&path)?, &key_id, &team, &topic)?)
        }
        None => None,
    };
    let fcm = match key_file(options, "--fcm-service-account", "fcm.json")? {
        Some(path) => Some(fcm::Fcm::new(&read(&path)?)?),
        None => None,
    };
    if apns.is_none() && fcm.is_none() {
        tracing::warn!("no APNs key and no Firebase service account: every push will be refused");
    }
    tracing::info!(apns = apns.is_some(), fcm = fcm.is_some(), %listen, "claktory-push starting");
    Ok(Arc::new(Gateways { apns, fcm }))
}
