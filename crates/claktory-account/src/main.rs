//! `claktory-account`: Claktory's account service, as one binary.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use claktory_account::server::{self, Config, Mailer, Secrets, Service, mail};

const USAGE: &str = "\
claktory-account: Claktory's account service

Usage:
  claktory-account [serve] --db FILE --secrets FILE [options]
  claktory-account init-secrets FILE
  claktory-account rotate-secrets FILE [--signing-key] [--k-share]
  claktory-account stats --db FILE
  claktory-account backup --db FILE --to DIR [--keep N]
  claktory-account set-cloudflare-token FILE (the token on standard input)
  claktory-account set-resend-key FILE      (the key on standard input)

serve options:
  --listen ADDRESS        where to listen (127.0.0.1:8080)
  --trust-proxy           behind a TLS proxy such as Caddy: the client's IP is
                          the last of X-Forwarded-For, and cookies are Secure
  --email PROVIDER        how emails go: cloudflare, resend or folder. Left
                          out: Cloudflare when the secrets file has
                          cloudflare_api_token, else Resend when it has
                          resend_api_key, else the folder of --mail-dir
  --cloudflare-account ID the Cloudflare account that sends (Cloudflare Email
                          Sending, its domain onboarded there)
  --mail-dir DIR          write emails to files in DIR instead of sending them
  --mail-domain DOMAIN    emails come from code@ and notices@ it (auth.claktory.com)
  --account-page TEXT     the account page, as emails write it (claktory.com/account)
  --email-daily-cap N     emails a day at most (90); codes stop there
  --admin-email ADDRESS   who runs the service: told when 80% of the day's
                          emails are used, and shown the usage counts at
                          /account/admin; repeat for more
  --geoip FILE            a GeoIP city file (.mmdb: DB-IP's IP to City Lite, or
                          GeoLite2 City) to know roughly where Claktories are
  --relay URL             a relay for Claktories to use; repeat for more

init-secrets writes new keys to FILE (never over one): back up its k_share and
signing_key offline.

rotate-secrets replaces the keys named (at least one), keeping the rest, after
copying FILE as it was to FILE.before-<time>. A new signing key: every
Claktory signs in again, and its secrets open by themselves after. A new
k_share: every Claktory's secrets open only with its recovery key, once.
Restart the service to use them. To send email, set-cloudflare-token puts a Cloudflare API
token with Email Sending: Edit in it (or set-resend-key a Resend key).

backup writes a consistent copy of the database to DIR, while it's in use, and
keeps the newest N copies (14).
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
        "init-secrets" => init_secrets(&args),
        "rotate-secrets" => rotate_secrets(&args),
        "stats" => stats(&args),
        "backup" => backup(&args),
        "set-resend-key" => set_resend_key(&args),
        "set-cloudflare-token" => set_cloudflare_token(&args),
        "help" => {
            print!("{USAGE}");
            Ok(())
        }
        other => Err(format!("unknown command {other}\n\n{USAGE}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("claktory-account: {error}");
            ExitCode::FAILURE
        }
    }
}

/// Options as `--name value` pairs and `--flag`s.
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

    fn values(&self, name: &str) -> Vec<String> {
        self.0.windows(2).filter(|pair| pair[0] == name).map(|pair| pair[1].clone()).collect()
    }

    fn flag(&self, name: &str) -> bool {
        self.0.iter().any(|arg| arg == name)
    }

    fn required(&self, name: &str) -> Result<String, String> {
        self.value(name)?.ok_or(format!("{name} is needed\n\n{USAGE}"))
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

fn serve(args: &[String]) -> Result<(), String> {
    let options = Options(args.to_vec());
    options.check(
        &[
            "--db",
            "--secrets",
            "--listen",
            "--email",
            "--cloudflare-account",
            "--mail-dir",
            "--mail-domain",
            "--account-page",
            "--email-daily-cap",
            "--admin-email",
            "--relay",
            "--geoip",
        ],
        &["--trust-proxy"],
    )?;
    let db = PathBuf::from(options.required("--db")?);
    let secrets = Secrets::load(&PathBuf::from(options.required("--secrets")?))?;
    let listen = options.value("--listen")?.unwrap_or_else(|| "127.0.0.1:8080".to_owned());
    let mut config = Config { trust_proxy: options.flag("--trust-proxy"), relays: options.values("--relay"), ..Config::default() };
    if let Some(domain) = options.value("--mail-domain")? {
        config.mail_domain = domain;
    }
    if let Some(page) = options.value("--account-page")? {
        config.account_page = page;
    }
    if let Some(cap) = options.value("--email-daily-cap")? {
        config.email_daily_cap = cap.parse().map_err(|_| "--email-daily-cap takes a number")?;
    }
    config.admins = options.values("--admin-email");
    if let Some(path) = options.value("--geoip")? {
        let path = PathBuf::from(path);
        if path.exists() {
            config.locator = Some(Arc::new(server::geo::GeoIp::open(&path)?));
        } else {
            tracing::warn!("no GeoIP file at {} yet: places stay unknown until it's there and the service restarts", path.display());
        }
    }
    let mailer = mailer(&options, &secrets)?;
    if secrets.relay_bearer.is_none() {
        tracing::info!("no relay_bearer in the secrets file: the relay allowlist is off");
    }
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build().map_err(|error| error.to_string())?;
    runtime.block_on(async move {
        let service = Service::new(&db, secrets, config, mailer, server::system_clock())?;
        let listener = tokio::net::TcpListener::bind(&listen).await.map_err(|error| format!("can't listen on {listen}: {error}"))?;
        tracing::info!(
            "listening on {}, service key {}",
            listener.local_addr().map_or(listen.clone(), |address| address.to_string()),
            service.service_key()
        );
        service.spawn_upkeep();
        tokio::select! {
            result = server::serve(listener, service) => result.map_err(|error| error.to_string()),
            _ = shutdown() => {
                tracing::info!("stopping");
                Ok(())
            }
        }
    })
}

/// How emails go: as `--email` says, or by what's configured.
fn mailer(options: &Options, secrets: &Secrets) -> Result<Arc<dyn Mailer>, String> {
    let dir = options.value("--mail-dir")?;
    let account = options.value("--cloudflare-account")?;
    let provider = match options.value("--email")? {
        Some(provider) => provider,
        None if secrets.cloudflare_api_token.is_some() => "cloudflare".to_owned(),
        None if secrets.resend_api_key.is_some() => "resend".to_owned(),
        None if dir.is_some() => "folder".to_owned(),
        None => {
            return Err(
                "no way to send email: put a Cloudflare API token in the secrets file (set-cloudflare-token) and give --cloudflare-account, or give --mail-dir"
                    .to_owned(),
            );
        }
    };
    let mailer: Arc<dyn Mailer> = match provider.as_str() {
        "cloudflare" => {
            let token = secrets
                .cloudflare_api_token
                .clone()
                .ok_or("--email cloudflare needs cloudflare_api_token in the secrets file (set-cloudflare-token)")?;
            let account = account.ok_or("--email cloudflare needs --cloudflare-account, the account's id")?;
            tracing::info!("emails go through Cloudflare Email Sending");
            Arc::new(mail::Cloudflare::new(token, &account))
        }
        "resend" => {
            let key = secrets.resend_api_key.clone().ok_or("--email resend needs resend_api_key in the secrets file (set-resend-key)")?;
            tracing::info!("emails go through Resend");
            Arc::new(mail::Resend::new(key))
        }
        "folder" => {
            let dir = dir.ok_or("--email folder needs --mail-dir")?;
            tracing::info!("emails are written to {dir}, not sent");
            Arc::new(mail::Folder::new(PathBuf::from(&dir)).map_err(|error| format!("can't use {dir}: {error}"))?)
        }
        other => return Err(format!("--email is cloudflare, resend or folder, not {other}")),
    };
    Ok(mailer)
}

async fn shutdown() {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()).expect("a signal handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

fn init_secrets(args: &[String]) -> Result<(), String> {
    let [path] = args else { return Err(format!("init-secrets takes the file to write\n\n{USAGE}")) };
    let secrets = Secrets::create(&PathBuf::from(path))?;
    println!("Wrote {path}. The service key is {}.", claktory_account::protocol::b64(secrets.signing_key.verifying_key().to_bytes()));
    println!("Back up its k_share and signing_key offline: without them no Claktory can unlock after a restart.");
    Ok(())
}

fn rotate_secrets(args: &[String]) -> Result<(), String> {
    let mut path = None;
    let mut rotate = claktory_account::server::Rotate::default();
    for arg in args {
        match arg.as_str() {
            "--signing-key" => rotate.signing_key = true,
            "--k-share" => rotate.k_share = true,
            flag if flag.starts_with('-') => return Err(format!("unknown option {flag}\n\n{USAGE}")),
            file if path.is_none() => path = Some(PathBuf::from(file)),
            _ => return Err(format!("rotate-secrets takes one file\n\n{USAGE}")),
        }
    }
    let Some(path) = path else { return Err(format!("rotate-secrets takes the secrets file\n\n{USAGE}")) };
    if !rotate.signing_key && !rotate.k_share {
        return Err(format!("say which key to rotate: --signing-key, --k-share, or both\n\n{USAGE}"));
    }
    let (secrets, before) = Secrets::rotate(&path, rotate, jiff::Timestamp::now().as_second())?;
    let mut said = vec![format!(
        "Rotated {}.",
        if rotate.signing_key && rotate.k_share {
            "the signing key and k_share"
        } else if rotate.signing_key {
            "the signing key"
        } else {
            "k_share"
        }
    )];
    said.push(format!("The file as it was is {}: delete it once the service runs with the new keys.", before.display()));
    if rotate.signing_key {
        said.push(format!(
            "The service key is now {}. Every Claktory signs in again; their secrets open by themselves after.",
            claktory_account::protocol::b64(secrets.signing_key.verifying_key().to_bytes())
        ));
    }
    if rotate.k_share {
        said.push("Every Claktory's secrets now open only with its recovery key, once.".to_owned());
    }
    said.push("Back up the new keys offline, then restart the service.".to_owned());
    println!("{}", said.join("\n"));
    Ok(())
}

fn stats(args: &[String]) -> Result<(), String> {
    let options = Options(args.to_vec());
    options.check(&["--db"], &[])?;
    let db = PathBuf::from(options.required("--db")?);
    let conn = rusqlite::Connection::open_with_flags(&db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|error| format!("can't open {}: {error}", db.display()))?;
    let now = jiff::Timestamp::now().as_second();
    print!("{}", server::stats::report(&conn, now).map_err(|error| error.to_string())?);
    Ok(())
}

fn backup(args: &[String]) -> Result<(), String> {
    let options = Options(args.to_vec());
    options.check(&["--db", "--to", "--keep"], &[])?;
    let db = PathBuf::from(options.required("--db")?);
    let dir = PathBuf::from(options.required("--to")?);
    let keep = match options.value("--keep")? {
        Some(keep) => keep.parse().ok().filter(|keep| *keep > 0).ok_or("--keep takes a number above 0")?,
        None => 14,
    };
    let written = server::db::backup(&db, &dir, keep, jiff::Timestamp::now().as_second())?;
    println!("Wrote {}", written.display());
    Ok(())
}

fn set_cloudflare_token(args: &[String]) -> Result<(), String> {
    let [path] = args else {
        return Err(format!("set-cloudflare-token takes the secrets file, and the token on standard input\n\n{USAGE}"));
    };
    let mut token = zeroize::Zeroizing::new(String::new());
    std::io::stdin().read_line(&mut token).map_err(|error| error.to_string())?;
    Secrets::set_cloudflare_token(&PathBuf::from(path), token.trim())?;
    println!("Set the Cloudflare token in {path}. Restart the service to use it.");
    Ok(())
}

fn set_resend_key(args: &[String]) -> Result<(), String> {
    let [path] = args else { return Err(format!("set-resend-key takes the secrets file, and the key on standard input\n\n{USAGE}")) };
    let mut key = zeroize::Zeroizing::new(String::new());
    std::io::stdin().read_line(&mut key).map_err(|error| error.to_string())?;
    Secrets::set_resend_key(&PathBuf::from(path), key.trim())?;
    println!("Set the Resend key in {path}. Restart the service to use it.");
    Ok(())
}
