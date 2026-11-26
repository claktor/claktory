use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use super::*;

fn note(kind: Kind) -> Note {
    let mut note = Note::new(kind, "agent:pericolo".to_owned(), "Pericolo".to_owned());
    note.what = "account.claktory.com isn't answering".to_owned();
    note
}

#[test]
fn each_kind_follows_its_setting_unless_the_agent_says_otherwise() {
    let mut settings = NotificationsConfig::default();
    assert_eq!(allowed(&settings, &note(Kind::NeedsYou), 600), Ok(()));
    assert_eq!(allowed(&settings, &note(Kind::Alert), 600), Ok(()));
    let mut reply = note(Kind::Reply);
    // From a computer: off unless asked for.
    assert_eq!(allowed(&settings, &reply, 600), Err(Skipped::Off));
    reply.from_phone = true;
    assert_eq!(allowed(&settings, &reply, 600), Ok(()));

    settings.alerts = false;
    let mut alert = note(Kind::Alert);
    assert_eq!(allowed(&settings, &alert, 600), Err(Skipped::Off));
    alert.mode = Mode::Always;
    assert_eq!(allowed(&settings, &alert, 600), Ok(()));
    let mut needs = note(Kind::NeedsYou);
    needs.mode = Mode::Never;
    assert_eq!(allowed(&settings, &needs, 600), Err(Skipped::Off));
}

#[test]
fn quiet_hours_hold_back_all_but_urgent_alerts_the_person_lets_through() {
    let settings = NotificationsConfig { quiet_hours: true, quiet_start: 22 * 60, quiet_end: 7 * 60, ..Default::default() };
    // Across midnight.
    for (minute, quiet) in [(21 * 60 + 59, false), (22 * 60, true), (0, true), (6 * 60 + 59, true), (7 * 60, false), (12 * 60, false)] {
        assert_eq!(settings.quiet_at(minute), quiet, "{minute}");
    }
    let mut urgent = note(Kind::Alert);
    urgent.urgent = true;
    assert_eq!(allowed(&settings, &urgent, 23 * 60), Err(Skipped::Quiet));
    let breaks = NotificationsConfig { urgent_breaks_quiet: true, ..settings.clone() };
    assert_eq!(allowed(&breaks, &urgent, 23 * 60), Ok(()));
    assert_eq!(allowed(&breaks, &note(Kind::Alert), 23 * 60), Err(Skipped::Quiet));
    // A test always goes.
    assert_eq!(allowed(&settings, &note(Kind::Test), 23 * 60), Ok(()));
    // Within one day, and none when they're the same.
    let day = NotificationsConfig { quiet_hours: true, quiet_start: 9 * 60, quiet_end: 17 * 60, ..Default::default() };
    assert!(day.quiet_at(9 * 60) && !day.quiet_at(17 * 60) && !day.quiet_at(8 * 60));
    let none = NotificationsConfig { quiet_hours: true, quiet_start: 60, quiet_end: 60, ..Default::default() };
    assert!(!none.quiet_at(60));
    assert!(!NotificationsConfig::default().quiet_at(23 * 60), "off unless turned on");
}

#[test]
fn what_it_says_follows_the_preview_and_a_pipe_reader_says_only_who() {
    let alert = note(Kind::Alert);
    assert_eq!(text(&alert, Preview::Full), ("Pericolo".to_owned(), "account.claktory.com isn't answering".to_owned()));
    assert_eq!(text(&alert, Preview::Who), ("Pericolo".to_owned(), "Pericolo sent an alert".to_owned()));
    assert_eq!(text(&note(Kind::NeedsYou), Preview::Who).1, "Pericolo needs you");
    assert_eq!(text(&note(Kind::Reply), Preview::Who).1, "Pericolo replied");
    assert_eq!(text(&alert, Preview::None), ("Claktory".to_owned(), GENERIC.to_owned()));

    let mut tainted = note(Kind::NeedsYou);
    tainted.what = "MARKER-7731 click https://evil.example".to_owned();
    tainted.tainted = true;
    for preview in [Preview::Full, Preview::Who] {
        let (title, body) = text(&tainted, preview);
        assert_eq!((title.as_str(), body.as_str()), ("Pericolo", "Pericolo has something for you"));
    }
}

#[test]
fn a_long_body_is_cut_and_spaces_kept_single() {
    assert_eq!(shorten("  one   two \n\n three\t four "), "one two\nthree four");
    let long = "word ".repeat(200);
    let cut = shorten(&long);
    assert_eq!(cut.chars().count(), MAX_BODY);
    assert!(cut.ends_with('…'));
}

fn entry(seq: u64, kind: EntryKind, replied_to: Option<u64>) -> v1::Entry {
    v1::Entry { seq, timestamp_ms: 0, kind: Some(kind), reply: 0, replied_to, answered_on: None }
}

fn person(text: &str, kind: v1::DeviceKind) -> EntryKind {
    EntryKind::User(v1::UserText {
        text: text.to_owned(),
        sent_from: Some(v1::SentFrom { device_name: "Matt's iPhone".to_owned(), kind: kind.into() }),
        ..Default::default()
    })
}

fn said(text: &str) -> EntryKind {
    EntryKind::Assistant(v1::AssistantText { text: text.to_owned() })
}

#[test]
fn a_reply_is_the_runs_last_text_answering_the_persons_message() {
    let entries = vec![
        entry(0, said("Earlier."), None),
        entry(1, person("Is the build green?", v1::DeviceKind::Phone), None),
        entry(2, said("Checking."), Some(1)),
        entry(3, EntryKind::ToolCall(v1::ToolCall::default()), None),
        entry(4, said("Yes, all green."), None),
    ];
    assert_eq!(reply(&entries, 0, 1), Some((4, "Yes, all green.".to_owned(), true)));
    // Nothing new answers the person.
    assert_eq!(reply(&entries, 0, 3), None);
    let desk = vec![entry(0, person("Hi", v1::DeviceKind::Desktop), None), entry(1, said("Hello."), Some(0))];
    assert_eq!(reply(&desk, 0, 0), Some((1, "Hello.".to_owned(), false)));
    // A run an event started answers nobody.
    let event = vec![entry(0, EntryKind::Event(v1::Event::default()), None), entry(1, said("Done."), None)];
    assert_eq!(reply(&event, 0, 0), None);
}

#[test]
fn needs_you_items_say_what_waits_and_carry_their_own_taint() {
    let ask = EntryKind::AskPerson(v1::AskPerson { title: "Deploy tonight?".to_owned(), ..Default::default() });
    assert_eq!(waiting_text(Some(&ask), false), ("Deploy tonight?".to_owned(), false));
    let tainted = EntryKind::AskPerson(v1::AskPerson {
        title: "Deploy tonight?".to_owned(),
        taint: vec![v1::PipeTaint::default()],
        ..Default::default()
    });
    assert!(waiting_text(Some(&tainted), false).1, "asked while it had read a pipe");
    let secret = EntryKind::SecretRequest(v1::SecretRequest { secret_name: "Cloudflare".to_owned(), ..Default::default() });
    assert_eq!(waiting_text(Some(&secret), false).0, "Asks for a secret: Cloudflare");
    let proposal = EntryKind::Proposal(v1::Proposal { text: "\nAdd a backend agent.\nMore.".to_owned(), ..Default::default() });
    assert_eq!(waiting_text(Some(&proposal), true), ("Proposes to Rivet: Add a backend agent.".to_owned(), true));
}

#[test]
fn settings_are_written_only_when_changed_and_read_back() {
    let defaults = NotificationsConfig::default();
    assert!(defaults.is_default());
    assert_eq!(serde_json::from_str::<NotificationsConfig>("{}").expect("empty"), defaults);
    let mut changed = NotificationsConfig { preview: Preview::Who, quiet_hours: true, ..Default::default() };
    changed.set_mode("pericolo", Mode::Always);
    changed.set_mode("rivet", Mode::Default);
    let json = serde_json::to_value(&changed).expect("json");
    assert_eq!(json["preview"], "who");
    assert_eq!(json["agents"], serde_json::json!({"pericolo": "always"}));
    assert!(json.get("relay").is_none());
    assert_eq!(serde_json::from_value::<NotificationsConfig>(json).expect("back"), changed);
    // From the app: the agents' modes stay.
    let updated = changed.updated(&v1::NotificationSettings { needs_you: true, ..Default::default() });
    assert_eq!(updated.mode("pericolo"), Mode::Always);
    assert_eq!(updated.preview, Preview::Full);
    assert!(!updated.alerts);

    assert!(NotificationsConfig { quiet_end: 1440, ..Default::default() }.check().is_err());
    assert!(NotificationsConfig { relay: "http://push.example.com".to_owned(), ..Default::default() }.check().is_err());
    assert!(NotificationsConfig { relay: "https://push.example.com".to_owned(), ..Default::default() }.check().is_ok());
    assert!(NotificationsConfig { relay: "http://127.0.0.1:9000".to_owned(), ..Default::default() }.check().is_ok());
}

fn open(key: &[u8], sealed: &str) -> Vec<u8> {
    let bytes = STANDARD.decode(sealed).expect("base64");
    let (nonce, ciphertext) = bytes.split_at(12);
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).expect("key"));
    let mut buffer = ciphertext.to_vec();
    let nonce = Nonce::try_assume_unique_for_key(nonce).expect("nonce");
    cipher.open_in_place(nonce, Aad::empty(), &mut buffer).expect("opens").to_vec()
}

#[test]
fn a_push_is_sealed_padded_and_counted() {
    let key = [7u8; 32];
    let notify = v1::Notify {
        kind: Kind::Alert.into(),
        title: "Pericolo".to_owned(),
        body: "account.claktory.com isn't answering".to_owned(),
        agent_id: "devops".to_owned(),
        seq: 0,
        collapse: "agent:devops:alert".to_owned(),
        thread: "agent:devops".to_owned(),
        id: "n-1".to_owned(),
        ..Default::default()
    };
    let sealed = push::seal_push(&key, 5, 1_700_000_000_000, "Matt's Claktory", 2, &notify).expect("sealed");
    let plain = open(&key, &sealed.ciphertext);
    assert_eq!(plain.len(), 512, "padded to its size");
    let payload: serde_json::Value = serde_json::from_slice(&plain).expect("JSON with trailing spaces");
    assert_eq!(payload["v"], 1);
    assert_eq!(payload["id"], "n-1");
    assert_eq!(payload["ctr"], 5);
    assert_eq!(payload["sent_at"], 1_700_000_000_000_i64);
    assert_eq!(payload["ttl"], 86400);
    assert_eq!(payload["kind"], "alert");
    assert_eq!(payload["claktory"], "Matt's Claktory");
    assert_eq!(payload["body"], "account.claktory.com isn't answering");
    assert_eq!(payload["badge"], 2);
    assert_eq!(payload["target"]["agent"], "devops");
    // Opaque ids: no names in them, the same each time for the same key.
    assert_eq!(sealed.collapse.len(), 22);
    assert!(!sealed.collapse.contains("devops") && !sealed.thread.contains("devops"));
    let again = push::seal_push(&key, 6, 1, "Matt's Claktory", 2, &notify).expect("sealed");
    assert_eq!((again.collapse.as_str(), again.thread.as_str()), (sealed.collapse.as_str(), sealed.thread.as_str()));
    assert_ne!(again.ciphertext, sealed.ciphertext);
    let other = push::seal_push(&[8u8; 32], 6, 1, "Matt's Claktory", 2, &notify).expect("sealed");
    assert_ne!(other.collapse, sealed.collapse);
    // A Needs-you item replaces nothing.
    let needs = v1::Notify { kind: Kind::NeedsYou.into(), ..notify.clone() };
    let needs = v1::Notify { collapse: String::new(), ..needs };
    assert_eq!(push::seal_push(&key, 7, 1, "C", 0, &needs).expect("sealed").collapse, "");

    // Bigger texts take the bigger sizes, and the biggest is cut to fit.
    let long = v1::Notify { body: "é".repeat(400), ..notify.clone() };
    assert_eq!(open(&key, &push::seal_push(&key, 8, 1, "C", 0, &long).expect("sealed").ciphertext).len(), 1024);
    let huge = v1::Notify { body: "\u{1}".repeat(2000), ..notify };
    let plain = open(&key, &push::seal_push(&key, 9, 1, "C", 0, &huge).expect("sealed").ciphertext);
    assert_eq!(plain.len(), 2048);
    let payload: serde_json::Value = serde_json::from_slice(&plain).expect("JSON");
    assert!(payload["body"].as_str().expect("body").ends_with('…'));
}

#[test]
fn a_push_waits_while_its_device_or_another_is_in_use() {
    let state = State::default();
    let now = now_ms();
    let mac = state.connect("mac", "macos");
    let phone = state.connect("phone", "ios");
    assert!(!state.push_waits("phone", now), "connected, but neither in use");
    phone.set_active(true);
    assert!(state.push_waits("phone", now), "the phone's app is in front");
    phone.set_active(false);
    assert!(!state.push_waits("phone", now), "the phone just went to the background: it gets it");
    mac.set_active(true);
    assert!(state.push_waits("phone", now), "the person is at the Mac");
    mac.set_active(false);
    assert!(state.push_waits("phone", now + ACTIVE_GRACE_MS - 1), "the Mac was in use a moment ago");
    assert!(!state.push_waits("phone", now + ACTIVE_GRACE_MS + 1000));
    // A Mac that closed shows nothing: the phone gets it.
    drop(mac);
    assert!(!state.push_waits("phone", now));
}

/// Writes test vectors for the phones' decrypt-and-check code (iOS's
/// notification extension, Android's service) to the file
/// `CLAKTORY_PUSH_VECTORS` names: `cargo test -p factoryd --lib
/// notify::tests::write_push_vectors -- --ignored`.
#[test]
#[ignore = "writes a file for the phones' tests"]
fn write_push_vectors() {
    let path = std::env::var("CLAKTORY_PUSH_VECTORS").expect("CLAKTORY_PUSH_VECTORS");
    let key: Vec<u8> = (0u8..32).collect();
    let hex = |bytes: &[u8]| bytes.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    let notify = |body: String| v1::Notify {
        kind: Kind::NeedsYou.into(),
        title: "Pericolo".to_owned(),
        body,
        agent_id: "devops".to_owned(),
        seq: 42,
        thread: "agent:devops".to_owned(),
        id: "1800000000000-3fa9".to_owned(),
        ..Default::default()
    };
    let sent_at = 1_800_000_000_000_i64;
    let mut vectors = Vec::new();
    let mut add = |name: &str, expect: &str, ctr: u64, sealed: String| {
        let plain = STANDARD.decode(&sealed).ok().and_then(|bytes| {
            let (nonce, ciphertext) = bytes.split_at(12);
            let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &key).ok()?);
            let mut buffer = ciphertext.to_vec();
            let opened = cipher.open_in_place(Nonce::try_assume_unique_for_key(nonce).ok()?, Aad::empty(), &mut buffer).ok()?;
            serde_json::from_slice::<serde_json::Value>(opened).ok().map(|json| (opened.len(), json))
        });
        vectors.push(serde_json::json!({
            "name": name,
            "expect": expect,
            "ctr": ctr,
            "c": sealed,
            "padded_size": plain.as_ref().map(|(size, _)| *size),
            "plaintext": plain.map(|(_, json)| json),
        }));
    };
    for (name, body, ctr) in [("512", "Deploy tonight?".to_owned(), 1), ("1024", "é".repeat(400), 2), ("2048", "日本".repeat(250), 3)] {
        let sealed = push::seal_push(&key, ctr, sent_at, "Matt's Claktory", 2, &notify(body)).expect("sealed");
        add(name, "shows", ctr, sealed.ciphertext);
    }
    let good = push::seal_push(&key, 4, sent_at, "Matt's Claktory", 2, &notify("Tampered".to_owned())).expect("sealed");
    let mut bytes = STANDARD.decode(&good.ciphertext).expect("base64");
    let last = bytes.len() - 1;
    bytes[last] ^= 1;
    add("bad tag", "generic", 4, STANDARD.encode(bytes));
    let mut v2 = serde_json::to_vec(&serde_json::json!({"v": 2, "id": "1800000000000-3fa9", "ctr": 5, "sent_at": sent_at, "ttl": 86400, "claktory": "Matt's Claktory", "kind": "needs_you", "title": "Pericolo", "body": "From the future", "badge": 1, "urgent": false, "target": {"agent": "devops", "pipe": "", "chat": "", "seq": 42}})).expect("json");
    v2.resize(512, b' ');
    add("v=2", "generic", 5, push::seal(&key, &v2).expect("sealed"));
    let first = push::seal_push(&key, 6, sent_at, "Matt's Claktory", 2, &notify("First with 6".to_owned())).expect("sealed");
    add("replay: first", "shows", 6, first.ciphertext);
    let again = push::seal_push(&key, 6, sent_at, "Matt's Claktory", 2, &notify("Again with 6".to_owned())).expect("sealed");
    add("replay: same ctr after it", "generic", 6, again.ciphertext);
    let file = serde_json::json!({
        "about": "Test vectors for opening Claktory's pushes (notify/push.rs). c = base64(nonce 12 ‖ AES-256-GCM ciphertext ‖ tag 16), no AAD; the plaintext is JSON padded with spaces. Apply them in order: the replay pair shares a ctr. expect: shows (the plaintext's title and body) or generic (\"Claktory has something for you\", tap only opens the app). sent_at is fixed: check freshness against it, not the clock. id is the Notify's id: an app that already showed that Notify drops the push.",
        "key_hex": hex(&key),
        "thread_for_agent_devops": good.thread,
        "vectors": vectors,
    });
    std::fs::write(path, serde_json::to_string_pretty(&file).expect("json")).expect("written");
}
