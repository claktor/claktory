//! Agents' characters: who each one looks like, and the mood Factory shows.

use std::time::Duration;

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, CharacterMood, client_message, server_message::Body};

const SHAPES: [&str; 6] = ["circle", "square", "triangle", "diamond", "pentagon", "hexagon"];

/// 32-bit FNV-1a, as the spec gives it.
fn fnv(id: &str) -> u32 {
    let mut hash = 0x811c_9dc5_u32;
    for byte in id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

fn update(agent: &str, change: impl FnOnce(&mut v1::UpdateAgent)) -> client_message::Body {
    let mut request = v1::UpdateAgent { agent_id: agent.into(), ..Default::default() };
    change(&mut request);
    client_message::Body::UpdateAgent(request)
}

/// Its shape, seed and chosen shape; faces are gone, so every agent's is empty.
fn identity(view: &View, id: &str) -> (String, u32, Option<String>) {
    let agent = view.agent(id);
    assert_eq!((agent.avatar_face.as_str(), agent.avatar_face_override.as_deref()), ("", None), "{id} has a face");
    (agent.avatar_shape.clone(), agent.character_seed, agent.avatar_shape_override.clone())
}

fn saved_avatar(dir: &std::path::Path, id: &str) -> serde_json::Value {
    let config: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("home/config.json")).expect("config")).expect("json");
    config["agents"].as_array().expect("agents").iter().find(|agent| agent["id"] == id).expect("agent")["avatar"].clone()
}

#[tokio::test]
async fn a_character_is_its_ids_unless_chosen_and_keeps_through_renames_moves_and_restarts() {
    let dir = temp_dir();
    let (addr, _old) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    // Rivet starts as a blue hexagon.
    assert_eq!(identity(&view, "rivet"), ("hexagon".into(), fnv("rivet"), Some("hexagon".into())));
    assert_eq!(view.agent("rivet").color, "blue");
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_project("App", &dir.join("app"))).await;
    for name in ["Builder", "Writer"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    let seed = fnv("builder");
    let automatic = (SHAPES[seed as usize % 6].to_owned(), seed, None);
    assert_eq!(identity(&view, "builder"), automatic);
    assert_eq!(view.agent("writer").character_seed, fnv("writer"));
    assert_eq!(saved_avatar(&dir, "builder"), serde_json::Value::Null, "automatic isn't saved");

    // Renamed, moved and reordered, it's the same character.
    apply_request(&mut client, &mut view, update("builder", |change| change.name = Some("Maker".into()))).await;
    apply_request(&mut client, &mut view, update("builder", |change| change.project_id = Some("app".into()))).await;
    assert_eq!(identity(&view, "builder"), automatic);

    // Chosen in its settings (a face asked for is ignored), and back to automatic.
    let choose = |change: &mut v1::UpdateAgent| {
        change.avatar_shape = Some("pentagon".into());
        change.avatar_face = Some("curious".into());
    };
    apply_request(&mut client, &mut view, update("builder", choose)).await;
    assert_eq!(identity(&view, "builder"), ("pentagon".into(), seed, Some("pentagon".into())));
    assert_eq!(saved_avatar(&dir, "builder"), serde_json::json!({"shape": "pentagon"}));
    apply_request(&mut client, &mut view, update("builder", |change| change.avatar_shape = Some(String::new()))).await;
    assert_eq!(identity(&view, "builder"), automatic);
    assert_eq!(saved_avatar(&dir, "builder"), serde_json::Value::Null);

    // A shape of before (or one this Factory doesn't know) is kept, and
    // drawn as automatic; an old face is still read, and dropped when the
    // file is next saved.
    let path = dir.join("home/config.json");
    let mut config: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path).expect("config")).expect("json");
    let agents = config["agents"].as_array_mut().expect("agents");
    let builder = agents.iter_mut().find(|agent| agent["id"] == "builder").expect("builder");
    builder["avatar"] = serde_json::json!({"shape": "cloud", "face": "curious"});
    std::fs::write(&path, serde_json::to_string_pretty(&config).expect("json")).expect("write");
    view.follow(&mut client, |view| view.agent("builder").avatar_shape_override.as_deref() == Some("cloud")).await;
    assert!(view.workspace.config_error.is_empty(), "{}", view.workspace.config_error);
    assert_eq!(identity(&view, "builder"), (automatic.0.clone(), seed, Some("cloud".into())));
    apply_request(&mut client, &mut view, update("builder", |change| change.name = Some("Builder".into()))).await;
    assert_eq!(saved_avatar(&dir, "builder"), serde_json::json!({"shape": "cloud"}));

    // A value of the wrong kind makes the file invalid.
    let text = std::fs::read_to_string(&path).expect("config");
    std::fs::write(&path, text.replace("\"cloud\"", "3")).expect("write");
    view.follow(&mut client, |view| !view.workspace.config_error.is_empty()).await;
    std::fs::write(&path, text).expect("write");
    view.follow(&mut client, |view| view.workspace.config_error.is_empty()).await;

    let (addr, _new) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    let view = View { workspace, ..View::default() };
    assert_eq!(identity(&view, "builder"), (automatic.0, seed, Some("cloud".into())));
}

fn mood(view: &View, id: &str) -> CharacterMood {
    view.agent(id).character_mood()
}

#[tokio::test]
async fn the_mood_follows_what_the_agent_does_and_it_falls_asleep_when_idle() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    assert_eq!(mood(&view, "builder"), CharacterMood::Idle);
    let born = view.agent("builder").character_mood_since_ms;
    // A new connection shows the same mood since the same time.
    let (_other, workspace, _) = Client::connect(addr).await;
    assert_eq!(workspace.agents.iter().find(|agent| agent.id == "builder").expect("builder").character_mood_since_ms, born);

    // Working on a reply: thinking. Stopped: idle, not failed.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Thinking).await;
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;

    // A run that ends while the person looks elsewhere: news, until they look.
    let select = |id: &str| client_message::Body::SelectAgent(v1::SelectAgent { agent_id: id.into() });
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::News).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;

    // Idle long enough, it falls asleep, dated from when it did; selecting it
    // doesn't wake it.
    let idle_since = view.agent("builder").character_mood_since_ms;
    factory.set_sleep_after(Duration::from_millis(400));
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Asleep).await;
    let asleep_since = view.agent("builder").character_mood_since_ms;
    // It counts from when its run ended, a moment before it was opened (and idle).
    assert!((idle_since - 1_000..=idle_since + 400).contains(&asleep_since), "{idle_since} {asleep_since}");
    apply_request(&mut client, &mut view, select("rivet")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    assert_eq!(mood(&view, "builder"), CharacterMood::Asleep);

    // After a restart it's still asleep, since the same time.
    let (addr, factory) = start(&dir).await;
    factory.set_sleep_after(Duration::from_millis(400));
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!((mood(&view, "builder"), view.agent("builder").character_mood_since_ms), (CharacterMood::Asleep, asleep_since));

    // /clear starts it over, awake.
    let clear = client_message::Body::RunCommand(v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() });
    apply_request(&mut client, &mut view, clear).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Asleep).await;

    // A message wakes it at once.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Thinking).await;
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, stop).await;

    // A model that can't run.
    apply_request(&mut client, &mut view, update("builder", |change| change.model = Some("gone/model".into()))).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::NoModel).await;

    // Deleted, its presence goes with it.
    let delete = client_message::Body::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() });
    assert!(matches!(apply_request(&mut client, &mut view, delete).await, Body::Ack(_)));
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    let rows: i64 = db.query_row("SELECT COUNT(*) FROM agent_presence WHERE agent = 'builder'", [], |row| row.get(0)).expect("count");
    assert_eq!(rows, 0);
}

/// The outcomes of the agent's runs, oldest first.
fn outcomes(dir: &std::path::Path, id: &str) -> Vec<String> {
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("db");
    let mut query = db.prepare("SELECT outcome FROM runs WHERE agent = ?1 AND ended IS NOT NULL ORDER BY id").expect("query");
    query.query_map([id], |row| row.get(0)).expect("rows").map(|outcome| outcome.expect("outcome")).collect()
}

#[tokio::test]
async fn a_running_tool_shows_working_and_a_failure_lasts_until_a_new_run_or_clear() {
    let dir = temp_dir();
    let (addr, _first) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;

    // While a tool runs it works, and it's idle once the run ends.
    apply_request(&mut client, &mut view, send_message("builder", r#"call bash {"command": "sleep 1"}"#)).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Working).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;
    assert_eq!(outcomes(&dir, "builder"), ["finished"]);

    // The model fails for good: the run ends failed, and stays so after a restart.
    apply_request(&mut client, &mut view, send_message("builder", "please fail")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Failed).await;
    assert_eq!(outcomes(&dir, "builder"), ["finished", "failed"]);
    let (addr, _second) = start(&dir).await;
    let (mut client, workspace, transcript) = Client::connect(addr).await;
    let entries = transcript.map(|transcript| transcript.entries).unwrap_or_default();
    let mut view = View { workspace, entries, ..View::default() };
    assert_eq!(mood(&view, "builder"), CharacterMood::Failed);

    // A new run wakes it from the failure; Stop isn't one.
    apply_request(&mut client, &mut view, send_message("builder", "hang")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Thinking).await;
    let stop = client_message::Body::StopAgent(v1::StopAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, stop).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;
    assert_eq!(outcomes(&dir, "builder").last().map(String::as_str), Some("stopped"));

    // A folder it can't work in fails it too; /clear starts it over, for good.
    std::fs::remove_dir_all(dir.join("site")).expect("remove");
    std::fs::write(dir.join("site"), "not a folder").expect("file");
    apply_request(&mut client, &mut view, send_message("builder", "hello")).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Failed).await;
    assert_eq!(outcomes(&dir, "builder").last().map(String::as_str), Some("failed"));
    let clear = client_message::Body::RunCommand(v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() });
    apply_request(&mut client, &mut view, clear).await;
    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Idle).await;
    let (addr, _third) = start(&dir).await;
    let (_client, workspace, _) = Client::connect(addr).await;
    assert_eq!(View { workspace, ..View::default() }.agent("builder").character_mood(), CharacterMood::Idle);
}

/// Onboarding rehearsal 2: Rivet's first turn on a key Anthropic refused
/// showed the raw answer. The failure's notice says whose credentials the
/// provider refused, so the app can offer to change them; any other failure
/// says nothing of the kind.
#[tokio::test]
async fn a_refused_key_says_whose_it_was() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: "builder".into() });
    apply_request(&mut client, &mut view, select).await;
    let notices = |view: &View| -> Vec<v1::Notice> {
        view.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(v1::entry::Kind::Notice(notice)) if notice.level() == v1::NoticeLevel::Error => Some(notice.clone()),
                _ => None,
            })
            .collect()
    };

    apply_request(&mut client, &mut view, send_message("builder", "please refuse my key")).await;
    view.follow(&mut client, |view| !notices(view).is_empty()).await;
    let refused = notices(&view)[0].clone();
    assert!(refused.text.contains("invalid x-api-key"), "the provider's words stay: {}", refused.text);
    let whose = refused.refused.expect("refused");
    assert_eq!((whose.provider_id.as_str(), whose.account_id.as_str(), whose.method_id.as_str()), ("scripted", "1", "api_key"));

    view.follow(&mut client, |view| mood(view, "builder") == CharacterMood::Failed).await;
    apply_request(&mut client, &mut view, send_message("builder", "please fail")).await;
    view.follow(&mut client, |view| notices(view).len() == 2).await;
    assert_eq!(notices(&view)[1].refused, None);
}
