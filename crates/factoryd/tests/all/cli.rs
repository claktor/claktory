//! The `factoryd` commands for setting Claktory up from a shell (onboarding
//! slice 3): each talks to the running backend and answers with one JSON
//! line and an exit code per outcome.

use std::path::Path;
use std::time::Duration;

use crate::common::{Scripted, home, start, start_with, temp_dir};
use factoryd::cli::{Answer, Exit, Flags, run};
use factoryd::home::Instance;
use serde_json::Value;

async fn cli(dir: &Path, addr: std::net::SocketAddr, line: &str, stdin: &str) -> Answer {
    let args: Vec<String> = shell_words(line);
    let flags = Flags::parse(&args).unwrap_or_else(|| panic!("{line} isn't one of the commands"));
    let instance = Instance { home: home(dir), addr };
    let stdin = stdin.to_owned();
    run(&instance, &flags, move || stdin).await
}

/// Words split on spaces, "quoted" ones kept whole.
fn shell_words(line: &str) -> Vec<String> {
    line.split('"')
        .enumerate()
        .flat_map(|(at, part)| if at % 2 == 1 { vec![part.to_owned()] } else { part.split_whitespace().map(str::to_owned).collect() })
        .collect()
}

/// An answer's keys, sorted: the shape agents rely on stays as it is.
fn keys(value: &Value) -> Vec<&str> {
    let mut keys: Vec<&str> = value.as_object().expect("an object").keys().map(String::as_str).collect();
    keys.sort_unstable();
    keys
}

fn ok(answer: &Answer) -> &Value {
    assert_eq!((answer.exit, &answer.json["ok"]), (Exit::Ok, &Value::Bool(true)), "{answer:?}");
    &answer.json
}

#[tokio::test]
async fn status_and_settings_answer_in_json() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let status = cli(&dir, addr, "status --json", "").await;
    let json = ok(&status);
    assert_eq!(json["account"]["state"], "signed_out");
    assert_eq!(json["providers"][0]["signed_in"], true);
    assert!(json["rivet"]["model"].as_str().is_some_and(|model| !model.is_empty()), "{json}");
    assert_eq!(json["devices"], Value::Array(Vec::new()));
    assert!(json["version"].as_str().is_some_and(|version| !version.is_empty()), "{json}");

    let set = cli(&dir, addr, r#"settings set user.name "Matt Smith" --json"#, "").await;
    assert_eq!(ok(&set)["value"], "Matt Smith");
    let status = cli(&dir, addr, "status --json", "").await;
    assert_eq!(ok(&status)["person"], serde_json::json!({"name": "Matt Smith", "initials": "MS"}));
    let unknown = cli(&dir, addr, "settings set theme dark --json", "").await;
    assert_eq!((unknown.exit, unknown.json["error"].as_str()), (Exit::Refused, Some("unknown_setting")));
    let bad = cli(&dir, addr, "settings set color plaid --json", "").await;
    assert_eq!(bad.exit, Exit::Refused, "{bad:?}");
    let usage = cli(&dir, addr, "account frobnicate --json", "").await;
    assert_eq!((usage.exit, usage.json["error"].as_str()), (Exit::Refused, Some("usage")));
}

#[tokio::test]
async fn an_api_key_comes_on_stdin_and_a_wrong_one_is_refused() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let argv = cli(&dir, addr, "provider key scripted --json", "").await;
    assert_eq!((argv.exit, argv.json["error"].as_str()), (Exit::Refused, Some("usage")), "only with --stdin");
    // Signing in to a model provider is the person's: an agent's command
    // is refused, and nothing is signed in.
    let added = cli(&dir, addr, "provider key scripted --stdin --json", "sk-good\n").await;
    assert_eq!((added.exit, added.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{added:?}");
    let status = cli(&dir, addr, "status --json", "").await;
    assert_eq!(ok(&status)["providers"][0]["signed_in"], false);
    // The key is nowhere in what was printed.
    assert!(!added.json.to_string().contains("sk-good") && !added.text.contains("sk-good"));
}

/// The person makes a pairing code in the app, as the person: the code and
/// the new device's id.
async fn person_pairs(addr: std::net::SocketAddr, name: &str) -> (String, String) {
    use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
    let (mut person, _, _) = crate::common::Client::connect(addr).await;
    let made = person.request(Request::CreatePairing(v1::CreatePairing { name: name.into(), host: String::new() })).await;
    match made.body {
        Body::PairingCode(made) => (made.code, made.device_id),
        other => panic!("a code, got {other:?}"),
    }
}

#[tokio::test]
async fn devices_are_listed_and_only_the_person_pairs_allows_or_removes_them() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    // Seeing a code is the person's: pair asks their devices, and
    // the command never holds the code.
    let pair = cli(&dir, addr, r#"pair "Matt's iPhone" --json"#, "").await;
    assert_eq!(ok(&pair)["device_id"], "matt-s-iphone", "{pair:?}");
    assert!(!pair.json.to_string().contains("factory://") && !pair.text.contains("factory://"), "{pair:?}");
    let devices = cli(&dir, addr, "devices --json", "").await;
    assert_eq!(ok(&devices)["devices"][0]["state"], "code_unused");
    assert_eq!(keys(&devices.json["devices"][0]), ["connected", "expires_in", "id", "last_seen_ms", "name", "state"]);
    // A code nobody used yet isn't a paired device (Abbacchio, a fresh Linux install).
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 0 devices."), "{}", status.text);
    // Allowing is the person's, on their devices: the command says where.
    let allow = cli(&dir, addr, r#"allow "matt's iphone" --json"#, "").await;
    assert_eq!((allow.exit, allow.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{allow:?}");
    assert!(allow.text.contains("Settings → Devices"), "{allow:?}");
    let nobody = cli(&dir, addr, "allow d-404 --json", "").await;
    assert_eq!((nobody.exit, nobody.json["error"].as_str()), (Exit::Refused, Some("not_found")));
    let gone = cli(&dir, addr, "unpair matt-s-iphone --json", "").await;
    assert_eq!((gone.exit, gone.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{gone:?}");
    assert_eq!(ok(&cli(&dir, addr, "devices --json", "").await)["devices"][0]["id"], "matt-s-iphone", "still there");
}

#[tokio::test]
async fn without_a_running_claktory_it_says_so() {
    let dir = temp_dir();
    // A port nothing listens on.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let answer = cli(&dir, addr, "status --json", "").await;
    assert_eq!((answer.exit, answer.json["error"].as_str()), (Exit::Unreachable, Some("unreachable")));
    assert_eq!(answer.exit as u8, 4);
}

/// #194: another user's Claktory on this computer turns this user's key
/// away, and the command says whose it is rather than only "unauthorized".
#[tokio::test]
async fn another_users_claktory_says_so() {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let other = temp_dir();
    let pairing = home(&other).root().join("remote");
    std::fs::create_dir_all(&pairing).expect("remote folder");
    std::fs::write(pairing.join("local-pairing"), "factory://pair?e=x&t=someone-elses-key\n").expect("local-pairing");
    let answer = cli(&other, addr, "status --json", "").await;
    assert_eq!((answer.exit, answer.json["error"].as_str()), (Exit::Refused, Some("not_yours")), "{answer:?}");
    assert!(answer.text.contains("belongs to another user on this computer"), "{}", answer.text);
}

/// Arjun's #191 test: a server has no screen, and its first device stayed
/// "First device". A code made without a name takes the device's own;
/// agents' commands list it waiting, never with its digits, and can't
/// allow it: only the person can, with the digits it shows.
#[tokio::test]
async fn a_device_paired_without_a_name_gives_its_own_and_only_the_person_allows_it() {
    use crate::common::{Client, View, apply_request};
    use crate::remote::{dial, hello_as, new_device, next, number, open, parse};
    use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (code, id) = person_pairs(addr, "").await;
    assert_eq!(ok(&cli(&dir, addr, "devices --json", "").await)["devices"][0]["name"], "New device");

    let code = parse(&code);
    let device = new_device().await;
    let connection = dial(&device, &code, &code.addresses).await.expect("a code is waiting: let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello_as(&mut socket, &code.token, "  Matt's   MacBook ").await, Body::PairingPending(_)));
    let shows = number(&code, &device.id().to_string());
    let devices = cli(&dir, addr, "devices --json", "").await;
    let listed = &ok(&devices)["devices"][0];
    assert_eq!((listed["name"].as_str(), listed["state"].as_str()), (Some("Matt's MacBook"), Some("waiting")), "{listed}");
    // Never its number (#352 P4): the person reads it off the device.
    assert!(listed.get("number").is_none(), "{listed}");
    // The key it asks with, for a setup on the device to compare with its own (#245).
    assert_eq!(listed["endpoint_id"].as_str(), Some(device.id().to_string().as_str()));
    assert_eq!(keys(listed), ["connected", "endpoint_id", "expires_in", "id", "last_seen_ms", "name", "state"]);
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 0 devices, 1 waiting to be allowed."), "{}", status.text);

    // An agent's command can't allow it, with the right digits or without,
    // and says no digits.
    let bare = cli(&dir, addr, &format!("allow {id}"), "").await;
    assert_eq!((bare.exit, bare.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{bare:?}");
    assert!(!bare.text.contains(&shows) && !bare.json.to_string().contains(&format!("\"{shows}\"")));
    let listed = cli(&dir, addr, "devices --no-wait", "").await;
    assert_eq!(listed.text, format!("{id}  {}", factoryd::cli::waiting_sentence(&id, "Matt's MacBook")));
    assert!(!listed.text.contains(&shows) && !listed.json.to_string().contains(&format!("\"{shows}\"")));
    let allowed = cli(&dir, addr, &format!(r#"allow "Matt's MacBook" --number {shows} --json"#), "").await;
    assert_eq!((allowed.exit, allowed.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{allowed:?}");
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 0 devices, 1 waiting to be allowed."), "{}", status.text);

    // The person allows it, with the digits it shows.
    let (mut person, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let allow = v1::ApproveDevice { device_id: id.clone(), number: shows, ..Default::default() };
    assert!(matches!(apply_request(&mut person, &mut view, Request::ApproveDevice(allow)).await, Body::Ack(_)));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 1 device."), "{}", status.text);

    // Nor the next one: a device that's in allows new ones.
    let (code, second_id) = person_pairs(addr, "").await;
    let code = parse(&code);
    let phone = new_device().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello_as(&mut socket, &code.token, "iPad").await, Body::PairingPending(_)));
    let shows = number(&code, &phone.id().to_string());
    for line in [format!("allow {second_id} --number {shows} --json"), format!("allow {second_id} --json")] {
        let refused = cli(&dir, addr, &line, "").await;
        assert_eq!((refused.exit, refused.json["error"].as_str()), (Exit::Refused, Some("person_only")), "{line}: {refused:?}");
    }
}

/// `factoryd` itself, against the Claktory in `dir`.
fn factoryd(dir: &Path, addr: std::net::SocketAddr, args: &[&str]) -> std::process::Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .args(args)
        .env("HOME", dir)
        .env("FACTORY_HOME", home(dir).root())
        .env("FACTORY_PORT", addr.port().to_string())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("factoryd")
}

/// `factoryd pair` asks the person's devices and prints no code:
/// the device waits for its code as Add a device makes it, and its card is
/// in the admin agent's conversation, marked as the command line's, where
/// the person's app gets the code. Without a name the device names itself;
/// the same name again is the same device, and only 3 wait at once.
#[tokio::test]
async fn factoryd_pair_asks_the_persons_devices_and_prints_no_code() {
    use crate::common::{Client, View, apply_request};
    use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let output = tokio::task::spawn_blocking({
        let dir = dir.clone();
        move || factoryd(&dir, addr, &["pair"])
    })
    .await
    .expect("ran");
    let printed = format!("{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
    assert!(output.status.success(), "{printed}");
    assert!(!printed.contains("factory://") && printed.contains("Needs you"), "{printed}");
    let devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    let made = devices.iter().find(|device| !device.local).expect("a device");
    assert_eq!((made.name.as_str(), made.unnamed), ("New device", true));
    let again = cli(&dir, addr, "pair --json", "").await;
    assert_eq!(ok(&again)["device_id"].as_str(), Some(made.id.as_str()), "the same one");

    let (mut person, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut person, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    let card = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::DevicePairing(pairing)) => Some(pairing.clone()),
            _ => None,
        })
        .expect("its card in Rivet's conversation");
    assert!(card.from_command_line && card.agent_id == "rivet" && card.device_id == made.id, "{card:?}");
    let shown = person.request(Request::ShowPairingCode(v1::ShowPairingCode { pairing_id: card.id.clone() })).await;
    assert!(matches!(&shown.body, Body::PairingCode(code) if code.code.starts_with("factory://")), "{:?}", shown.body);

    for name in ["Phone", "Tablet"] {
        ok(&cli(&dir, addr, &format!("pair {name} --json"), "").await);
    }
    let more = cli(&dir, addr, "pair Laptop --json", "").await;
    assert_eq!(more.exit, Exit::Refused, "{more:?}");
    let waiting = factoryd::remote::devices(&home(&dir)).expect("devices").into_iter().filter(|device| !device.local).count();
    assert_eq!(waiting, 3);
}

/// A pairing card that's the first entry of Rivet's conversation is at seq
/// 0: the Workspace says it waits, and every entry of its waiting_versions
/// carries its key on the wire, since an app reading a keyless one drops
/// the whole Workspace.
#[tokio::test]
async fn a_card_at_seq_0_reaches_the_app_with_no_keyless_version() {
    use crate::common::{Client, View};
    use prost::Message;
    #[derive(Clone, PartialEq, prost::Message)]
    struct WireEntry {
        #[prost(uint64, optional, tag = "1")]
        key: Option<u64>,
        #[prost(uint64, optional, tag = "2")]
        value: Option<u64>,
    }
    #[derive(Clone, PartialEq, prost::Message)]
    struct WireAgent {
        #[prost(message, repeated, tag = "55")]
        waiting_versions: Vec<WireEntry>,
    }

    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    ok(&cli(&dir, addr, "pair Phone --no-wait --json", "").await);
    let (mut person, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut person, |view| !view.agent("rivet").waiting_seqs.is_empty()).await;
    let rivet = view.agent("rivet");
    assert_eq!(rivet.waiting_seqs, [0], "the card is the conversation's first entry");
    let wire = WireAgent::decode(rivet.encode_to_vec().as_slice()).expect("an Agent");
    assert!(wire.waiting_versions.iter().all(|entry| entry.key.is_some() && entry.value.is_some()), "{wire:?}");
}

/// With no admin agent to show the card, `factoryd pair` asks nothing: it
/// says to add the device in Settings → Devices, and no device waits.
#[tokio::test]
async fn factoryd_pair_with_no_admin_agent_makes_nothing() {
    use crate::common::{Client, View, apply_request};
    use factory_protocol::v1::{self, client_message::Body as Request};
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let (mut person, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let off = v1::ExtensionChange { id: "agents".into(), enabled: Some(false), ..Default::default() };
    let update = v1::UpdateAgent { agent_id: "rivet".into(), extension: Some(off), ..Default::default() };
    apply_request(&mut person, &mut view, Request::UpdateAgent(update)).await;
    let pair = cli(&dir, addr, "pair Phone --json", "").await;
    assert_eq!(pair.exit, Exit::Refused, "{pair:?}");
    assert!(pair.text.contains("Settings → Devices → Add a device"), "{pair:?}");
    assert!(factoryd::remote::devices(&home(&dir)).unwrap_or_default().iter().all(|device| device.local));
}

/// `factoryd pair --show-code` prints the first device's code only on a
/// new install: never once a device is allowed, the Claktory has been
/// signed in or an agent has run.
#[test]
fn the_installers_code_is_only_for_a_new_install() {
    use factoryd::remote::Device;
    let dir = temp_dir();
    let home = home(&dir);
    std::fs::create_dir_all(home.root()).expect("home");
    assert_eq!(factoryd::remote::first_code_refused(&home), None, "a new install");

    let account = home.root().join("account");
    std::fs::create_dir_all(&account).expect("account");
    std::fs::write(account.join("account.json"), r#"{"step":"asked"}"#).expect("account.json");
    assert_eq!(factoryd::remote::first_code_refused(&home), None, "asked to sign in, never signed in");
    std::fs::write(account.join("account.json"), r#"{"signed_in_ms":5}"#).expect("account.json");
    assert!(factoryd::remote::first_code_refused(&home).is_some(), "signed in once");
    std::fs::remove_file(account.join("account.json")).expect("rm");

    let db = rusqlite::Connection::open(home.db_path()).expect("db");
    db.execute_batch("CREATE TABLE runs (id INTEGER);").expect("runs");
    assert_eq!(factoryd::remote::first_code_refused(&home), None, "no agent ran");
    db.execute("INSERT INTO runs VALUES (1)", []).expect("a run");
    assert!(factoryd::remote::first_code_refused(&home).is_some(), "an agent ran");
    db.execute("DELETE FROM runs", []).expect("none");

    let allowed = Device { id: "phone".to_owned(), name: "Phone".to_owned(), endpoint_id: "ab".repeat(32), ..Device::default() };
    std::fs::create_dir_all(home.root().join("remote")).expect("remote");
    std::fs::write(home.root().join("remote/devices.json"), serde_json::to_vec(&[allowed]).expect("json")).expect("devices");
    assert!(factoryd::remote::first_code_refused(&home).is_some(), "a device is in");
}

/// `factoryd devices` in a terminal, from the devices file: a waiting
/// device's number is never printed, and a name the device chose reaches
/// the terminal without escape sequences, controls or bidi marks (Abbacchio's
/// rows 705-706, Fugo: OSC 52 could write the clipboard).
#[test]
fn factoryd_devices_prints_no_number_and_no_escapes() {
    use factoryd::remote::Device;
    let dir = temp_dir();
    let home = home(&dir);
    let later = jiff::Timestamp::now().as_millisecond() + 600_000;
    let name = "\x1b]52;c;aGk=\x07Evil\x1b[2K\u{202e}Mac\u{200b}\u{9b}\u{feff} Laptop of someone with a very long name 1234567890";
    let waiting = Device {
        id: "first".to_owned(),
        name: name.to_owned(),
        token_sha256: "00".repeat(32),
        code_expires: Some(later),
        pending_endpoint_id: "e-first".to_owned(),
        pending_since: Some(later - 1000),
        ..Device::default()
    };
    std::fs::create_dir_all(home.root().join("remote")).expect("remote");
    std::fs::write(home.root().join("remote/devices.json"), serde_json::to_vec(&[waiting]).expect("json")).expect("devices");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .arg("devices")
        .env("FACTORY_HOME", home.root())
        .stdin(std::process::Stdio::null())
        .output()
        .expect("factoryd devices");
    let printed = String::from_utf8(output.stdout).expect("utf-8");
    assert!(output.status.success(), "{printed}");
    assert!(!printed.chars().any(|char| char.is_control() && char != '\n' || matches!(char, '\u{200b}' | '\u{202e}' | '\u{feff}')), "{printed:?}");
    let shown = factoryd::remote::shown_name(name);
    assert_eq!(
        printed,
        format!("first  {}\n", factoryd::cli::waiting_sentence("first", &shown))
    );
    let claktory = factoryd::remote::secret_key(&home).expect("key").public().to_string();
    let number = factoryd::remote::approval_number(&claktory, "e-first", &"00".repeat(32));
    assert!(!printed.contains(&number), "{printed}");
}

/// `factoryd allow` without a running Claktory allows nothing: only the
/// running one checks the number typed (#352 P4), so nothing allows a
/// device from its files alone.
#[test]
fn factoryd_allow_without_a_running_claktory_allows_nothing() {
    use factoryd::remote::Device;
    let dir = temp_dir();
    let home = home(&dir);
    let later = jiff::Timestamp::now().as_millisecond() + 600_000;
    let waiting = Device {
        id: "first".to_owned(),
        name: "first".to_owned(),
        token_sha256: "00".repeat(32),
        code_expires: Some(later),
        pending_endpoint_id: "e-first".to_owned(),
        pending_since: Some(later - 1000),
        ..Device::default()
    };
    std::fs::create_dir_all(home.root().join("remote")).expect("remote");
    std::fs::write(home.root().join("remote/devices.json"), serde_json::to_vec(&[waiting]).expect("json")).expect("devices");
    for args in [&["allow", "first"][..], &["allow", "first", "--number", "1234"][..]] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_factoryd"))
            .args(args)
            .env("FACTORY_HOME", home.root())
            .stdin(std::process::Stdio::null())
            .output()
            .expect("factoryd allow");
        assert!(!output.status.success(), "{args:?}: {}", String::from_utf8_lossy(&output.stdout));
        let first = factoryd::remote::devices(&home).expect("devices").into_iter().find(|device| device.id == "first").expect("first");
        assert_eq!((first.endpoint_id.as_str(), first.pending_endpoint_id.as_str()), ("", "e-first"), "{args:?}");
    }
}
