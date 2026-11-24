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
    let wrong = cli(&dir, addr, "provider key scripted --stdin --json", "sk-wrong\n").await;
    assert_eq!((wrong.exit, wrong.json["error"].as_str()), (Exit::Refused, Some("sign_in_failed")), "{wrong:?}");
    let added = cli(&dir, addr, "provider key scripted --stdin --json", "sk-good\n").await;
    assert_eq!(ok(&added)["provider"], "scripted");
    let status = cli(&dir, addr, "status --json", "").await;
    assert_eq!(ok(&status)["providers"][0]["signed_in"], true);
    // The key is nowhere in what was printed.
    assert!(!added.json.to_string().contains("sk-good") && !added.text.contains("sk-good"));
}

#[tokio::test]
async fn devices_are_paired_without_waiting_and_allowed_once_they_ask() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let pair = cli(&dir, addr, r#"pair "Matt's iPhone" --no-wait --json"#, "").await;
    let json = ok(&pair).clone();
    assert!(json["code"].as_str().is_some_and(|code| code.starts_with("factory://")), "{json}");
    assert_eq!((json["device_id"].as_str(), json["expires_in"].as_u64()), (Some("matt-s-iphone"), Some(600)), "{json}");
    // The shape agents read (#245): the same state as devices says.
    assert_eq!(keys(&json), ["code", "device_id", "expires_in", "ok", "state"]);
    assert_eq!(json["state"], "code_unused");
    let devices = cli(&dir, addr, "devices --json", "").await;
    assert_eq!(ok(&devices)["devices"][0]["state"], "code_unused");
    assert_eq!(keys(&devices.json["devices"][0]), ["connected", "expires_in", "id", "last_seen_ms", "name", "state"]);
    // A code nobody used yet isn't a paired device (Abbacchio, a fresh Linux install).
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 0 devices."), "{}", status.text);
    // Not yet: the device has to use its code first.
    let early = cli(&dir, addr, r#"allow "matt's iphone" --json"#, "").await;
    assert_eq!((early.exit, early.json["error"].as_str()), (Exit::NeedsHuman, Some("not_waiting")), "{early:?}");
    let nobody = cli(&dir, addr, "allow d-404 --json", "").await;
    assert_eq!((nobody.exit, nobody.json["error"].as_str()), (Exit::Refused, Some("not_found")));
    let gone = cli(&dir, addr, "unpair matt-s-iphone --json", "").await;
    assert_eq!(ok(&gone)["unpaired"], "matt-s-iphone");
    assert_eq!(ok(&cli(&dir, addr, "devices --json", "").await)["devices"], Value::Array(Vec::new()));
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
/// "First device". A code made without a name takes the device's own, and
/// the agent allows it in one step with the digits the person reads.
#[tokio::test]
async fn a_device_paired_without_a_name_gives_its_own_and_is_allowed_by_its_digits() {
    use crate::remote::{dial, hello_as, new_device, next, number, open, parse};
    use factory_protocol::v1::server_message::Body;
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    factory.set_tunnel_online_wait(Duration::from_millis(100));
    let pair = cli(&dir, addr, "pair --no-wait --json", "").await;
    let made = ok(&pair).clone();
    let id = made["device_id"].as_str().expect("an id").to_owned();
    assert_eq!(ok(&cli(&dir, addr, "devices --json", "").await)["devices"][0]["name"], "New device");

    let code = parse(made["code"].as_str().expect("a code"));
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

    // Other digits: it may be someone else's device, so nothing is allowed.
    let other = if shows == "0000" { "0001" } else { "0000" };
    // Without --json too, the flag first or after the device (Arjun's finding).
    let wrong = cli(&dir, addr, &format!("allow --number {other} {id}"), "").await;
    assert_eq!((wrong.exit, wrong.json["error"].as_str()), (Exit::Refused, Some("wrong_number")), "{wrong:?}");
    assert_eq!(wrong.text, "That's not the number Matt's MacBook shows. 2 tries left.");
    // Without the number, nothing is allowed, and the digits aren't said.
    let bare = cli(&dir, addr, &format!("allow {id}"), "").await;
    assert_eq!((bare.exit, bare.json["error"].as_str()), (Exit::Refused, Some("needs_number")), "{bare:?}");
    assert_eq!(
        bare.text,
        format!(
            "A device calling itself \"Matt's MacBook\" is waiting. Allow it only if its screen shows the number you type: factoryd allow {id} --number <the 4 digits on its screen>."
        )
    );
    assert!(!bare.text.contains(&shows) && !bare.json.to_string().contains(&format!("\"{shows}\"")));
    // devices says the same, never the digits (Abbacchio's row 705, Fugo).
    let listed = cli(&dir, addr, "devices --no-wait", "").await;
    assert_eq!(listed.text, format!("{id}  {}", bare.text));
    assert!(!listed.text.contains(&shows) && !listed.json.to_string().contains(&format!("\"{shows}\"")));
    let allowed = cli(&dir, addr, &format!(r#"allow "Matt's MacBook" --number {shows} --json"#), "").await;
    assert_eq!((ok(&allowed)["allowed"].as_str(), allowed.json.get("number")), (Some(id.as_str()), None));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
    let status = cli(&dir, addr, "status", "").await;
    assert!(status.text.ends_with(", 1 device."), "{}", status.text);

    // Fugo's rule (#245): once a device is in, a program lets no other in,
    // with its digits or without; a person at a terminal still can.
    let second = ok(&cli(&dir, addr, "pair --no-wait --json", "").await).clone();
    let second_id = second["device_id"].as_str().expect("an id").to_owned();
    let code = parse(second["code"].as_str().expect("a code"));
    let phone = new_device().await;
    let connection = dial(&phone, &code, &code.addresses).await.expect("let in");
    let mut socket = open(&connection).await;
    assert!(matches!(hello_as(&mut socket, &code.token, "iPad").await, Body::PairingPending(_)));
    let shows = number(&code, &phone.id().to_string());
    for line in [
        format!("allow {second_id} --number {shows} --json"),
        format!("allow {second_id} --json"),
        format!("allow {second_id} --number {shows}"),
    ] {
        let refused = cli(&dir, addr, &line, "").await;
        assert_eq!((refused.exit, refused.json["error"].as_str()), (Exit::Refused, Some("has_devices")), "{line}: {refused:?}");
        assert_eq!(refused.text, factoryd::cli::HAS_DEVICES);
    }
    let waiting = ok(&cli(&dir, addr, "devices --json", "").await).clone();
    assert!(
        waiting["devices"]
            .as_array()
            .expect("devices")
            .iter()
            .any(|device| device["id"] == second_id.as_str() && device["state"] == "waiting")
    );
    let mut flags = Flags::parse(&shell_words(&format!("allow {second_id} --number {shows}"))).expect("ours");
    flags.interactive = true;
    let allowed = run(&Instance { home: home(&dir), addr }, &flags, String::new).await;
    assert_eq!(ok(&allowed)["allowed"].as_str(), Some(second_id.as_str()));
    assert!(matches!(next(&mut socket).await, Some(Body::Welcome(_))));
}

/// Arjun's installer runs plain `factoryd pair` in a terminal: without a
/// name it makes a "New device" code, which the device names (#73).
#[tokio::test]
async fn factoryd_pair_without_a_name_makes_a_code_the_device_names() {
    // With a terminal it would wait there for the device.
    if std::fs::OpenOptions::new().read(true).open("/dev/tty").is_ok() {
        return;
    }
    let dir = temp_dir();
    let (_addr, _factory) = start(&dir).await;
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_factoryd"))
        .arg("pair")
        .env("FACTORY_HOME", dir.join("home"))
        .stdin(std::process::Stdio::null())
        .output()
        .expect("factoryd pair");
    let printed = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "{printed}{}", String::from_utf8_lossy(&output.stderr));
    assert!(printed.contains("factory://?t="), "{printed}");
    let devices = factoryd::remote::devices(&home(&dir)).expect("devices");
    let made = devices.iter().find(|device| !device.local).expect("a device");
    assert_eq!((made.name.as_str(), made.unnamed), ("New device", true));
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
        format!("first  A device calling itself \"{shown}\" is waiting. Allow it only if its screen shows the number you type: factoryd allow first --number <the 4 digits on its screen>.\n")
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
