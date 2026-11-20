use claktory_account::server::{Secrets, db, stats};

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
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/emails", listener.local_addr().unwrap());
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
        let answer = format!("HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}");
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
    let email = mail::code_mail("auth.claktory.com", "matt@example.com", "482913");
    resend.send(&email).await.expect("sent");
    let request = request.await.unwrap();
    let (head, body) = request.split_once("\r\n\r\n").unwrap();
    assert!(head.starts_with("POST /emails ") && head.to_ascii_lowercase().contains("authorization: bearer re_key"), "{head}");
    let body: serde_json::Value = serde_json::from_str(body).unwrap();
    assert_eq!(body["from"], "Claktory <signin@auth.claktory.com>");
    assert_eq!(body["to"], serde_json::json!(["matt@example.com"]));
    assert_eq!(body["subject"], "Your Claktory code: 482913");
    assert_eq!(body["text"], email.text.as_str());
    assert_eq!(body["html"], email.html.as_str());

    let (url, _) = fake_resend("422 Unprocessable Entity").await;
    let error = mail::Resend::with_url("re_key".into(), url).send(&email).await.expect_err("refused");
    assert!(error.contains("422"), "{error}");
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
