use claktory_account::server::{Rotate, Secrets, db, stats};

use crate::support::*;

#[test]
fn secrets_are_made_once_readable_only_by_their_owner() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let made = Secrets::create(&path).expect("made");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let loaded = Secrets::load(&path).expect("loaded");
    assert_eq!(*loaded.k_share, *made.k_share);
    assert_eq!(loaded.signing_key.to_bytes(), made.signing_key.to_bytes());
    assert_eq!(loaded.relay_bearer, made.relay_bearer);
    assert_eq!(loaded.resend_api_key, None);
    assert!(Secrets::create(&path).is_err(), "never over a file");
    assert_eq!(*Secrets::load(&path).unwrap().k_share, *made.k_share);

    let mut text: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    text["resend_api_key"] = "re_123".into();
    std::fs::write(&path, text.to_string()).unwrap();
    assert_eq!(Secrets::load(&path).unwrap().resend_api_key.as_deref(), Some("re_123"));
    std::fs::write(&path, "{\"k_share\": \"short\", \"signing_key\": \"x\"}").unwrap();
    assert!(Secrets::load(&path).is_err());
}

#[tokio::test]
async fn stats_count_accounts_claktories_and_sign_ins() {
    let instance = start().await;
    let mut first = Claktory::new("198.51.107.1");
    instance.sign_in(&mut first, "matt@example.com", "Mac mini").await;
    let mut second = Claktory::new("198.51.107.2");
    instance.sign_in(&mut second, "matt@example.com", "Studio").await;
    let mut third = Claktory::new("198.51.107.3");
    instance.sign_in(&mut third, "ana@example.com", "Laptop").await;

    let conn = db::open(&instance.db).unwrap();
    let report = stats::report(&conn, instance.now() + 8 * 86_400).unwrap();
    assert!(report.contains("Accounts: 2\n"), "{report}");
    assert!(report.contains("Claktories: 3 (0 removed)\n"));
    assert!(report.contains("in the last 30 days: Claktories 0 / 0 / 3; accounts 0 / 0 / 2\n"), "{report}");
    assert!(report.contains("  1: 1\n  2: 1\n"), "per account");
    assert!(report.contains("  v0.1.9: 3\n") && report.contains("  macos-arm64: 3\n"));
    assert!(report.contains("  2026-11-20: 3\n"));
}

#[test]
fn old_share_releases_and_their_ips_are_forgotten() {
    let dir = tempfile::tempdir().unwrap();
    let conn = db::open(&dir.path().join("account.db")).unwrap();
    db::record_release(&conn, "claktory-0123456789", START, "203.0.113.7", 1).unwrap();
    db::purge(&conn, START + db::RELEASES_KEPT - 1).unwrap();
    assert_eq!(db::releases(&conn, "claktory-0123456789", 10).unwrap().len(), 1);
    db::purge(&conn, START + db::RELEASES_KEPT + 1).unwrap();
    assert!(db::releases(&conn, "claktory-0123456789", 10).unwrap().is_empty());
}

/// A stand-in for Resend's API: answers one request with `status`, and
/// hands back what it was sent.
async fn fake_resend(status: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let (base, handle) = fake(status, "{}").await;
    (format!("{base}/emails"), handle)
}

/// A stand-in for a mail provider's API, at the URL it gives: answers one
/// request with `status` and `body`, and hands back what it was sent.
async fn fake(status: &'static str, body: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = Vec::new();
        let mut buffer = [0u8; 4096];
        loop {
            let read = stream.read(&mut buffer).await.unwrap();
            request.extend_from_slice(&buffer[..read]);
            let text = String::from_utf8_lossy(&request).to_string();
            if let Some((head, body)) = text.split_once("\r\n\r\n") {
                let length: usize = head
                    .lines()
                    .find_map(|line| line.to_ascii_lowercase().strip_prefix("content-length:").map(|value| value.trim().parse().unwrap()))
                    .unwrap_or(0);
                if body.len() >= length {
                    break;
                }
            }
        }
        let answer = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}", body.len());
        stream.write_all(answer.as_bytes()).await.unwrap();
        String::from_utf8_lossy(&request).to_string()
    });
    (url, handle)
}

#[tokio::test]
async fn resend_gets_the_whole_email_with_the_key() {
    use claktory_account::server::{Mailer, mail};
    let (url, request) = fake_resend("200 OK").await;
    let resend = mail::Resend::with_url("re_key".into(), url);
    let email = mail::code_mail("auth.claktory.com", "https://account.claktory.com", "matt@example.com", "482913");
    resend.send(&email).await.expect("sent");
    let request = request.await.unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /emails ") && head.to_ascii_lowercase().contains("authorization: bearer re_key"), "{head}");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["from"], "Claktory <code@auth.claktory.com>");
    assert_eq!(body["to"], serde_json::json!(["matt@example.com"]));
    assert_eq!(body["subject"], "Your Claktory code: 482913");
    assert_eq!(body["text"], email.text.as_str());
    assert_eq!(body["html"], email.html.as_str());

    let (url, _) = fake_resend("422 Unprocessable Entity").await;
    let error = mail::Resend::with_url("re_key".into(), url).send(&email).await.expect_err("refused");
    assert!(matches!(&error, mail::MailError::NotSent(text) if text.contains("422")), "{error}");
    let (url, _) = fake_resend("429 Too Many Requests").await;
    let error = mail::Resend::with_url("re_key".into(), url).send(&email).await.expect_err("refused");
    assert!(matches!(error, mail::MailError::Refused { kind: "throttled", .. }), "{error}");
}

#[tokio::test]
async fn cloudflare_gets_the_whole_email_with_the_token_and_its_refusals_are_told_apart() {
    use claktory_account::server::{Mailer, mail};
    let email = mail::code_mail("auth.claktory.com", "https://account.claktory.com", "matt@example.com", "482913");
    let sent = r#"{"success": true, "errors": [], "messages": [], "result": {"message_id": "<m@auth.claktory.com>", "delivered": ["matt@example.com"], "permanent_bounces": [], "queued": [], "suppressed_recipients": []}}"#;
    let (base, request) = fake("200 OK", sent).await;
    mail::Cloudflare::with_base("cf-token".into(), "acc", &format!("{base}/client/v4")).send(&email).await.expect("sent");
    let request = request.await.unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /client/v4/accounts/acc/email/sending/send "), "{head}");
    assert!(head.to_ascii_lowercase().contains("authorization: bearer cf-token"), "{head}");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["from"], serde_json::json!({"address": "code@auth.claktory.com", "name": "Claktory"}));
    assert_eq!(body["to"], "matt@example.com");
    assert_eq!(body["subject"], "Your Claktory code: 482913");
    assert_eq!(body["text"], email.text.as_str());
    assert_eq!(body["html"], email.html.as_str());

    let email = &email;
    let send = |status: &'static str, answer: &'static str| async move {
        let (base, _) = fake(status, answer).await;
        mail::Cloudflare::with_base("cf-token".into(), "acc", &format!("{base}/client/v4")).send(email).await
    };
    // Dropped as suppressed, or bounced: not sent.
    let dropped = r#"{"success": true, "errors": [], "result": {"delivered": [], "permanent_bounces": [], "queued": [], "suppressed_recipients": ["Matt@Example.com"]}}"#;
    let error = send("200 OK", dropped).await.expect_err("suppressed");
    assert!(matches!(&error, mail::MailError::NotSent(text) if text.contains("suppression list")), "{error}");
    let refused =
        r#"{"success": false, "errors": [{"code": 10001, "message": "email.sending.error.recipient_suppressed"}], "result": null}"#;
    assert!(matches!(send("400 Bad Request", refused).await, Err(mail::MailError::NotSent(_))));
    // The account's problems are told apart, for the log and the alert.
    for (status, code, message, kind) in [
        ("403 Forbidden", 10105, "email.sending.error.authentication.not_entitled", "not_entitled"),
        ("403 Forbidden", 10203, "email.sending.error.email.sending_disabled", "sending_disabled"),
        ("403 Forbidden", 10207, "email.sending.error.email.account_suspended", "account_suspended"),
        ("401 Unauthorized", 10101, "email.sending.error.authentication.unauthorized", "unauthorized"),
        ("403 Forbidden", 10102, "email.sending.error.authentication.forbidden", "forbidden"),
        ("429 Too Many Requests", 10004, "email.sending.error.throttled", "throttled"),
    ] {
        let answer: &'static str = Box::leak(
            serde_json::json!({"success": false, "errors": [{"code": code, "message": message}], "result": null})
                .to_string()
                .into_boxed_str(),
        );
        match send(status, answer).await {
            Err(mail::MailError::Refused { kind: said, text }) => {
                assert_eq!(said, kind);
                assert!(text.contains(message), "{text}");
            }
            other => panic!("{kind}: {other:?}"),
        }
    }
    let broken = r#"{"success": false, "errors": [{"code": 10002, "message": "email.sending.error.internal_server"}], "result": null}"#;
    assert!(matches!(send("500 Internal Server Error", broken).await, Err(mail::MailError::NotSent(_))));
}

#[tokio::test]
async fn a_backup_is_a_whole_copy_and_only_the_newest_are_kept() {
    let instance = start().await;
    let mut claktory = Claktory::new("198.51.108.1");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let dir = tempfile::tempdir().unwrap();
    let mut written = Vec::new();
    for day in 0..4 {
        written.push(db::backup(&instance.db, dir.path(), 3, START + day * 86_400).expect("a backup"));
    }
    assert_eq!(written[0].file_name().unwrap(), "account-20261120-140200.db");
    let mut left: Vec<_> = std::fs::read_dir(dir.path()).unwrap().map(|entry| entry.unwrap().path()).collect();
    left.sort();
    assert_eq!(left, written[1..].to_vec(), "the oldest went");
    let copy = db::open(&written[3]).unwrap();
    let email: String = copy.query_row("SELECT email FROM accounts", [], |row| row.get(0)).unwrap();
    assert_eq!(email, "matt@example.com");
}

#[test]
fn the_resend_key_goes_in_and_the_keys_stay() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let made = Secrets::create(&path).unwrap();
    Secrets::set_resend_key(&path, "re_abc123").expect("set");
    let loaded = Secrets::load(&path).unwrap();
    assert_eq!(loaded.resend_api_key.as_deref(), Some("re_abc123"));
    assert_eq!(*loaded.k_share, *made.k_share);
    assert_eq!(loaded.signing_key.to_bytes(), made.signing_key.to_bytes());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }
    assert!(Secrets::set_resend_key(&path, "").is_err());
    assert!(Secrets::set_resend_key(&path, "re_a b").is_err());
    assert!(Secrets::set_resend_key(&dir.path().join("missing.json"), "re_x").is_err());
}

#[test]
fn each_key_rotates_on_its_own_and_the_file_before_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let made = Secrets::create(&path).unwrap();
    Secrets::set_cloudflare_token(&path, "cf_token").unwrap();
    assert!(Secrets::rotate(&path, Rotate::default(), 1).is_err(), "a key has to be named");

    // The signing key alone: k_share, the token and the bearer stay.
    let (signing, before) = Secrets::rotate(&path, Rotate { signing_key: true, k_share: false }, 100).unwrap();
    assert_ne!(signing.signing_key.to_bytes(), made.signing_key.to_bytes());
    assert_eq!(*signing.k_share, *made.k_share);
    assert_eq!(signing.cloudflare_api_token.as_deref(), Some("cf_token"));
    assert_eq!(signing.relay_bearer, made.relay_bearer);
    assert_eq!(before, dir.path().join("secrets.json.before-100"));
    let kept = Secrets::load(&before).unwrap();
    assert_eq!(kept.signing_key.to_bytes(), made.signing_key.to_bytes(), "the file as it was");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        for file in [&path, &before] {
            assert_eq!(std::fs::metadata(file).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }

    // k_share alone: the signing key stays.
    let (share, _) = Secrets::rotate(&path, Rotate { signing_key: false, k_share: true }, 200).unwrap();
    assert_ne!(*share.k_share, *made.k_share);
    assert_eq!(share.signing_key.to_bytes(), signing.signing_key.to_bytes());

    // Both, and never over an earlier copy.
    let (both, _) = Secrets::rotate(&path, Rotate { signing_key: true, k_share: true }, 300).unwrap();
    assert_ne!(*both.k_share, *share.k_share);
    assert_ne!(both.signing_key.to_bytes(), share.signing_key.to_bytes());
    assert!(Secrets::rotate(&path, Rotate { signing_key: true, k_share: false }, 300).is_err(), "the copy at 300 is there");
    assert_eq!(Secrets::load(&path).unwrap().signing_key.to_bytes(), both.signing_key.to_bytes(), "nothing changed then");
}
