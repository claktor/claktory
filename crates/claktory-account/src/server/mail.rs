//! Email: the fixed messages the service sends, and the ways it sends them.
//! Nothing in a message comes from a stranger but a Claktory's or a
//! device's cleaned name, so the service can't be made to send anyone's
//! words.

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use serde_json::json;

/// One email, in plain text and HTML.
#[derive(Clone, Debug, PartialEq)]
pub struct Mail {
    pub from: String,
    pub to: String,
    pub subject: String,
    pub text: String,
    pub html: String,
}

pub type Sending<'a> = Pin<Box<dyn Future<Output = Result<(), MailError>> + Send + 'a>>;

/// Why an email wasn't sent.
#[derive(Clone, Debug, PartialEq)]
pub enum MailError {
    /// The provider won't send for the service at all until whoever runs it
    /// acts: its plan, its domain, its token, its rate. `kind` names it
    /// (`not_entitled`, `sending_disabled`, `throttled`…), for the alert;
    /// `text` says what to do.
    Refused { kind: &'static str, text: String },
    /// This one email wasn't sent: its address is suppressed or bounced, or
    /// the provider failed.
    NotSent(String),
}

impl std::fmt::Display for MailError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MailError::Refused { text, .. } | MailError::NotSent(text) => f.write_str(text),
        }
    }
}

/// What sends email: Cloudflare, Resend, a folder, or memory in tests.
pub trait Mailer: Send + Sync {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a>;
}

/// The address codes come from: `code@<domain>`.
pub fn code_address(domain: &str) -> String {
    format!("code@{domain}")
}

/// The code that signs in, from Claktory, from `code@<domain>`.
pub fn code_mail(domain: &str, to: &str, code: &str) -> Mail {
    let text = format!(
        "Your code to sign in to Claktory:\n\n{code}\n\nIt works for 15 minutes.\n\nIf you didn't ask for it, ignore this email. No one can sign in without the code.\n"
    );
    let html = page(&format!(
        "<p>Your code to sign in to Claktory:</p>\
         <p style=\"font-size:32px;font-weight:600;letter-spacing:6px;margin:24px 0\">{code}</p>\
         <p>It works for 15 minutes.</p>\
         <p>If you didn't ask for it, ignore this email. No one can sign in without the code.</p>"
    ));
    let from = format!("Claktory <{}>", code_address(domain));
    Mail { from, to: to.to_owned(), subject: format!("Your Claktory code: {code}"), text, html }
}

/// The code that signs a device in, such as the app on a phone, naming it
/// so the person knows which one asked.
pub fn device_code_mail(domain: &str, to: &str, code: &str, device: &str) -> Mail {
    let text = format!(
        "Your code to sign in to Claktory on {device}:\n\n{code}\n\nIt works for 15 minutes.\n\nIf you didn't ask for it, ignore this email. No one can sign in without the code.\n"
    );
    let html = page(&format!(
        "<p>Your code to sign in to Claktory on {device}:</p>\
         <p style=\"font-size:32px;font-weight:600;letter-spacing:6px;margin:24px 0\">{code}</p>\
         <p>It works for 15 minutes.</p>\
         <p>If you didn't ask for it, ignore this email. No one can sign in without the code.</p>",
        device = escape(device)
    ));
    let from = format!("Claktory <{}>", code_address(domain));
    Mail { from, to: to.to_owned(), subject: format!("Your Claktory code: {code}"), text, html }
}

/// A Claktory got its share after a restart: so its owner learns if anyone
/// else did, with a copy of its disk.
pub fn unlock_mail(domain: &str, to: &str, name: &str, when: i64, ip: &str, account_page: &str) -> Mail {
    let when = format_time(when);
    let text = format!(
        "{name} unlocked its secrets after starting up.\n\n\
         When: {when}\nFrom: {ip}\n\n\
         This happens each time {name} restarts, such as after an update. If you restarted it, there's nothing to do.\n\n\
         If you didn't, someone may have a copy of its disk. Remove {name} from your account at {account_page} so it can't unlock again. \
         Then change the keys kept in its secrets at each service they're for.\n"
    );
    let subject = format!("{name} unlocked its secrets");
    let (name, ip, account_page) = (escape(name), escape(ip), escape(account_page));
    let html = page(&format!(
        "<p>{name} unlocked its secrets after starting up.</p>\
         <p>When: {when}<br>From: {ip}</p>\
         <p>This happens each time {name} restarts, such as after an update. If you restarted it, there's nothing to do.</p>\
         <p>If you didn't, someone may have a copy of its disk. Remove {name} from your account at {account_page} so it can't unlock again. \
         Then change the keys kept in its secrets at each service they're for.</p>"
    ));
    Mail { from: format!("Claktory <notices@{domain}>"), to: to.to_owned(), subject, text, html }
}

/// To whoever runs the service: the mail provider refused to send. It may
/// not arrive (the provider refuses this one too), so the log says it as
/// well.
pub fn refused_alert_mail(domain: &str, to: &str, why: &str) -> Mail {
    let text = format!("The account service's mail provider refused to send email, so sign-in codes aren't going out.\n\n{why}\n");
    let html = page(&format!("<p>{}</p>", escape(&text).replace("\n\n", "</p><p>")));
    Mail {
        from: format!("Claktory <notices@{domain}>"),
        to: to.to_owned(),
        subject: "Claktory account service: email refused".to_owned(),
        text,
        html,
    }
}

/// To whoever runs the service: today's email is nearly used up.
pub fn cap_alert_mail(domain: &str, to: &str, sent: u32, cap: u32) -> Mail {
    let text = format!(
        "The account service has sent {sent} of its {cap} emails for today. At {cap} it stops sending sign-in codes until tomorrow (UTC).\n\n\
         Raise --email-daily-cap if your mail plan allows more.\n"
    );
    let html = page(&format!("<p>{}</p>", escape(&text).replace("\n\n", "</p><p>")));
    Mail {
        from: format!("Claktory <notices@{domain}>"),
        to: to.to_owned(),
        subject: format!("Claktory account service: {sent} of {cap} emails sent today"),
        text,
        html,
    }
}

fn page(body: &str) -> String {
    format!(
        "<!doctype html><html><body style=\"font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif;font-size:15px;line-height:1.5;color:#1d1d1f\">{body}</body></html>"
    )
}

/// "20 Nov 2026, 14:02 UTC".
pub fn format_time(unix: i64) -> String {
    match jiff::Timestamp::from_second(unix) {
        Ok(time) => time.strftime("%-d %b %Y, %H:%M UTC").to_string(),
        Err(_) => unix.to_string(),
    }
}

/// HTML-escapes text.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(character),
        }
    }
    out
}

/// Sends through Resend's API.
pub struct Resend {
    key: String,
    url: String,
    client: reqwest::Client,
}

impl Resend {
    pub fn new(key: String) -> Resend {
        Resend::with_url(key, "https://api.resend.com/emails".to_owned())
    }

    /// For a stand-in of Resend's API, in tests.
    pub fn with_url(key: String, url: String) -> Resend {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("an HTTP client");
        Resend { key, url, client }
    }
}

impl Mailer for Resend {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            let body = json!({"from": mail.from, "to": [mail.to], "subject": mail.subject, "text": mail.text, "html": mail.html});
            let response = self
                .client
                .post(&self.url)
                .bearer_auth(&self.key)
                .json(&body)
                .send()
                .await
                .map_err(|error| MailError::NotSent(format!("Resend: {error}")))?;
            let status = response.status();
            if status.is_success() {
                return Ok(());
            }
            let answer: String = response.text().await.unwrap_or_default().chars().take(300).collect();
            let text = format!("Resend: {status}: {answer}");
            Err(match status.as_u16() {
                401 | 403 => MailError::Refused { kind: "unauthorized", text: format!("{text}. Check the Resend API key.") },
                429 => MailError::Refused { kind: "throttled", text },
                _ => MailError::NotSent(text),
            })
        })
    }
}

/// Sends through Cloudflare Email Service's REST API, from a domain
/// onboarded for Email Sending in the account, with an API token that has
/// Email Sending: Edit.
pub struct Cloudflare {
    token: String,
    url: String,
    client: reqwest::Client,
}

impl Cloudflare {
    pub fn new(token: String, account: &str) -> Cloudflare {
        Cloudflare::with_base(token, account, "https://api.cloudflare.com/client/v4")
    }

    /// For a stand-in of Cloudflare's API, in tests.
    pub fn with_base(token: String, account: &str, base: &str) -> Cloudflare {
        let client = reqwest::Client::builder().timeout(Duration::from_secs(20)).build().expect("an HTTP client");
        let url = format!("{}/accounts/{account}/email/sending/send", base.trim_end_matches('/'));
        Cloudflare { token, url, client }
    }
}

/// "Claktory <code@…>" as Cloudflare takes it: `{"address", "name"}`.
fn address(text: &str) -> serde_json::Value {
    match text.rsplit_once('<') {
        Some((name, rest)) if rest.ends_with('>') => {
            let address = &rest[..rest.len() - 1];
            match name.trim() {
                "" => json!(address),
                name => json!({"address": address, "name": name}),
            }
        }
        _ => json!(text),
    }
}

/// What Cloudflare's refusal means, by its error code: the problems whoever
/// runs the service must fix, and what to do.
fn cloudflare_refusal(code: i64, message: &str) -> Option<(&'static str, &'static str)> {
    Some(match code {
        10105 => {
            ("not_entitled", "The Cloudflare account isn't entitled to Email Sending: it needs the Workers Paid plan to email anyone.")
        }
        10203 => (
            "sending_disabled",
            "Sending is turned off for the domain or the account: turn it on in Cloudflare under Email Service → Email Sending.",
        ),
        10207 => ("account_suspended", "Cloudflare paused Email Sending for the account: its Email Sending page says why."),
        10101 | 10103 => (
            "unauthorized",
            "Cloudflare refused the API token: make a new one with Email Sending: Edit and set it with the deploy kit's cloudflare-token command.",
        ),
        10102 => ("forbidden", "The API token can't send email: give it Email Sending: Edit on the account."),
        10000 => (
            "not_found",
            "Cloudflare doesn't know that account, or its domain isn't onboarded for Email Sending: check --cloudflare-account.",
        ),
        10004 => ("throttled", "Cloudflare is limiting how fast the service sends: codes go again once it lets them."),
        _ if message.ends_with("not_entitled") => ("not_entitled", "The Cloudflare account isn't entitled to Email Sending."),
        _ if message.ends_with("sending_disabled") => ("sending_disabled", "Sending is turned off for the domain or the account."),
        _ if message.ends_with("throttled") => ("throttled", "Cloudflare is limiting how fast the service sends."),
        _ => return None,
    })
}

impl Mailer for Cloudflare {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            let body = json!({
                "from": address(&mail.from),
                "to": mail.to,
                "subject": mail.subject,
                "text": mail.text,
                "html": mail.html,
            });
            let response = self
                .client
                .post(&self.url)
                .bearer_auth(&self.token)
                .json(&body)
                .send()
                .await
                .map_err(|error| MailError::NotSent(format!("Cloudflare: {error}")))?;
            let status = response.status();
            let answer = response.text().await.unwrap_or_default();
            let value: serde_json::Value = serde_json::from_str(&answer).unwrap_or_default();
            let short: String = answer.chars().take(300).collect();
            if status.is_success() && value["success"] != json!(false) {
                // Dropped (on the suppression list) or bounced at once: not sent.
                let result = &value["result"];
                for (list, why) in [("suppressed_recipients", "is on Cloudflare's suppression list"), ("permanent_bounces", "bounced")] {
                    let listed = result[list].as_array().is_some_and(|addresses| {
                        addresses.iter().any(|address| address.as_str().is_some_and(|address| address.eq_ignore_ascii_case(&mail.to)))
                    });
                    if listed {
                        return Err(MailError::NotSent(format!("Cloudflare: {} {why}", mail.to)));
                    }
                }
                return Ok(());
            }
            let error = &value["errors"][0];
            let code = error["code"].as_i64().unwrap_or_default();
            let message = error["message"].as_str().unwrap_or_default();
            let text = format!("Cloudflare: {status}: {}", if message.is_empty() { short.as_str() } else { message });
            if let Some((kind, what)) = cloudflare_refusal(code, message) {
                return Err(MailError::Refused { kind, text: format!("{what} ({text})") });
            }
            if status.as_u16() == 429 {
                return Err(MailError::Refused { kind: "throttled", text });
            }
            Err(MailError::NotSent(text))
        })
    }
}

/// Writes each email to a file in a folder instead of sending it: for a
/// local instance, and for self-hosting without a mail provider.
pub struct Folder {
    dir: PathBuf,
    count: AtomicU64,
}

impl Folder {
    pub fn new(dir: PathBuf) -> std::io::Result<Folder> {
        std::fs::create_dir_all(&dir)?;
        Ok(Folder { dir, count: AtomicU64::new(0) })
    }
}

impl Mailer for Folder {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            let number = self.count.fetch_add(1, Ordering::Relaxed);
            let stamp = jiff::Timestamp::now().strftime("%Y%m%d-%H%M%S");
            let path = self.dir.join(format!("{stamp}-{}-{number}.eml", std::process::id()));
            let text = format!("From: {}\nTo: {}\nSubject: {}\n\n{}", mail.from, mail.to, mail.subject, mail.text);
            tokio::fs::write(&path, text).await.map_err(|error| MailError::NotSent(format!("can't write {}: {error}", path.display())))?;
            tracing::info!("mail written to {}", path.display());
            Ok(())
        })
    }
}

/// Keeps what it sends, for tests; `fail` makes it fail as a provider can,
/// and `refuse_next` refuses the next email only.
#[derive(Default)]
pub struct Memory {
    pub sent: Mutex<Vec<Mail>>,
    pub fail: AtomicBool,
    pub refuse_next: Mutex<Option<MailError>>,
}

impl Memory {
    pub fn sent(&self) -> Vec<Mail> {
        self.sent.lock().expect("mail").clone()
    }
}

impl Mailer for Memory {
    fn send<'a>(&'a self, mail: &'a Mail) -> Sending<'a> {
        Box::pin(async move {
            if let Some(error) = self.refuse_next.lock().expect("mail").take() {
                return Err(error);
            }
            if self.fail.load(Ordering::Relaxed) {
                return Err(MailError::NotSent("the provider failed".to_owned()));
            }
            self.sent.lock().expect("mail").push(mail.clone());
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_emails_read_as_written() {
        let mail = code_mail("auth.claktory.com", "matt@example.com", "482913");
        assert_eq!(mail.from, "Claktory <code@auth.claktory.com>");
        assert_eq!(mail.subject, "Your Claktory code: 482913");
        assert!(mail.text.starts_with("Your code to sign in to Claktory:\n\n482913\n\nIt works for 15 minutes.\n\n"));

        let mail = unlock_mail("auth.claktory.com", "matt@example.com", "Mac <mini>", 1_795_183_320, "203.0.113.7", "claktory.com/account");
        assert_eq!(mail.from, "Claktory <notices@auth.claktory.com>");
        assert_eq!(mail.subject, "Mac <mini> unlocked its secrets");
        assert!(mail.text.contains("When: 20 Nov 2026, 14:02 UTC\nFrom: 203.0.113.7\n"), "{}", mail.text);
        assert!(mail.html.contains("Mac &lt;mini&gt;") && !mail.html.contains("<mini>"));
        for word in ["vault", "share", "token", "epoch"] {
            assert!(!mail.text.contains(word), "{word}");
        }
    }
}
