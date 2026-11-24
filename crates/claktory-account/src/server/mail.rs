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

use super::db::Unusual;

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
    sign_in_mail(domain, to, code, "Your code to sign in to Claktory:")
}

/// The code that signs a device in, such as the app on a phone, naming it
/// so the person knows which one asked.
pub fn device_code_mail(domain: &str, to: &str, code: &str, device: &str) -> Mail {
    sign_in_mail(domain, to, code, &format!("Your code to sign in to Claktory on {device}:"))
}

const FONT: &str = "-apple-system,BlinkMacSystemFont,'Segoe UI',Helvetica,Arial,sans-serif";

/// A sign-in code: in plain text, and in HTML as the app looks (its mark,
/// the code large in a well, the rest quieter), laid out in tables with
/// inline styles, which Gmail and Apple Mail both keep. It loads nothing:
/// the mark is drawn in HTML (the icon's graphite tile, a white C with the
/// blue shadow where the client draws one), since Gmail hides a remote
/// image behind "Images in this message are hidden". `intro` is plain
/// text, escaped here.
fn sign_in_mail(domain: &str, to: &str, code: &str, intro: &str) -> Mail {
    const WORKS: &str = "It works for 15 minutes.";
    const IGNORE: &str = "If you didn't ask for it, ignore this email. No one can sign in without the code.";
    let text = format!("{intro}\n\n{code}\n\n{WORKS}\n\n{IGNORE}\n");
    let (intro, code) = (escape(intro), escape(code));
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\">\
         <meta name=\"color-scheme\" content=\"light\"><meta name=\"supported-color-schemes\" content=\"light\"><title>Your Claktory code</title></head>\
         <body style=\"margin:0;padding:0;background:#F5F5F6\">\
         <div style=\"display:none;max-height:0;overflow:hidden;opacity:0\">{code} is your code to sign in to Claktory.</div>\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"background:#F5F5F6\"><tr><td align=\"center\" style=\"padding:40px 16px\">\
         <table role=\"presentation\" width=\"100%\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\" style=\"max-width:440px;background:#FFFFFF;border:1px solid #E5E5E7;border-radius:14px\">\
         <tr><td style=\"padding:28px 32px 0\">\
         <table role=\"presentation\" cellpadding=\"0\" cellspacing=\"0\" border=\"0\"><tr>\
         <td width=\"36\" height=\"36\" align=\"center\" style=\"width:36px;height:36px;vertical-align:middle;background:#151517;border-radius:9px;\
         font-family:{FONT};font-size:21px;font-weight:700;line-height:36px;color:#FFFFFF;text-align:center;text-shadow:2px 2px 0 #2F6FEB\">C</td>\
         <td style=\"vertical-align:middle;padding-left:10px;font-family:{FONT};font-size:17px;font-weight:600;color:#1D1D1F\">Claktory</td>\
         </tr></table></td></tr>\
         <tr><td style=\"padding:24px 32px 0;font-family:{FONT};font-size:15px;line-height:1.5;color:#1D1D1F\">{intro}</td></tr>\
         <tr><td style=\"padding:16px 32px\"><div style=\"background:#F5F5F6;border-radius:10px;padding:18px 0 18px 8px;text-align:center;\
         font-family:'SF Mono',Menlo,Consolas,monospace;font-size:34px;font-weight:600;letter-spacing:8px;color:#1D1D1F\">{code}</div></td></tr>\
         <tr><td style=\"padding:0 32px 32px;font-family:{FONT};font-size:14px;line-height:1.5;color:#6E6E73\">{WORKS}<br><br>{IGNORE}</td></tr>\
         </table></td></tr></table></body></html>"
    );
    let from = format!("Claktory <{}>", code_address(domain));
    Mail { from, to: to.to_owned(), subject: format!("Your Claktory code: {code}"), text, html }
}

/// A Claktory got its share after a restart in a way that doesn't look
/// like it restarting where it runs (#412): so its owner learns if anyone
/// else did, with a copy of its disk.
pub fn unlock_mail(domain: &str, to: &str, name: &str, unusual: Unusual, when: i64, ip: &str, account_page: &str) -> Mail {
    let when = format_time(when);
    let what = |name: &str| match unusual {
        Unusual::NewNetwork => format!("{name} unlocked its secrets from a network it hasn't used in the last 30 days."),
        Unusual::OlderCopy => format!("An older copy of {name}, from before it last signed in to your account, unlocked its secrets."),
    };
    let why = |name: &str| {
        format!(
            "Claktory emails you only when this looks unusual, not each time {name} restarts. \
             If you took {name} somewhere new or restored it from a backup, there's nothing to do."
        )
    };
    let text = format!(
        "{what}\n\n\
         When: {when}\nFrom: {ip}\n\n\
         {why}\n\n\
         If you didn't, someone may have a copy of its disk. Remove {name} from your account at {account_page} so it can't unlock again. \
         Then change the keys kept in its secrets at each service they're for.\n",
        what = what(name),
        why = why(name),
    );
    let subject = format!("{name} unlocked its secrets");
    let (name, ip, account_page) = (escape(name), escape(ip), escape(account_page));
    let html = page(&format!(
        "<p>{what}</p>\
         <p>When: {when}<br>From: {ip}</p>\
         <p>{why}</p>\
         <p>If you didn't, someone may have a copy of its disk. Remove {name} from your account at {account_page} so it can't unlock again. \
         Then change the keys kept in its secrets at each service they're for.</p>",
        what = what(&name),
        why = why(&name),
    ));
    Mail { from: format!("Claktory <notices@{domain}>"), to: to.to_owned(), subject, text, html }
}

/// To the account: one of its Claktories moved to another computer.
pub fn moved_mail(domain: &str, to: &str, from: &str, name: &str, when: i64, ip: &str, account_page: &str) -> Mail {
    let when = format_time(when);
    let from = if from.is_empty() { "Your Claktory".to_owned() } else { from.to_owned() };
    let text = format!(
        "{from} moved to {name}.\n\n\
         When: {when}\nFrom: {ip}\n\n\
         The copy on the old computer can no longer sign in to your account or unlock its secrets. If you moved it, there's nothing to do.\n\n\
         If you didn't, remove it from your account at {account_page}, then change the keys kept in its secrets at each service they're for.\n"
    );
    let subject = format!("{from} moved to {name}");
    let (from, name, ip, account_page) = (escape(&from), escape(name), escape(ip), escape(account_page));
    let html = page(&format!(
        "<p>{from} moved to {name}.</p>\
         <p>When: {when}<br>From: {ip}</p>\
         <p>The copy on the old computer can no longer sign in to your account or unlock its secrets. If you moved it, there's nothing to do.</p>\
         <p>If you didn't, remove it from your account at {account_page}, then change the keys kept in its secrets at each service they're for.</p>"
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
        // The HTML part: the mark drawn in it, the code large, the same words.
        assert!(mail.html.contains("background:#151517") && mail.html.contains(">C</td>"), "{}", mail.html);
        assert!(mail.html.contains("letter-spacing:8px;color:#1D1D1F\">482913</div>"), "{}", mail.html);
        assert!(mail.html.contains("482913 is your code to sign in to Claktory."), "the inbox preview");
        assert!(mail.html.contains("It works for 15 minutes.<br><br>If you didn't ask for it"), "{}", mail.html);
        assert!(!mail.html.contains("<script") && !mail.html.contains("<style"), "inline styles only");

        // A device's name is the one thing in it from outside: escaped in HTML.
        let mail = device_code_mail("auth.claktory.com", "matt@example.com", "482913", "Matt's <iPhone>");
        assert!(mail.text.starts_with("Your code to sign in to Claktory on Matt's <iPhone>:\n\n482913\n"));
        assert!(mail.html.contains("on Matt&#39;s &lt;iPhone&gt;:") && !mail.html.contains("<iPhone>"));

        let mail = unlock_mail(
            "auth.claktory.com",
            "matt@example.com",
            "Mac <mini>",
            Unusual::NewNetwork,
            1_795_183_320,
            "203.0.113.7",
            "claktory.com/account",
        );
        assert_eq!(mail.from, "Claktory <notices@auth.claktory.com>");
        assert_eq!(mail.subject, "Mac <mini> unlocked its secrets");
        assert!(mail.text.contains("When: 20 Nov 2026, 14:02 UTC\nFrom: 203.0.113.7\n"), "{}", mail.text);
        assert!(mail.html.contains("Mac &lt;mini&gt;") && !mail.html.contains("<mini>"));
        for word in ["vault", "share", "token", "epoch"] {
            assert!(!mail.text.contains(word), "{word}");
        }
        let moved = moved_mail("auth.claktory.com", "matt@example.com", "", "Mac", 1_795_183_320, "203.0.113.7", "claktory.com/account");
        // Nothing loads from elsewhere, which Gmail would hide.
        let refused = refused_alert_mail("d", "m", "Not entitled.");
        let cap = cap_alert_mail("d", "m", 90, 100);
        for mail in [code_mail("d", "m", "1"), device_code_mail("d", "m", "1", "iPhone"), mail, moved, refused, cap] {
            assert!(!mail.html.contains("<img") && !mail.html.contains("src=") && !mail.html.contains("url("), "{}", mail.html);
        }
    }
}
