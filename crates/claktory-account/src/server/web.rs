//! claktory.com/account: sign in with an emailed code, see your Claktories
//! and their recent unlocks, remove one, download your data or delete the
//! account. Plain HTML forms, no scripts.

use hyper::header::{HeaderName, HeaderValue};
use hyper::{Method, StatusCode};

use super::api::{check_code, new_code, sign_in_limits, start};
use super::mail::{escape, format_time};
use super::{ApiError, Call, Reply, Service, db};
use crate::protocol::{b64, normalize_email, random_32};

const COOKIE: &str = "claktory_account";
/// How long a web session lasts, in seconds.
const SESSION: i64 = 3600;

pub(super) async fn page(service: &Service, call: &Call) -> Reply {
    let form = Form::parse(call);
    let result = match (&call.method, call.path.as_str()) {
        (&Method::GET, "/account") => Ok(home(service, call, None)),
        (&Method::POST, "/account/sign-in") => sign_in(service, call, &form).await,
        (&Method::POST, "/account/verify") => verify(service, call, &form),
        (&Method::POST, "/account/remove") => remove(service, call, &form),
        (&Method::POST, "/account/delete") => delete(service, call, &form),
        (&Method::POST, "/account/sign-out") => Ok(sign_out(service, call)),
        (&Method::GET, "/account/data") => data(service, call),
        (&Method::GET, "/account/admin") => Ok(admin(service, call)),
        _ => Ok(html(
            StatusCode::NOT_FOUND,
            "Page not found",
            "<p>There's nothing here.</p><p><a href=\"/account\">Go to your account</a></p>",
        )),
    };
    result.unwrap_or_else(|error| match error.body.error.as_str() {
        "rate_limited" => html(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many tries",
            "<p>Wait a few minutes, then try again.</p><p><a href=\"/account\">Go to your account</a></p>",
        ),
        _ => html(
            error.status,
            "Something went wrong",
            "<p>Try again in a few minutes.</p><p><a href=\"/account\">Go to your account</a></p>",
        ),
    })
}

/// A form's fields.
struct Form(Vec<(String, String)>);

impl Form {
    fn parse(call: &Call) -> Form {
        Form(form_urlencoded::parse(&call.body).into_owned().collect())
    }

    fn get(&self, name: &str) -> &str {
        self.0.iter().find(|(key, _)| key == name).map_or("", |(_, value)| value.as_str())
    }
}

/// The signed-in person's email and CSRF token.
fn session(service: &Service, call: &Call) -> Option<(String, String, String)> {
    let token = cookie(call)?;
    let (email, csrf) = db::session(&service.db(), &token, service.now()).ok()??;
    Some((token, email, csrf))
}

fn cookie(call: &Call) -> Option<String> {
    call.headers
        .get_all(hyper::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == COOKIE)
        .map(|(_, value)| value.to_owned())
}

/// A session that's signed in and sent its form's CSRF token.
fn checked_session(service: &Service, call: &Call, form: &Form) -> Option<String> {
    let (_, email, csrf) = session(service, call)?;
    (!csrf.is_empty() && form.get("csrf") == csrf).then_some(email)
}

fn home(service: &Service, call: &Call, notice: Option<&str>) -> Reply {
    let notice = notice.map(|text| format!("<p class=\"notice\">{}</p>", escape(text))).unwrap_or_default();
    let Some((_, email, csrf)) = session(service, call) else {
        return sign_in_page(&notice, "");
    };
    let conn = service.db();
    let account = db::account_by_email(&conn, &email).ok().flatten();
    let mut body = format!("{notice}<p class=\"muted\">Signed in as {}</p>", escape(&email));
    let claktories: Vec<db::Claktory> = account
        .as_ref()
        .and_then(|account| db::claktories(&conn, &account.id).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|claktory| claktory.revoked_at.is_none())
        .collect();
    if claktories.is_empty() {
        body.push_str("<p>No Claktory uses this account yet.</p>");
    }
    for claktory in &claktories {
        let unlocks = db::releases(&conn, &claktory.id, 5).unwrap_or_default();
        let unlocks = if unlocks.is_empty() {
            "<p class=\"muted\">No unlocks in the last 30 days.</p>".to_owned()
        } else {
            let items: String =
                unlocks.iter().map(|(time, ip)| format!("<li>{} from {}</li>", escape(&format_time(*time)), escape(ip))).collect();
            format!("<p class=\"muted\">Unlocked its secrets after a restart:</p><ul>{items}</ul>")
        };
        let sent = sent(&conn, claktory, service.now());
        body.push_str(&format!(
            "<section><h2>{name}</h2><p class=\"muted\">{version} on {platform} · last contact {seen}</p>{sent}{unlocks}\
             <form method=\"post\" action=\"/account/remove\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
             <input type=\"hidden\" name=\"claktory\" value=\"{id}\"><button>Remove</button></form></section>",
            name = escape(&claktory.name),
            version = escape(&claktory.version),
            platform = escape(&platform_name(&claktory.platform)),
            seen = escape(&format_time(claktory.last_seen)),
            id = escape(&claktory.id),
            csrf = escape(&csrf),
        ));
    }
    if service.config.admins.contains(&email) {
        body.push_str("<p><a href=\"/account/admin\">Usage counts</a></p>");
    }
    if service.config.locator.is_some() && claktories.iter().any(|claktory| claktory.location.short().is_some()) {
        body.push_str(CREDIT);
    }
    body.push_str(&format!(
        "<section class=\"actions\"><p><a href=\"/account/data\">Download my data</a></p>\
         <form method=\"post\" action=\"/account/sign-out\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\"><button class=\"plain\">Sign out</button></form>\
         {delete}</section>",
        csrf = escape(&csrf),
        delete = if account.is_some() {
            format!(
                "<form method=\"post\" action=\"/account/delete\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button class=\"danger\">Delete account…</button></form>",
                escape(&csrf)
            )
        } else {
            String::new()
        },
    ));
    html(StatusCode::OK, "Your Claktory account", &body)
}

/// DB-IP's free database asks for this wherever its places are shown.
const CREDIT: &str = "<p class=\"muted\"><a href=\"https://db-ip.com\">IP Geolocation by DB-IP</a></p>";

/// What a Claktory sends at its check-ins, as the account page shows it.
fn sent(conn: &rusqlite::Connection, claktory: &db::Claktory, now: i64) -> String {
    let mut out = String::new();
    if let Some(place) = claktory.location.short() {
        out.push_str(&format!("<p class=\"muted\">Near {} (from its IP address)</p>", escape(&place)));
    }
    if let Some(counts) = claktory.counts {
        out.push_str(&format!(
            "<p class=\"muted\">{} · {} · {}</p>",
            plural(counts.agents.into(), "agent"),
            plural(counts.projects.into(), "project"),
            plural(counts.devices.into(), "device")
        ));
    }
    let minutes = db::agent_minutes_since(conn, &claktory.id, now - now.rem_euclid(3600) - 7 * 86_400).unwrap_or(0);
    if minutes > 0 {
        let hours = (minutes + 30) / 60;
        out.push_str(&format!("<p class=\"muted\">Agents worked {} in the last 7 days.</p>", plural(hours, "hour")));
    }
    let usage = db::usage_since(conn, &claktory.id, &db::day(now - 29 * 86_400)).unwrap_or_default();
    if !usage.is_empty() {
        let items: String = usage
            .iter()
            .map(|(model, requests, tokens)| {
                format!("<li>{}: {} tokens in {}</li>", escape(model), rounded(*tokens), plural(*requests, "request"))
            })
            .collect();
        out.push_str(&format!("<p class=\"muted\">Usage in the last 30 days:</p><ul>{items}</ul>"));
    }
    out
}

/// A token count as the app writes it: 950, 840k, 1.2M, 3.4B.
fn rounded(count: i64) -> String {
    let short = |value: f64, unit: &str| {
        let text = if value < 10.0 { format!("{value:.1}") } else { format!("{value:.0}") };
        format!("{}{unit}", text.strip_suffix(".0").unwrap_or(&text))
    };
    if count < 1_000 {
        count.to_string()
    } else if count < 999_500 {
        format!("{}k", (count as f64 / 1e3).round())
    } else if count < 999_950_000 {
        short(count as f64 / 1e6, "M")
    } else {
        short(count as f64 / 1e9, "B")
    }
}

/// "1 agent", "1,234 tokens".
fn plural(count: i64, noun: &str) -> String {
    if count == 1 { format!("1 {noun}") } else { format!("{} {noun}s", super::stats::grouped(count)) }
}

/// The usage counts, for whoever runs the service.
fn admin(service: &Service, call: &Call) -> Reply {
    let Some((_, email, _)) = session(service, call).filter(|(_, email, _)| service.config.admins.contains(email)) else {
        return home(service, call, None);
    };
    let report = match super::stats::report(&service.db(), service.now()) {
        Ok(report) => report,
        Err(error) => {
            return html(StatusCode::INTERNAL_SERVER_ERROR, "Something went wrong", &format!("<p>{}</p>", escape(&error.to_string())));
        }
    };
    let credit = if service.config.locator.is_some() { CREDIT } else { "" };
    html(
        StatusCode::OK,
        "Usage counts",
        &format!(
            "<p class=\"muted\">Signed in as {}. <a href=\"/account\">Your account</a></p><section><pre>{}</pre></section>{credit}",
            escape(&email),
            escape(&report)
        ),
    )
}

fn sign_in_page(notice: &str, email: &str) -> Reply {
    html(
        StatusCode::OK,
        "Sign in to Claktory",
        &format!(
            "{notice}<p>Enter your email address. You'll get a code to sign in.</p>\
             <form method=\"post\" action=\"/account/sign-in\"><input type=\"email\" name=\"email\" value=\"{}\" placeholder=\"Email address\" required autofocus>\
             <button>Send code</button></form>",
            escape(email)
        ),
    )
}

/// A code that can't be used any more: a new one to the same address.
fn new_code_page(email: &str, notice: &str) -> Reply {
    if normalize_email(email).is_none() {
        return sign_in_page(&format!("<p class=\"notice\">{}</p>", escape(notice)), "");
    }
    html(
        StatusCode::OK,
        "Sign in to Claktory",
        &format!(
            "<p class=\"notice\">{}</p><form method=\"post\" action=\"/account/sign-in\"><input type=\"hidden\" name=\"email\" value=\"{}\">\
             <button>Send new code</button></form><p><a href=\"/account\">Use a different email</a></p>",
            escape(notice),
            escape(email)
        ),
    )
}

fn code_page(email: &str, request_id: &str, notice: &str) -> Reply {
    html(
        StatusCode::OK,
        "Check your email",
        &format!(
            "{notice}<p>Enter the 6-digit code sent to {email}. It works for 15 minutes.</p>\
             <form method=\"post\" action=\"/account/verify\"><input type=\"hidden\" name=\"request\" value=\"{request}\">\
             <input type=\"hidden\" name=\"email\" value=\"{email}\">\
             <input name=\"code\" inputmode=\"numeric\" autocomplete=\"one-time-code\" pattern=\"[0-9]{{6}}\" maxlength=\"6\" placeholder=\"Code\" required autofocus>\
             <button>Continue</button></form>\
             <form method=\"post\" action=\"/account/sign-in\"><input type=\"hidden\" name=\"email\" value=\"{email}\"><button class=\"plain\">Resend code</button></form>\
             <p><a href=\"/account\">Use a different email</a></p>",
            email = escape(email),
            request = escape(request_id),
        ),
    )
}

/// A platform as people read it: "macos-arm64" is "macOS (Apple silicon)".
fn platform_name(platform: &str) -> String {
    match platform {
        "macos-arm64" | "macos-aarch64" => "macOS (Apple silicon)",
        "macos-x86_64" => "macOS (Intel)",
        "linux-x86_64" => "Linux (x86_64)",
        "linux-aarch64" | "linux-arm64" => "Linux (ARM)",
        other => other,
    }
    .to_owned()
}

async fn sign_in(service: &Service, call: &Call, form: &Form) -> Result<Reply, ApiError> {
    let Some(email) = normalize_email(form.get("email")) else {
        return Ok(sign_in_page("<p class=\"notice\">Enter an email address, like you@example.com.</p>", form.get("email")));
    };
    sign_in_limits(service, &email, &call.ip)?;
    let request = db::Request {
        id: String::new(),
        kind: "web".to_owned(),
        email: email.clone(),
        claktory_id: None,
        name: None,
        backend_key: None,
        version: None,
        platform: None,
        code_hash: String::new(),
        attempts: 0,
        expires: 0,
    };
    match start(service, request, &new_code()).await {
        Ok(request_id) => Ok(code_page(&email, &request_id, "")),
        Err(error) if error.body.error == "email_unavailable" => {
            Ok(sign_in_page("<p class=\"notice\">Codes can't be sent right now. Try again in a few minutes.</p>", &email))
        }
        Err(error) => Err(error),
    }
}

fn verify(service: &Service, _call: &Call, form: &Form) -> Result<Reply, ApiError> {
    let email = form.get("email");
    let request = match check_code(service, form.get("request"), form.get("code"), "web") {
        Ok(request) => request,
        Err(error) if error.body.error == "wrong_code" => {
            let left = error.body.attempts_left.unwrap_or(0);
            if left == 0 {
                return Ok(new_code_page(email, "Too many tries. Get a new code."));
            }
            let tries = if left == 1 { "1 try".to_owned() } else { format!("{left} tries") };
            return Ok(code_page(email, form.get("request"), &format!("<p class=\"notice\">That code isn't right. {tries} left.</p>")));
        }
        Err(error) if error.body.error == "code_expired" => {
            return Ok(new_code_page(email, "That code has expired. Get a new one."));
        }
        Err(error) => return Err(error),
    };
    let token = b64(*random_32());
    let csrf = b64(&random_32()[..16]);
    {
        let conn = service.db();
        db::delete_request(&conn, &request.id)?;
        db::insert_session(&conn, &token, &request.email, &csrf, service.now() + SESSION)?;
    }
    let secure = if service.config.trust_proxy { "; Secure" } else { "" };
    let cookie = format!("{COOKIE}={token}; Path=/account; HttpOnly; SameSite=Strict; Max-Age={SESSION}{secure}");
    Ok(redirect(Some(cookie)))
}

fn remove(service: &Service, call: &Call, form: &Form) -> Result<Reply, ApiError> {
    let Some(email) = checked_session(service, call, form) else { return Ok(home(service, call, None)) };
    let conn = service.db();
    let account = db::account_by_email(&conn, &email)?;
    let claktory = db::claktory(&conn, form.get("claktory"))?
        .filter(|claktory| claktory.revoked_at.is_none() && account.as_ref().is_some_and(|account| account.id == claktory.account_id));
    let (Some(account), Some(claktory)) = (account, claktory) else {
        drop(conn);
        return Ok(home(service, call, None));
    };
    if form.get("confirm") != "yes" {
        return Ok(html(
            StatusCode::OK,
            &format!("Remove {}?", claktory.name),
            &format!(
                "<p>It won't be able to unlock its secrets after a restart until you sign in on it again.</p>\
                 <form method=\"post\" action=\"/account/remove\"><input type=\"hidden\" name=\"csrf\" value=\"{csrf}\">\
                 <input type=\"hidden\" name=\"claktory\" value=\"{id}\"><input type=\"hidden\" name=\"confirm\" value=\"yes\">\
                 <button class=\"danger\">Remove</button> <a href=\"/account\">Cancel</a></form>",
                csrf = escape(form.get("csrf")),
                id = escape(&claktory.id),
            ),
        ));
    }
    db::revoke(&conn, &account.id, &claktory.id, service.now())?;
    tracing::info!("removed: Claktory {} of account {}", claktory.id, account.id);
    Ok(redirect(None))
}

fn delete(service: &Service, call: &Call, form: &Form) -> Result<Reply, ApiError> {
    let Some(email) = checked_session(service, call, form) else { return Ok(home(service, call, None)) };
    let Some(account) = db::account_by_email(&service.db(), &email)? else { return Ok(home(service, call, None)) };
    if form.get("confirm") != "yes" {
        return Ok(html(
            StatusCode::OK,
            "Delete your account?",
            &format!(
                "<p>This deletes your email and everything linked to it. Your Claktories keep running, but after a restart \
                 they can unlock their secrets only with their recovery key, until you sign in on them again.</p>\
                 <form method=\"post\" action=\"/account/delete\"><input type=\"hidden\" name=\"csrf\" value=\"{}\">\
                 <input type=\"hidden\" name=\"confirm\" value=\"yes\"><button class=\"danger\">Delete account</button> <a href=\"/account\">Cancel</a></form>",
                escape(form.get("csrf"))
            ),
        ));
    }
    db::delete_account(&mut service.db(), &account)?;
    tracing::info!("deleted: account {}", account.id);
    let mut reply = sign_in_page("<p class=\"notice\">Your account was deleted.</p>", "");
    reply.headers.push(clear_cookie());
    Ok(reply)
}

fn sign_out(service: &Service, call: &Call) -> Reply {
    if let Some(token) = cookie(call) {
        let _ = db::delete_session(&service.db(), &token);
    }
    let mut reply = redirect(None);
    reply.headers.push(clear_cookie());
    reply
}

fn data(service: &Service, call: &Call) -> Result<Reply, ApiError> {
    let Some((_, email, _)) = session(service, call) else { return Ok(redirect(None)) };
    let conn = service.db();
    let data = match db::account_by_email(&conn, &email)? {
        Some(account) => db::account_data(&conn, &account)?,
        None => serde_json::json!({"account": null, "claktories": []}),
    };
    let mut reply = Reply::json(&data);
    reply.headers.push((hyper::header::CONTENT_DISPOSITION, HeaderValue::from_static("attachment; filename=\"claktory-account.json\"")));
    Ok(reply)
}

fn clear_cookie() -> (HeaderName, HeaderValue) {
    (
        hyper::header::SET_COOKIE,
        HeaderValue::from_str(&format!("{COOKIE}=; Path=/account; HttpOnly; SameSite=Strict; Max-Age=0")).expect("a cookie"),
    )
}

/// Back to the account page, after a form.
fn redirect(cookie: Option<String>) -> Reply {
    let mut reply =
        Reply { status: StatusCode::SEE_OTHER, content_type: "text/plain; charset=utf-8", headers: Vec::new(), body: Vec::new() };
    reply.headers.push((hyper::header::LOCATION, HeaderValue::from_static("/account")));
    if let Some(cookie) = cookie.and_then(|cookie| HeaderValue::from_str(&cookie).ok()) {
        reply.headers.push((hyper::header::SET_COOKIE, cookie));
    }
    reply
}

const STYLE: &str = "body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;font-size:15px;line-height:1.5;\
color:#1d1d1f;background:#f5f5f7;margin:0}main{max-width:560px;margin:48px auto;padding:0 20px}h1{font-size:28px;margin:0 0 16px}\
h2{font-size:18px;margin:0 0 4px}section{background:#fff;border-radius:12px;padding:16px 20px;margin:16px 0}\
.muted{color:#6e6e73;margin:4px 0}.notice{background:#fff;border-radius:8px;padding:8px 12px}ul{margin:4px 0 12px;padding-left:20px}\
pre{font-size:13px;overflow-x:auto;white-space:pre}input{font:inherit;padding:8px 10px;border:1px solid #d2d2d7;border-radius:8px;margin-right:8px}\
button{font:inherit;padding:8px 14px;border:0;border-radius:8px;background:#0071e3;color:#fff;cursor:pointer}\
button.plain{background:#e8e8ed;color:#1d1d1f}button.danger{background:#d70015}form{margin:8px 0}\
@media (prefers-color-scheme:dark){body{background:#000;color:#f5f5f7}section,.notice{background:#1c1c1e}.muted{color:#98989d}\
input{background:#1c1c1e;color:#f5f5f7;border-color:#3a3a3c}button.plain{background:#3a3a3c;color:#f5f5f7}a{color:#2997ff}}";

fn html(status: StatusCode, title: &str, body: &str) -> Reply {
    let page = format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1\">\
         <title>{title}</title><style>{STYLE}</style></head><body><main><h1>{title}</h1>{body}</main></body></html>",
        title = escape(title)
    );
    let mut reply = Reply { status, content_type: "text/html; charset=utf-8", headers: Vec::new(), body: page.into_bytes() };
    reply.headers.push((
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
    ));
    reply.headers.push((HeaderName::from_static("referrer-policy"), HeaderValue::from_static("no-referrer")));
    reply
}

#[cfg(test)]
mod tests {
    #[test]
    fn token_counts_are_rounded_as_the_app_writes_them() {
        for (count, text) in [
            (950, "950"),
            (1_000, "1k"),
            (840_123, "840k"),
            (999_499, "999k"),
            (999_500, "1M"),
            (1_234_567, "1.2M"),
            (12_345_678, "12M"),
            (999_950_000, "1B"),
            (3_400_000_000, "3.4B"),
            (1_000_000_000_000, "1000B"),
        ] {
            assert_eq!(super::rounded(count), text, "{count}");
        }
    }
}
