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
    let devices = cli(&dir, addr, "devices --json", "").await;
    assert_eq!(ok(&devices)["devices"][0]["state"], "code_unused");
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
    assert_eq!(listed["number"].as_str(), Some(shows.as_str()));

    // Other digits: it may be someone else's device, so nothing is allowed.
    let other = if shows == "0000" { "0001" } else { "0000" };
    let wrong = cli(&dir, addr, &format!("allow {id} --number {other} --json"), "").await;
    assert_eq!((wrong.exit, wrong.json["error"].as_str()), (Exit::Refused, Some("wrong_number")), "{wrong:?}");
    let allowed = cli(&dir, addr, &format!(r#"allow "Matt's MacBook" --number {shows} --json"#), "").await;
    assert_eq!((ok(&allowed)["allowed"].as_str(), allowed.json["number"].as_str()), (Some(id.as_str()), Some(shows.as_str())));
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
