//! The warning signs over seeded records: each turns red when it holds in
//! the last 6 hours and the 6 before them, and only then.

use std::collections::HashMap;

use rusqlite::{Connection, params};

use super::metrics::{HOUR_MS, Member, Team};
use super::signs::{self, Sign};
use crate::store::Store;

const END: i64 = 1_800_000_000_000;
const MIN: i64 = 60_000;

struct Records {
    store: Store,
    path: std::path::PathBuf,
}

impl Drop for Records {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn records() -> Records {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "swarm-test-{}-{}.db",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&path);
    Records { store: Store::open(&path).expect("store"), path }
}

fn team(members: &[(&str, &str)]) -> Team {
    Team {
        name: "Site".to_owned(),
        members: members.iter().map(|(id, role)| Member { id: (*id).to_owned(), role: (*role).to_owned(), leads: false }).collect(),
        ttl_ms: std::collections::BTreeMap::new(),
        compact_at: 400_000,
    }
}

fn run(db: &Connection, agent: &str, started: i64, ended: i64) {
    db.execute("INSERT INTO runs (agent, version, started, ended, outcome) VALUES (?1, 't', ?2, ?3, 'finished')", params![agent, started, ended]).expect("run");
}

fn message(db: &Connection, from: &str, to: &str, created: i64) {
    db.execute(
        "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created) VALUES (?1, 'agent', ?2, ?2, '', '', ?3)",
        params![to, from, created],
    )
    .expect("message");
}

/// The signs at END: those red, and those only in the last 6 hours.
fn signs(db: &Connection, team: &Team) -> (Vec<String>, Vec<String>) {
    let (red, now) = signs::red(db, team, END, END, &HashMap::new()).expect("signs");
    let keys = |signs: Vec<Sign>| signs.into_iter().map(|sign| sign.key).collect();
    (keys(red), keys(now))
}

#[test]
fn the_busiest_agent_on_a_tie_is_the_one_written_to() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("lea", "Backend"), ("solo", "Backend")]);
    for i in 0..35 {
        message(&db, "lea", "solo", END - 20 * 60 * MIN + i);
    }
    let traffic = super::metrics::traffic(&db, &team, super::metrics::Window::ending(END, 24.0)).expect("traffic");
    assert_eq!((traffic.hub.as_str(), traffic.hub_sent, traffic.hub_received, traffic.hub_senders), ("solo", 0, 35, 1));
}

#[test]
fn provenance_counts_only_what_senders_said() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("lea", "Backend"), ("bo", "Frontend")]);
    message(&db, "lea", "bo", END - 3 * 60 * MIN);
    for (to, reply_to, intent) in [("bo", "[]", "handoff"), ("lea", "[5]", "")] {
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, send_id, sender_run, reply_to, intent, taint)
             VALUES (?1, 'agent', 'Lea', 'lea', '', '', ?2, 's1', 1, ?3, ?4, '{}')",
            params![to, END - 60 * MIN, reply_to, intent],
        )
        .expect("message");
    }
    db.execute(
        "INSERT INTO quiet_copies (agent, send_id, sender_id, sender, text, to_ids, reply_to, intent, taint, created)
         VALUES ('bo', 's1', 'lea', 'Lea', '', '[]', '[]', '', '{}', ?1)",
        [END - 60 * MIN],
    )
    .expect("copy");
    let traffic = super::metrics::traffic(&db, &team, super::metrics::Window::ending(END, 6.0)).expect("traffic");
    let known = traffic.provenance.expect("known");
    assert_eq!((known.recorded, known.sends, known.answers, known.handoffs, known.quiet_copies), (2, 1, 1, 1, 1));
}

#[test]
fn coordination_concentrated_at_a_hub_is_red_only_in_both_windows() {
    let records = records();
    let db = records.store.db();
    let ids = ["lead", "a", "b", "c", "d", "e"];
    let team = team(&ids.map(|id| (id, "")));
    // Working, so it isn't idle.
    run(&db, "lead", END - 13 * HOUR_MS, END);
    // 120 messages in the last 6 hours, 60 of them to the lead from 5 agents.
    for i in 0..60 {
        let peer = ids[1 + i as usize % 5];
        message(&db, peer, "lead", END - HOUR_MS + i * MIN / 2);
        message(&db, "lead", peer, END - HOUR_MS + i * MIN / 2 + 1000);
    }
    assert_eq!(signs(&db, &team), (vec![], vec!["concentrated:lead".to_owned()]));
    // The same in the 6 hours before: red.
    for i in 0..60 {
        let peer = ids[1 + i as usize % 5];
        message(&db, peer, "lead", END - 7 * HOUR_MS + i * MIN / 2);
        message(&db, "lead", peer, END - 7 * HOUR_MS + i * MIN / 2 + 1000);
    }
    assert_eq!(signs(&db, &team), (vec!["concentrated:lead".to_owned()], vec![]));
}

#[test]
fn short_runs_and_slow_answers_at_low_load_are_coordination() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("a", ""), ("b", "")]);
    // 25 runs of 10 seconds in each window.
    for window in [0, 6] {
        for i in 0..25 {
            run(&db, "a", END - (window + 5) * HOUR_MS + i * 10 * MIN, END - (window + 5) * HOUR_MS + i * 10 * MIN + 10_000);
        }
    }
    let (red, _) = signs(&db, &team);
    assert_eq!(red, ["coordination"]);
}

#[test]
fn a_saturated_role_slow_to_answer_with_work_ready_needs_capacity() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("be", "Backend"), ("pm", "Lead")]);
    // Busy all 12 hours.
    run(&db, "be", END - 13 * HOUR_MS, END);
    // In each window, it answers 5 times after 65 minutes; the lead at once.
    for window in [0, 6] {
        let start = END - window * HOUR_MS - 350 * MIN;
        for i in 0..5 {
            message(&db, "pm", "be", start + i * 66 * MIN);
            message(&db, "be", "pm", start + i * 66 * MIN + 65 * MIN);
        }
    }
    for id in 1..=3 {
        db.execute("INSERT INTO checklist_items (agent, id, text, status, owner, created, updated) VALUES ('pm', ?1, '', 'open', 'be', 0, 0)", [id])
            .expect("item");
    }
    let (red, _) = signs(&db, &team);
    assert_eq!(red, ["capacity:Backend"]);
    // With nothing ready, it isn't.
    db.execute("UPDATE checklist_items SET status = 'done'", []).expect("done");
    assert_eq!(signs(&db, &team).0, Vec::<String>::new());
}

#[test]
fn the_person_over_asked_and_idle_agents_and_wake_ups() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("a", ""), ("idle", "")]);
    // 12 questions, 10 hours ago.
    for _ in 0..12 {
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, decided) VALUES ('a', '', '', 0, ?1, 'answered', '', ?1)",
            [END - 10 * HOUR_MS],
        )
        .expect("ask");
    }
    // An agent sent 40 messages in 3 days that never works.
    for i in 0..40 {
        message(&db, "a", "idle", END - 60 * HOUR_MS + i * MIN);
    }
    // Woken every 20 minutes for a day, past the 5-minute cache, writing it
    // all again each time.
    for i in 0..72 {
        run(&db, "a", END - 24 * HOUR_MS + i * 20 * MIN, END - 24 * HOUR_MS + i * 20 * MIN + 2 * MIN);
    }
    db.execute(
        "INSERT INTO agent_usage (hour, provider, model, account, agent, agent_name, requests, input, output, cache_read, cache_write)
         VALUES (?1, 'anthropic', 'm', '', 'a', 'A', 72, 1000, 10000, 1000000, 5000000)",
        [(END - 10 * HOUR_MS) / 1000 / 3600 * 3600],
    )
    .expect("usage");
    let (red, _) = signs(&db, &team);
    assert_eq!(red, ["over_asked", "wake_cost", "idle:idle"]);
}

/// Rivet's report (Bruno 25299): new agents were judged over 3 days they
/// hadn't lived, and agents working on request over the wake-ups they
/// answered.
#[test]
fn new_agents_and_agents_that_answer_their_wake_ups_arent_idle() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("lead", ""), ("newborn", ""), ("young", ""), ("writer", ""), ("copied", ""), ("slacker", "")]);
    let woken = |to: &str, from_hours_ago: i64| {
        for i in 0..40 {
            message(&db, "lead", to, END - from_hours_ago * HOUR_MS + i * 30 * MIN);
        }
    };
    // Under a day old: not judged yet.
    woken("newborn", 20);
    // 30 hours old and busy 3 of them: 10% of its life (4% of 3 days).
    woken("young", 30);
    run(&db, "young", END - 10 * HOUR_MS, END - 7 * HOUR_MS);
    // Woken 40 times over 2 and a half days, never busy long, but each
    // time it answers within minutes: it works on request.
    woken("writer", 60);
    for i in 0..40 {
        message(&db, "writer", "lead", END - 60 * HOUR_MS + i * 30 * MIN + 5 * MIN);
    }
    // Copied in quietly 40 times: that doesn't wake it.
    for i in 0..40 {
        db.execute(
            "INSERT INTO messages (agent, source, sender, sender_id, summary, text, created, quiet) VALUES ('copied', 'agent', 'lead', 'lead', '', '', ?1, 1)",
            [END - 60 * HOUR_MS + i * 30 * MIN],
        )
        .expect("quiet");
    }
    // 40 hours old, never busy, never answering: idle over its own life.
    woken("slacker", 40);
    let (red, _) = signs(&db, &team);
    assert_eq!(red, ["idle:slacker"]);
    let (red, _) = signs::red(&db, &team, END, END, &HashMap::new()).expect("signs");
    assert_eq!(
        red[0].says,
        "Idle: slacker was busy 0% of the 40 hours since it started with nothing ready, yet was woken 40 times by messages and events not from the person, and answered 0 of them within the hour."
    );

    // Wake-ups answered only a day and more later weren't answered.
    db.execute("UPDATE messages SET created = ?1 WHERE sender_id = 'writer'", [END - HOUR_MS]).expect("later");
    assert_eq!(signs(&db, &team).0, ["idle:writer", "idle:slacker"]);
}

#[test]
fn questions_a_clear_closed_are_neither_decided_nor_dismissed() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("a", "")]);
    let ask = |state: &str, decided: i64| {
        db.execute(
            "INSERT INTO asks (agent, title, body, wants_text, created, state, answer, decided) VALUES ('a', '', '', 0, ?1, ?2, '', ?3)",
            params![END - HOUR_MS, state, decided],
        )
        .expect("ask");
    };
    ask("answered", END - HOUR_MS + 2 * MIN);
    ask("dismissed", END - HOUR_MS + 4 * MIN);
    ask("cleared", END - 1);
    let cards = super::metrics::cards(&db, &team, super::metrics::Window::ending(END, 6.0), END).expect("cards");
    assert_eq!((cards.raised, cards.decided, cards.dismissed), (3, 2, 1));
    assert_eq!(cards.decided_p90_min, Some(4.0), "a /clear's close isn't a time to decide");
}

#[test]
fn a_silent_wait_six_hours_ago_is_rebuilt_from_the_history() {
    let records = records();
    let db = records.store.db();
    let team = team(&[("a", ""), ("b", "")]);
    let event = |agent: &str, item: i64, time: i64, kind: &str, value: &str| {
        db.execute(
            "INSERT INTO checklist_events (agent, item, time, actor, kind, value) VALUES (?1, ?2, ?3, ?1, ?4, ?5)",
            params![agent, item, time, kind, value],
        )
        .expect("event");
    };
    let item = |agent: &str, id: i64, status: &str, blocked_on: &str| {
        db.execute(
            "INSERT INTO checklist_items (agent, id, text, status, owner, created, updated, blocked_on) VALUES (?1, ?2, '', ?3, '', 0, 0, ?4)",
            params![agent, id, status, blocked_on],
        )
        .expect("item");
    };
    // Blocked on the person from 20 hours ago until 2 hours ago: a silent
    // wait 6 hours ago, none now.
    item("a", 1, "open", "");
    event("a", 1, END - 20 * HOUR_MS, "status", "blocked");
    event("a", 1, END - 20 * HOUR_MS, "blocked_on", "person");
    event("a", 1, END - 2 * HOUR_MS, "status", "open");
    assert_eq!(signs(&db, &team), (vec![], vec![]));
    // Blocked all along, with a card raised an hour ago: silent then, not now.
    item("b", 1, "blocked", "person");
    event("b", 1, END - 20 * HOUR_MS, "status", "blocked");
    event("b", 1, END - 20 * HOUR_MS, "blocked_on", "person");
    db.execute("INSERT INTO asks (agent, title, body, wants_text, created, state, answer) VALUES ('b', '', '', 0, ?1, 'pending', '')", [END - HOUR_MS])
        .expect("ask");
    assert_eq!(signs(&db, &team), (vec![], vec![]));
    // The card answered: silent in both.
    db.execute("UPDATE asks SET state = 'answered', decided = ?1", [END - 30 * MIN]).expect("answered");
    assert_eq!(signs(&db, &team), (vec!["person_blind".to_owned()], vec![]));
}

/// Writes swarm_report's measures, as JSON, for records and settings named
/// by the environment, at a fixed end, so they can be compared with the
/// playbook's SQL on the same copy: CLAKTORY_SWARM_DB (a copy of
/// factory.db), CLAKTORY_SWARM_CONFIG (config.json), CLAKTORY_SWARM_END (ms
/// since 1970), CLAKTORY_SWARM_HOURS (6 unless given), CLAKTORY_SWARM_PROJECT
/// (everyone unless given) and CLAKTORY_SWARM_OUT (the file to write):
/// `cargo test -p factoryd --lib swarm::tests::write_report -- --ignored`.
#[test]
#[ignore = "reads a copy of someone's records"]
fn write_report() {
    let var = |name: &str| std::env::var(name).ok().filter(|value| !value.is_empty());
    // A scratch copy, brought up to this version's schema as an update
    // would (an older Claktory's records lack `blocked_on`, say).
    let copy = std::env::temp_dir().join(format!("swarm-report-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&copy);
    let source = Connection::open_with_flags(var("CLAKTORY_SWARM_DB").expect("CLAKTORY_SWARM_DB"), rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
        .expect("the records");
    source.execute("VACUUM INTO ?1", [copy.display().to_string()]).expect("copied");
    drop(source);
    let db = Connection::open(&copy).expect("the copy");
    let typed: bool = db.query_row("SELECT COUNT(*) FROM pragma_table_info('checklist_items') WHERE name = 'blocked_on'", [], |row| row.get(0)).expect("columns");
    if !typed {
        db.execute_batch(crate::extensions::checklist::BLOCKED_ON).expect("migrated");
    }
    let config_path = std::path::PathBuf::from(var("CLAKTORY_SWARM_CONFIG").expect("CLAKTORY_SWARM_CONFIG"));
    let text = std::fs::read_to_string(&config_path).expect("config.json");
    let config = crate::config::Config::parse(&text, config_path.parent().expect("its folder")).expect("settings");
    let end: i64 = var("CLAKTORY_SWARM_END").expect("CLAKTORY_SWARM_END").parse().expect("ms");
    let hours: f64 = var("CLAKTORY_SWARM_HOURS").map_or(6.0, |hours| hours.parse().expect("hours"));
    let (team, names) = super::team(&config, var("CLAKTORY_SWARM_PROJECT").as_deref()).expect("team");
    let report = super::measure(&db, &team, &names, super::metrics::Window::ending(end, hours), end).expect("measured");
    let mut json = serde_json::to_value(&report).expect("json");
    json["members"] = serde_json::json!(team.members.iter().map(|member| &member.id).collect::<Vec<_>>());
    json["text"] = serde_json::json!(super::text(&report));
    std::fs::write(var("CLAKTORY_SWARM_OUT").expect("CLAKTORY_SWARM_OUT"), serde_json::to_string_pretty(&json).expect("json")).expect("written");
    drop(db);
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", copy.display()));
    }
}
