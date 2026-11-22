//! claktory-user-apps: the user-level apps' worker, beside the account
//! service. It listens on a local socket that only the service's user may
//! use, keeps its own database, and holds none of the service's keys.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::ExitCode;

use claktory_user_apps::worker::{self, Worker};

const USAGE: &str = "\
claktory-user-apps: the worker for Claktory's user-level apps (DMs).

usage:
  claktory-user-apps [serve] --db FILE --socket PATH --root-uid UID
  claktory-user-apps backup --db FILE --to DIR [--keep N]
  claktory-user-apps help

serve:
  --db FILE       the worker's own database (made if missing)
  --socket PATH   the socket to listen on (replaced if there), made 0660; the
                  account service connects to it (its --user-apps PATH)
  --root-uid UID  the account service's user: connections from any other
                  user are refused

backup writes a consistent copy of the database while the worker runs, as
DIR/apps-<UTC time>.db, and keeps the newest N there (7).
";

/// `--name value` pairs.
struct Options(Vec<String>);

impl Options {
    fn new(args: Vec<String>, known: &[&str]) -> Result<Options, String> {
        let mut at = 0;
        while at < args.len() {
            if !known.contains(&args[at].as_str()) {
                return Err(format!("unknown option {}", args[at]));
            }
            if args.get(at + 1).is_none_or(|value| value.starts_with("--")) {
                return Err(format!("{} needs a value", args[at]));
            }
            at += 2;
        }
        Ok(Options(args))
    }

    fn value(&self, name: &str) -> Option<String> {
        self.0.iter().position(|arg| arg == name).and_then(|at| self.0.get(at + 1)).cloned()
    }

    fn required(&self, name: &str) -> Result<String, String> {
        self.value(name).ok_or_else(|| format!("{name} is needed"))
    }
}

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let command = if args.first().is_some_and(|arg| !arg.starts_with("--")) { args.remove(0) } else { "serve".to_owned() };
    let result = match command.as_str() {
        "serve" if args.iter().any(|arg| arg == "--help" || arg == "-h") => {
            print!("{USAGE}");
            Ok(())
        }
        "serve" => serve(args),
        "backup" => backup(args),
        "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("claktory-user-apps: {error}");
            ExitCode::FAILURE
        }
    }
}

fn backup(args: Vec<String>) -> Result<(), String> {
    let options = Options::new(args, &["--db", "--to", "--keep"])?;
    let db = PathBuf::from(options.required("--db")?);
    let dir = PathBuf::from(options.required("--to")?);
    let keep = match options.value("--keep") {
        Some(keep) => keep.parse::<usize>().ok().filter(|keep| *keep > 0).ok_or("--keep takes a number from 1")?,
        None => 7,
    };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_secs() as i64;
    let path = worker::backup(&db, &dir, keep, now)?;
    println!("{}", path.display());
    Ok(())
}

fn serve(args: Vec<String>) -> Result<(), String> {
    let options = Options::new(args, &["--db", "--socket", "--root-uid"])?;
    let db = PathBuf::from(options.required("--db")?);
    let socket = PathBuf::from(options.required("--socket")?);
    let root_uid: u32 = options.required("--root-uid")?.parse().map_err(|_| "--root-uid takes a number")?;
    let worker = Worker::open(&db)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        match std::fs::remove_file(&socket) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(format!("can't replace {}: {error}", socket.display()));
            }
            _ => {}
        }
        let listener = tokio::net::UnixListener::bind(&socket).map_err(|error| format!("can't listen on {}: {error}", socket.display()))?;
        // The service reaches it through the socket's group (the deploy's);
        // the peer check refuses every other user anyway.
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o660))
            .map_err(|error| format!("can't set {}'s permissions: {error}", socket.display()))?;
        tracing::info!("serving on {} for uid {root_uid}", socket.display());
        tokio::select! {
            served = worker::serve(listener, worker, root_uid) => served.map_err(|error| error.to_string()),
            _ = tokio::signal::ctrl_c() => Ok(()),
        }
    })
}
