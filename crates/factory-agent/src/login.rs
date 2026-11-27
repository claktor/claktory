//! Interactive sign-in from a terminal. This is the one place the crate talks to the terminal directly;
//! prompts and progress go to stderr, answers come from stdin.

use std::io::Write;
use std::path::Path;
use std::process::Stdio;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthEvent, AuthFuture, AuthInteraction, AuthPrompt, SelectOption};
use crate::errors::{Error, models_error};
use crate::providers::models::Registry;

/// Signs in to `provider` (asking which one if `None`) and stores the credential in `auth_path`. Subscription
/// providers use their OAuth flows: a browser login with a localhost callback (the URL is opened and printed,
/// and a pasted redirect URL or code also works) or a device code. API-key providers prompt for the key
/// without echoing it. Ctrl-C cancels; tokio keeps its SIGINT handler installed afterwards, so call this from
/// a process that exits once the login is done.
pub async fn login_terminal(auth_path: &Path, provider: Option<&str>) -> Result<(), Error> {
    let registry = Registry::new(auth_path.to_path_buf());
    let terminal = Terminal { cancel: CancellationToken::new() };
    let provider_id = match provider {
        Some(id) => id.to_owned(),
        None => {
            let mut options = Vec::new();
            for provider in registry.providers().iter().filter(|provider| !crate::credentials::VAULT_ONLY.contains(&provider.id)) {
                options.push(SelectOption { id: provider.id.to_owned(), label: provider.name.to_owned(), description: None });
            }
            terminal.prompt(AuthPrompt::Select { message: "Sign in to:".into(), options }).await?
        }
    };
    let provider =
        registry.provider(&provider_id).ok_or_else(|| models_error("provider", format!("Unknown provider: {provider_id}"), None))?;
    // This writes `auth.json`, which never holds them.
    if crate::credentials::VAULT_ONLY.contains(&provider.id) {
        return Err(models_error(
            "provider",
            format!("Sign in to {} in Claktory, under Settings → Providers: its sign-in is kept only with your secrets.", provider.name),
            None,
        ));
    }
    let kind = match (&provider.oauth, &provider.api_key) {
        (Some(oauth), Some(api_key)) => {
            let options = vec![
                SelectOption { id: "oauth".into(), label: oauth.name().to_owned(), description: None },
                SelectOption { id: "api_key".into(), label: api_key.name().to_owned(), description: None },
            ];
            terminal.prompt(AuthPrompt::Select { message: format!("Sign in to {} with:", provider.name), options }).await?
        }
        (Some(_), None) => "oauth".to_owned(),
        (None, _) => "api_key".to_owned(),
    };
    let name = provider.name;
    let login = registry.login(&provider_id, &kind, &terminal);
    tokio::pin!(login);
    loop {
        tokio::select! {
            outcome = &mut login => {
                outcome?;
                break;
            }
            signal = tokio::signal::ctrl_c() => {
                signal?;
                terminal.cancel.cancel();
            }
        }
    }
    say(&format!("Signed in to {name}."));
    Ok(())
}

struct Terminal {
    cancel: CancellationToken,
}

impl AuthInteraction for Terminal {
    fn prompt<'a>(&'a self, prompt: AuthPrompt) -> AuthFuture<'a, Result<String, Error>> {
        Box::pin(async move {
            match prompt {
                AuthPrompt::Select { message, options } => {
                    let mut menu = message;
                    for (index, option) in options.iter().enumerate() {
                        menu.push_str(&format!("\n  {}. {}", index + 1, option.label));
                        if let Some(description) = &option.description {
                            menu.push_str(&format!(" ({description})"));
                        }
                    }
                    say(&menu);
                    loop {
                        let answer = read_line(&format!("Choose 1-{}: ", options.len()), false).await?;
                        let chosen = answer.trim().parse::<usize>().ok().and_then(|number| number.checked_sub(1));
                        if let Some(option) = chosen.and_then(|index| options.get(index)) {
                            return Ok(option.id.clone());
                        }
                    }
                }
                AuthPrompt::Secret { message, .. } => read_line(&format!("{message}: "), true).await,
                AuthPrompt::Text { message, placeholder } | AuthPrompt::ManualCode { message, placeholder } => {
                    let hint = placeholder.map(|placeholder| format!(" ({placeholder})")).unwrap_or_default();
                    read_line(&format!("{message}{hint}: "), false).await
                }
            }
        })
    }

    fn notify(&self, event: AuthEvent) {
        match event {
            AuthEvent::AuthUrl { url, instructions } => {
                say(&format!("\nOpen this URL to sign in:\n{url}"));
                if let Some(instructions) = instructions.filter(|text| !text.is_empty()) {
                    say(&instructions);
                }
                open_browser(&url);
            }
            AuthEvent::DeviceCode { user_code, verification_uri, expires_in_seconds, .. } => {
                say(&format!("\nOpen {verification_uri} and enter code {user_code}"));
                if let Some(seconds) = expires_in_seconds.filter(|seconds| *seconds > 0) {
                    say(&format!("The code expires in {} minutes.", seconds.div_ceil(60)));
                }
                open_browser(&verification_uri);
            }
            AuthEvent::Info { message, links } => {
                say(&message);
                for link in links {
                    match link.label.filter(|label| !label.is_empty()) {
                        Some(label) => say(&format!("  {label}: {}", link.url)),
                        None => say(&format!("  {}", link.url)),
                    }
                }
            }
            AuthEvent::Progress { message } => say(&message),
        }
    }

    fn cancel(&self) -> CancellationToken {
        self.cancel.clone()
    }
}

/// Terminal output; a closed stderr leaves nothing to report the failure to.
fn say(text: &str) {
    let _ = writeln!(std::io::stderr(), "{text}");
}

/// Reads one line from stdin, with echo off for a secret. A read cut short by the browser callback stays pending
/// on tokio's stdin thread until a line arrives or the process exits.
async fn read_line(prompt: &str, secret: bool) -> Result<String, Error> {
    let mut stderr = std::io::stderr();
    write!(stderr, "{prompt}")?;
    stderr.flush()?;
    let echo = if secret { EchoOff::new() } else { None };
    let mut line = String::new();
    let read = BufReader::new(tokio::io::stdin()).read_line(&mut line).await;
    if echo.is_some() {
        drop(echo);
        writeln!(stderr)?;
    }
    if read? == 0 {
        return Err(Error::Other("stdin closed before an answer was entered".into()));
    }
    Ok(line.trim_end_matches(['\r', '\n']).to_owned())
}

#[cfg(windows)]
use crate::windows::console::EchoOff;

/// Turns terminal echo off until dropped. `None` when stdin is not a terminal.
#[cfg(unix)]
struct EchoOff(libc::termios);

#[cfg(unix)]
impl EchoOff {
    fn new() -> Option<EchoOff> {
        // SAFETY: plain termios calls on stdin with a zeroed, correctly sized termios that tcgetattr fills.
        unsafe {
            if libc::isatty(libc::STDIN_FILENO) != 1 {
                return None;
            }
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(libc::STDIN_FILENO, &mut original) != 0 {
                return None;
            }
            let mut silent = original;
            silent.c_lflag &= !libc::ECHO;
            if libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &silent) != 0 {
                return None;
            }
            Some(EchoOff(original))
        }
    }
}

#[cfg(unix)]
impl Drop for EchoOff {
    fn drop(&mut self) {
        // SAFETY: restores the attributes read from the same descriptor in `new`.
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}

/// Opens `url` in the default browser where there is a known opener; the URL is printed either way.
fn open_browser(url: &str) {
    let opener = match std::env::consts::OS {
        "macos" => "open",
        "linux" => "xdg-open",
        _ => return,
    };
    // tokio reaps the dropped child in the background.
    let spawned = tokio::process::Command::new(opener).arg(url).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn();
    if let Err(error) = spawned {
        tracing::debug!(opener, %error, "couldn't open the browser");
    }
}
