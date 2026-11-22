//! claktory-user-apps: the user-level apps' worker, beside the account
//! service. It listens on a local socket that only the service's user may
//! use, keeps its own database, and holds none of the service's keys.

use std::os::unix::fs::PermissionsExt as _;
use std::path::PathBuf;
use std::process::ExitCode;

use claktory_user_apps::dms::keys::Keys;
use claktory_user_apps::worker::{self, Worker};

const USAGE: &str = "\
claktory-user-apps: the worker for Claktory's user-level apps (DMs).

usage:
  claktory-user-apps [serve] --db FILE --dm-key FILE --socket PATH --root-uid UID
  claktory-user-apps init-key FILE
  claktory-user-apps rewrap --db FILE --from OLD-KEY --to NEW-KEY
  claktory-user-apps backup --db FILE --to DIR [--keep N]
  claktory-user-apps reports --db FILE --dm-key FILE --operator NAME [--close ID]
  claktory-user-apps help

serve:
  --db FILE       the worker's own database (made if missing)
  --dm-key FILE   the DM wrapping key (init-key makes it): what opens DMs at
                  rest. Kept apart from the database and its backups, and
                  backed up offline: without it, they open to nothing. A
                  database made with another key is refused
  --socket PATH   the socket to listen on (replaced if there), made 0660; the
                  account service connects to it (its --user-apps PATH)
  --root-uid UID  the account service's user: connections from any other
                  user are refused

init-key writes a new DM wrapping key to FILE (0600, never over one) and
prints its id. Make it once, before the first serve.

rewrap seals the database's DM data keys with the wrapping key NEW-KEY (from
init-key) instead of OLD-KEY, all at once or not at all: after OLD-KEY may have
been exposed. Stop the worker first, then start it with NEW-KEY. Backups made
before still need OLD-KEY. The data keys stay the same, so it answers a leaked
key file, not a leaked key file and a copy of the database sealed with it.

reports prints the open DM reports, oldest first, one JSON object a line:
who reported whom (account ids), about what, why, and only what the reporter
chose to send. Each one printed is recorded as read by NAME (you), with the
time. --close ID closes a report instead (its reporter sees it reviewed),
recorded the same way. Reports go 30 days after they're made.

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
        "init-key" => init_key(args),
        "rewrap" => rewrap(args),
        "reports" => reports(args),
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

fn init_key(args: Vec<String>) -> Result<(), String> {
    let [path] = args.as_slice() else { return Err(format!("init-key takes one FILE\n\n{USAGE}")) };
    let keys = Keys::create(&PathBuf::from(path)).map_err(|refused| refused.to_string())?;
    println!("{}", keys.id());
    Ok(())
}

fn rewrap(args: Vec<String>) -> Result<(), String> {
    let options = Options::new(args, &["--db", "--from", "--to"])?;
    let db = PathBuf::from(options.required("--db")?);
    let from = Keys::load(&PathBuf::from(options.required("--from")?)).map_err(|refused| refused.to_string())?;
    let to = Keys::load(&PathBuf::from(options.required("--to")?)).map_err(|refused| refused.to_string())?;
    if from.id() == to.id() {
        return Err("--from and --to are the same key".to_owned());
    }
    let mut conn = rusqlite::Connection::open(&db).map_err(|error| format!("can't open {}: {error}", db.display()))?;
    conn.busy_timeout(std::time::Duration::from_secs(30)).map_err(|error| error.to_string())?;
    let count = claktory_user_apps::dms::keys::rewrap(&mut conn, &from, &to).map_err(|refused| refused.to_string())?;
    println!("{count} data keys now sealed with {}", to.id());
    Ok(())
}

fn reports(args: Vec<String>) -> Result<(), String> {
    let options = Options::new(args, &["--db", "--dm-key", "--operator", "--close"])?;
    let db = PathBuf::from(options.required("--db")?);
    let keys = Keys::load(&PathBuf::from(options.required("--dm-key")?)).map_err(|refused| refused.to_string())?;
    let operator = options.required("--operator")?;
    if operator.trim().is_empty() || operator.len() > 80 || operator.chars().any(char::is_control) {
        return Err("--operator takes your name, at most 80 characters".to_owned());
    }
    if !db.exists() {
        return Err(format!("{} doesn't exist", db.display()));
    }
    let mut conn = worker::open_db(&db, &keys)?;
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_millis() as i64;
    if let Some(id) = options.value("--close") {
        let closed =
            claktory_user_apps::dms::reports::close(&mut conn, &id, operator.trim(), now).map_err(|refused| refused.to_string())?;
        if !closed {
            return Err(format!("no open report {id}"));
        }
        println!("closed {id}");
        return Ok(());
    }
    let open = claktory_user_apps::dms::reports::review(&mut conn, &keys, operator.trim(), now).map_err(|refused| refused.to_string())?;
    for report in open {
        println!("{report}");
    }
    Ok(())
}

fn serve(args: Vec<String>) -> Result<(), String> {
    let options = Options::new(args, &["--db", "--dm-key", "--socket", "--root-uid"])?;
    let db = PathBuf::from(options.required("--db")?);
    let keys = Keys::load(&PathBuf::from(options.required("--dm-key")?)).map_err(|refused| refused.to_string())?;
    let socket = PathBuf::from(options.required("--socket")?);
    let root_uid: u32 = options.required("--root-uid")?.parse().map_err(|_| "--root-uid takes a number")?;
    let worker = Worker::open(&db, keys)?;
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
