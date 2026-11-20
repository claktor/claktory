use std::borrow::Cow;

use base64::Engine;
use rand::Rng;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};
use tokio_util::sync::CancellationToken;

use crate::auth::{AuthEvent, AuthInteraction, AuthPrompt, RequestAuth};
use crate::credentials::Credential;
use crate::errors::Error;

pub const CANCEL_MESSAGE: &str = "Login cancelled";

pub fn check_cancel(cancel: &CancellationToken) -> Result<(), Error> {
    if cancel.is_cancelled() { Err(Error::Other(CANCEL_MESSAGE.into())) } else { Ok(()) }
}

const LOGO_SVG: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 800 800" aria-hidden="true"><path fill="#fff" fill-rule="evenodd" d="M165.29 165.29 H517.36 V400 H400 V517.36 H282.65 V634.72 H165.29 Z M282.65 282.65 V400 H400 V282.65 Z"/><path fill="#fff" d="M517.36 400 H634.72 V634.72 H517.36 Z"/></svg>"##;

fn escape_html(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn render_page(title: &str, heading: &str, message: &str, details: Option<&str>) -> String {
    let details = details.map(|details| format!("<div class=\"details\">{}</div>", escape_html(details))).unwrap_or_default();
    format!(
        r#"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1" />
  <title>{title}</title>
  <style>
    :root {{
      --text: #fafafa;
      --text-dim: #a1a1aa;
      --page-bg: #09090b;
      --font-sans: ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, "Noto Sans", sans-serif, "Apple Color Emoji", "Segoe UI Emoji", "Segoe UI Symbol", "Noto Color Emoji";
      --font-mono: ui-monospace, SFMono-Regular, Menlo, Monaco, Consolas, "Liberation Mono", "Courier New", monospace;
    }}
    * {{ box-sizing: border-box; }}
    html {{ color-scheme: dark; }}
    body {{
      margin: 0;
      min-height: 100vh;
      display: flex;
      align-items: center;
      justify-content: center;
      padding: 24px;
      background: var(--page-bg);
      color: var(--text);
      font-family: var(--font-sans);
      text-align: center;
    }}
    main {{
      width: 100%;
      max-width: 560px;
      display: flex;
      flex-direction: column;
      align-items: center;
      justify-content: center;
    }}
    .logo {{
      width: 72px;
      height: 72px;
      display: block;
      margin-bottom: 24px;
    }}
    h1 {{
      margin: 0 0 10px;
      font-size: 28px;
      line-height: 1.15;
      font-weight: 650;
      color: var(--text);
    }}
    p {{
      margin: 0;
      line-height: 1.7;
      color: var(--text-dim);
      font-size: 15px;
    }}
    .details {{
      margin-top: 16px;
      font-family: var(--font-mono);
      font-size: 13px;
      color: var(--text-dim);
      white-space: pre-wrap;
      word-break: break-word;
    }}
  </style>
</head>
<body>
  <main>
    <div class="logo">{LOGO_SVG}</div>
    <h1>{heading}</h1>
    <p>{message}</p>
    {details}
  </main>
</body>
</html>"#,
        title = escape_html(title),
        heading = escape_html(heading),
        message = escape_html(message),
    )
}

pub fn success_html(message: &str) -> String {
    render_page("Signed in", "Signed in", message, None)
}

/// The page a callback shows before Claktory has exchanged the code, which
/// may still fail: it says the sign-in is being finished, not that it worked.
pub fn finishing_html(provider: &str) -> String {
    let message =
        format!("Claktory is finishing your {provider} sign-in. You can close this tab and go back to Claktory to see how it went.");
    render_page("Finishing sign-in", "Finishing sign-in", &message, None)
}

pub fn error_html(message: &str, details: Option<&str>) -> String {
    render_page("Couldn't sign in", "Couldn't sign in", message, details)
}

pub fn base64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    rand::rng().fill_bytes(&mut bytes);
    bytes
}

pub fn random_hex<const N: usize>() -> String {
    random_bytes::<N>().iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The first `name` query parameter, if any.
fn query_value<'a>(mut pairs: impl Iterator<Item = (Cow<'a, str>, Cow<'a, str>)>, name: &str) -> Option<String> {
    pairs.find(|(key, _)| key == name).map(|(_, value)| value.into_owned())
}

/// PKCE S256: a 32-byte verifier and its SHA-256 challenge, both base64url without padding.
pub fn generate_pkce() -> (String, String) {
    let verifier = base64url(&random_bytes::<32>());
    let challenge = base64url(&Sha256::digest(verifier.as_bytes()));
    (verifier, challenge)
}

/// A pasted authorization code, a `code#state` pair, a query string, or the full redirect URL.
pub fn parse_authorization_input(input: &str) -> (Option<String>, Option<String>) {
    let value = input.trim();
    if value.is_empty() {
        return (None, None);
    }
    if let Ok(url) = reqwest::Url::parse(value) {
        return (query_value(url.query_pairs(), "code"), query_value(url.query_pairs(), "state"));
    }
    if let Some((code, state)) = value.split_once('#') {
        return (Some(code.to_owned()), Some(state.to_owned()));
    }
    if value.contains("code=") {
        let find = |name| query_value(url::form_urlencoded::parse(value.as_bytes()), name);
        return (find("code"), find("state"));
    }
    (Some(value.to_owned()), None)
}

pub fn callback_host() -> String {
    std::env::var("PI_OAUTH_CALLBACK_HOST").ok().filter(|host| !host.trim().is_empty()).unwrap_or_else(|| "127.0.0.1".into())
}

pub async fn respond(socket: &mut TcpStream, status: u16, reason: &str, html: &str) {
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        html.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(html.as_bytes()).await;
    let _ = socket.shutdown().await;
}

async fn read_request(mut socket: TcpStream) -> Option<(TcpStream, String, String)> {
    let mut raw = Vec::new();
    let mut buffer = [0u8; 4096];
    while let Ok(read) = socket.read(&mut buffer).await {
        if read == 0 {
            break;
        }
        raw.extend_from_slice(&buffer[..read]);
        if raw.windows(4).any(|window| window == b"\r\n\r\n") {
            break;
        }
    }
    let head = String::from_utf8_lossy(&raw);
    let mut parts = head.lines().next()?.split(' ');
    let (method, target) = (parts.next()?.to_owned(), parts.next()?.to_owned());
    Some((socket, method, target))
}

/// Requests in arrival order; each connection reads its head on its own task, so one that never sends a request holds up nothing.
pub struct CallbackRequests {
    listener: TcpListener,
    heads: JoinSet<Option<(TcpStream, String, String)>>,
}

impl CallbackRequests {
    pub fn new(listener: TcpListener) -> CallbackRequests {
        CallbackRequests { listener, heads: JoinSet::new() }
    }

    /// The next request's socket, method and target; `None` once the listener fails.
    pub async fn next(&mut self) -> Option<(TcpStream, String, String)> {
        loop {
            tokio::select! {
                accepted = self.listener.accept() => {
                    let (socket, _) = accepted.ok()?;
                    self.heads.spawn(read_request(socket));
                }
                Some(read) = self.heads.join_next() => {
                    if let Ok(Some(request)) = read {
                        return Some(request);
                    }
                }
            }
        }
    }
}

pub struct FixedCallback {
    pub path: &'static str,
    pub success: String,
    pub internal: String,
    pub missing: String,
    pub oauth_error: Option<&'static str>,
    pub state_first: bool,
}

/// A callback server: the first code and state it receives (`None` when it
/// couldn't listen), and its task.
pub type Callback = (oneshot::Receiver<Option<(String, String)>>, JoinHandle<()>);

/// The port a callback listener was asked for, or the one it got when that
/// was 0 (any free port, as tests ask, so they never race for one).
pub fn bound_port(listener: &std::io::Result<TcpListener>, port: u16) -> u16 {
    match listener {
        Ok(listener) if port == 0 => listener.local_addr().map_or(port, |address| address.port()),
        _ => port,
    }
}

/// Serves a fixed-path OAuth callback until the returned task is aborted.
pub fn fixed_callback(listener: std::io::Result<TcpListener>, expected_state: String, config: FixedCallback) -> Callback {
    let (tx, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        let Ok(listener) = listener else {
            let _ = tx.send(None);
            return;
        };
        let mut settle = Some(tx);
        let mut requests = CallbackRequests::new(listener);
        while let Some((mut socket, _, target)) = requests.next().await {
            let Ok(url) = reqwest::Url::parse(&format!("http://localhost{target}")) else {
                respond(&mut socket, 500, "Internal Server Error", &config.internal).await;
                continue;
            };
            let query = |name| query_value(url.query_pairs(), name);
            if url.path() != config.path {
                respond(&mut socket, 404, "Not Found", &error_html("Callback route not found.", None)).await;
                continue;
            }
            if let (Some(message), Some(error)) = (config.oauth_error, query("error")) {
                respond(&mut socket, 400, "Bad Request", &error_html(message, Some(&format!("Error: {error}")))).await;
                continue;
            }
            let (code, state) = if config.state_first {
                let Some(state) = query("state").filter(|state| state == &expected_state) else {
                    respond(&mut socket, 400, "Bad Request", &error_html("State mismatch.", None)).await;
                    continue;
                };
                let Some(code) = query("code").filter(|code| !code.is_empty()) else {
                    respond(&mut socket, 400, "Bad Request", &config.missing).await;
                    continue;
                };
                (code, state)
            } else {
                let (Some(code), Some(state)) =
                    (query("code").filter(|code| !code.is_empty()), query("state").filter(|state| !state.is_empty()))
                else {
                    respond(&mut socket, 400, "Bad Request", &config.missing).await;
                    continue;
                };
                if state != expected_state {
                    respond(&mut socket, 400, "Bad Request", &error_html("State mismatch.", None)).await;
                    continue;
                }
                (code, state)
            };
            respond(&mut socket, 200, "OK", &config.success).await;
            if let Some(settle) = settle.take() {
                let _ = settle.send(Some((code, state)));
            }
        }
    });
    (rx, task)
}

/// Publishes an authorization URL and races its fixed callback against manual input and cancellation.
pub async fn browser_authorization(
    interaction: &dyn AuthInteraction,
    cancel: &CancellationToken,
    (callback, server): Callback,
    url: String,
    (instructions, message, placeholder): (&str, &str, String),
    expected_state: &str,
    state_mismatch: &str,
) -> Result<(String, String), Error> {
    interaction.notify(AuthEvent::AuthUrl { url, instructions: Some(instructions.into()) });
    let manual = interaction.prompt(AuthPrompt::ManualCode { message: message.into(), placeholder: Some(placeholder) });
    tokio::pin!(manual);
    let outcome = async {
        let mut callback = callback;
        let pasted = tokio::select! {
            received = &mut callback => match received {
                Ok(Some(result)) => return Ok(result),
                _ => manual.await?,
            },
            typed = &mut manual => typed?,
            () = cancel.cancelled() => return Err(Error::Aborted),
        };
        let (code, state) = parse_authorization_input(&pasted);
        if state.as_deref().is_some_and(|state| state != expected_state) {
            return Err(Error::Other(state_mismatch.into()));
        }
        let code = code.ok_or_else(|| Error::Other("Missing authorization code".into()))?;
        Ok((code, state.unwrap_or_else(|| expected_state.into())))
    }
    .await;
    server.abort();
    let _ = server.await;
    outcome
}

pub fn request_auth(credential: &Credential) -> RequestAuth {
    let key = match credential {
        Credential::Oauth { access, .. } => access,
        Credential::ApiKey { key, .. } => key,
    };
    RequestAuth { api_key: Some(key.clone()), ..RequestAuth::default() }
}

/// A fetch that the login's cancellation turns into Pi's "Login cancelled".
pub async fn send_cancellable(request: reqwest::RequestBuilder, cancel: &CancellationToken) -> Result<reqwest::Response, Error> {
    tokio::select! {
        sent = request.send() => sent.map_err(|error| if cancel.is_cancelled() { Error::Other(CANCEL_MESSAGE.into()) } else { Error::Other(error.to_string()) }),
        () = cancel.cancelled() => Err(Error::Other(CANCEL_MESSAGE.into())),
    }
}
