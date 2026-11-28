//! `POST /v1/reports/ai` (Report this reply): a reported reply is kept 30
//! days with nothing of who sent it; unsigned ones are limited by address,
//! signed ones by their Claktory's key, and all together by a ceiling a
//! day; anything but a clean report is refused. Claktory's team reads them
//! on the server with `claktory-account ai-reports`, which only reads.

use claktory_account::protocol::*;
use claktory_account::server::{self, db};
use serde_json::json;

use crate::support::*;

fn report(note: &str) -> AiReport {
    AiReport {
        text: "echo: something rude".into(),
        model: "Claude Opus 4.5".into(),
        version: "v0.1.30".into(),
        platform: "macos-arm64".into(),
        note: note.into(),
    }
}

fn kept(instance: &Instance) -> Vec<(i64, String, String, String, String, String)> {
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let mut statement = conn.prepare("SELECT received, text, model, version, platform, note FROM ai_reports ORDER BY id").expect("sql");
    statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)))
        .expect("sql")
        .collect::<rusqlite::Result<_>>()
        .expect("rows")
}

#[tokio::test]
async fn a_report_is_kept_30_days_with_nothing_of_who_sent_it() {
    let instance = start().await;
    let answer = instance.post(AI_REPORT_PATH, &report("It was rude."), "198.51.120.1").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.json::<AiReportKept>().keep_days, 30);
    let mut claktory = Claktory::new("198.51.120.2");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let answer = instance.signed(&claktory, "POST", AI_REPORT_PATH, &report(""), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());

    let rows = kept(&instance);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        (
            START - START.rem_euclid(3600),
            "echo: something rude".into(),
            "Claude Opus 4.5".into(),
            "v0.1.30".into(),
            "macos-arm64".into(),
            "It was rude.".into()
        )
    );
    // Signed or not, the same: no address, account, Claktory or key.
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");
    let columns: Vec<String> = conn
        .prepare("SELECT name FROM pragma_table_info('ai_reports')")
        .expect("sql")
        .query_map([], |row| row.get(0))
        .expect("sql")
        .collect::<rusqlite::Result<_>>()
        .expect("columns");
    assert_eq!(columns, ["id", "received", "text", "model", "version", "platform", "note"]);
    let dump: String = conn
        .prepare("SELECT key FROM limits")
        .expect("sql")
        .query_map([], |row| row.get::<_, String>(0))
        .expect("sql")
        .collect::<rusqlite::Result<Vec<_>>>()
        .expect("keys")
        .join(" ");
    assert!(!dump.contains(&claktory.id), "the limits name the Claktory by a hash only: {dump}");

    // Received to the hour only: 14:00 for one sent at 14:02.
    db::purge(&conn, START + 29 * 86_400).expect("purge");
    assert_eq!(kept(&instance).len(), 2);
    db::purge(&conn, START - START.rem_euclid(3600) + 30 * 86_400 + 1).expect("purge");
    assert!(kept(&instance).is_empty(), "gone after 30 days");
}

#[tokio::test]
async fn unsigned_reports_are_limited_by_address_and_signed_ones_by_key() {
    let instance = start().await;
    // An address: 10 an hour.
    for number in 0..10 {
        assert_eq!(instance.post(AI_REPORT_PATH, &report(""), "198.51.121.1").await.status, 200, "{number}");
    }
    let answer = instance.post(AI_REPORT_PATH, &report(""), "198.51.121.1").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert!(answer.value()["retry_after"].as_i64().is_some_and(|wait| wait > 0 && wait <= 3600));
    assert_eq!(instance.post(AI_REPORT_PATH, &report(""), "198.51.121.2").await.status, 200, "another address isn't");
    // And 50 a day (START is 14:02 UTC: the day goes on past 19:02).
    for hour in 1..5 {
        instance.advance(3600);
        for number in 0..10 {
            assert_eq!(instance.post(AI_REPORT_PATH, &report(""), "198.51.121.1").await.status, 200, "{hour} {number}");
        }
    }
    instance.advance(3600);
    assert_eq!(instance.post(AI_REPORT_PATH, &report(""), "198.51.121.1").await.error(), "rate_limited", "50 a day");

    // A signed-in Claktory: 20 a day, whatever its address has spent.
    let mut claktory = Claktory::new("198.51.121.1");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    for number in 0..20 {
        let answer = instance.signed(&claktory, "POST", AI_REPORT_PATH, &report(&format!("Number {number}.")), None).await;
        assert_eq!(answer.status, 200, "{number}: {}", answer.text());
    }
    let answer = instance.signed(&claktory, "POST", AI_REPORT_PATH, &report(""), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    // A bad signature isn't a report.
    let mut stranger = Claktory::new("198.51.121.9");
    stranger.token = claktory.token.clone();
    let answer = instance.signed(&stranger, "POST", AI_REPORT_PATH, &report(""), None).await;
    assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"));
}

#[tokio::test]
async fn past_the_days_ceiling_no_one_reports_until_tomorrow() {
    let instance = start().await;
    let day = START - START.rem_euclid(86_400);
    rusqlite::Connection::open(&instance.db)
        .expect("the database")
        .execute("INSERT INTO limits (key, window, count) VALUES ('ai-report-day', ?1, 2000)", [day])
        .expect("sql");
    let answer = instance.post(AI_REPORT_PATH, &report(""), "198.51.122.1").await;
    assert_eq!((answer.status, answer.error().as_str()), (429, "rate_limited"));
    assert_eq!(answer.value()["retry_after"], day + 86_400 - START);
    assert!(kept(&instance).is_empty());
    instance.advance(86_400);
    assert_eq!(instance.post(AI_REPORT_PATH, &report(""), "198.51.122.1").await.status, 200);
}

#[tokio::test]
async fn only_a_clean_report_is_kept() {
    let instance = start().await;
    let ip = "198.51.123.1";
    let mut extra = serde_json::to_value(report("")).expect("json");
    extra["account_id"] = json!("someone");
    let refused = |answer: Answer| assert_eq!((answer.status, answer.error()), (400, "invalid_request".to_owned()), "{answer:?}");
    refused(instance.post(AI_REPORT_PATH, &extra, ip).await);
    refused(instance.post(AI_REPORT_PATH, &json!({"text": "t", "model": "m"}), ip).await);
    refused(instance.post(AI_REPORT_PATH, &AiReport { text: "x".repeat(AI_REPORT_TEXT_MAX + 1), ..report("") }, ip).await);
    refused(instance.post(AI_REPORT_PATH, &AiReport { text: "x".repeat(50_000), ..report("") }, ip).await);
    refused(instance.post(AI_REPORT_PATH, &report(&"n".repeat(AI_REPORT_NOTE_MAX + 1)), ip).await);
    refused(instance.post(AI_REPORT_PATH, &report("\u{202E}reversed"), ip).await);
    refused(instance.post(AI_REPORT_PATH, &AiReport { model: "Opus\n4.5".into(), ..report("") }, ip).await);
    refused(instance.post(AI_REPORT_PATH, &AiReport { text: "  ".into(), ..report("") }, ip).await);
    assert!(kept(&instance).is_empty());
    let full = AiReport { text: "é".repeat(AI_REPORT_TEXT_MAX / 2), ..report(&"ü".repeat(AI_REPORT_NOTE_MAX)) };
    let answer = instance.post(AI_REPORT_PATH, &full, ip).await;
    assert_eq!(answer.status, 200, "a report at every cap: {}", answer.text());
    let code =
        AiReport { text: "\"\n".repeat(AI_REPORT_TEXT_MAX / 2), ..report(&format!("{}a", "a\n".repeat(AI_REPORT_NOTE_MAX / 2 - 1))) };
    let answer = instance.post(AI_REPORT_PATH, &code, ip).await;
    assert_eq!(answer.status, 200, "16 KB of quotes and line breaks, twice that in JSON: {}", answer.text());
}

#[tokio::test]
async fn the_team_reads_kept_reports_a_page_at_a_time_and_a_report_cant_act_on_their_terminal() {
    let instance = start().await;
    let hostile = AiReport {
        text: "fine\n#9 2026-01-01 00:00 UTC · Fake · v0 · nowhere\n\u{1b}]0;owned\u{7}\u{1b}[2J\u{9b}31m \u{202E}desrever\r".into(),
        ..report("Look\tclosely.")
    };
    for sent in [report("First."), hostile, report("")] {
        assert_eq!(instance.post(AI_REPORT_PATH, &sent, "198.51.124.1").await.status, 200);
    }
    let conn = rusqlite::Connection::open(&instance.db).expect("the database");

    let (page, more) = server::reports::kept(&conn, START, 0, 2).expect("kept");
    assert_eq!(page.iter().map(|kept| kept.id).collect::<Vec<_>>(), [1, 2]);
    assert!(more);
    assert_eq!((page[0].note.as_str(), page[0].model.as_str(), page[0].received), ("First.", "Claude Opus 4.5", START - START.rem_euclid(3600)));
    let (rest, more) = server::reports::kept(&conn, START, 2, 2).expect("kept");
    assert_eq!((rest.iter().map(|kept| kept.id).collect::<Vec<_>>(), more), (vec![3], false));
    assert_eq!(server::reports::text(&page, true).lines().last(), Some("More follow: --after 2"));

    let shown = server::reports::text(&page[1..], false);
    assert!(!shown.chars().any(|character| character != '\n' && character != '\t' && character.is_control()), "{shown:?}");
    assert!(!shown.contains('\u{202E}'), "{shown:?}");
    assert!(shown.contains("\\u{001B}]0;owned\\u{0007}\\u{001B}[2J\\u{009B}31m \\u{202E}desrever\\u{000D}"), "{shown}");
    assert!(shown.starts_with("#2 "), "{shown}");
    assert!(shown.lines().skip(1).all(|line| line.is_empty() || line.starts_with("  │ ") || line.ends_with(':')), "{shown}");
    assert!(shown.contains("  │ #9 2026-01-01"), "a reply's line can't pass for a report's heading: {shown}");

    // Past its 30 days, it's no longer kept, purged yet or not.
    let past = START - START.rem_euclid(3600) + 30 * 86_400 + 1;
    assert_eq!(server::reports::kept(&conn, past, 0, 50).expect("kept"), (vec![], false));
}

#[test]
fn ai_reports_only_reads_the_database() {
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join("account.db");
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("now").as_secs() as i64;
    {
        let conn = db::open(&path).expect("the database");
        for note in ["One.", "Two."] {
            db::add_ai_report(&conn, &report(note), now).expect("kept");
        }
    }
    let before = std::fs::read(&path).expect("the file");
    let run = |args: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_claktory-account"))
            .arg("ai-reports")
            .arg("--db")
            .arg(&path)
            .args(args)
            .output()
            .expect("ran");
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).expect("utf-8")
    };
    let answer: serde_json::Value = serde_json::from_str(&run(&["--format", "json", "--most", "1"])).expect("json");
    assert_eq!(answer["reports"].as_array().map(Vec::len), Some(1));
    assert_eq!((answer["reports"][0]["note"].as_str(), answer["next_after"].as_i64()), (Some("One."), Some(1)));
    let text = run(&["--after", "1"]);
    assert!(text.starts_with("#2 ") && text.contains("  │ Two.") && !text.contains("More follow"), "{text}");
    assert_eq!(std::fs::read(&path).expect("the file"), before, "nothing written");
}
