//! Keys the vault generates: an agent asks, the person approves, and
//! the private half is made and sealed in the vault, never shown to anyone;
//! the agent hears only the public half. And notes on a decision, each as
//! its writer's.

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{call, idle};
use factory_protocol::v1::{self, client_message, entry::Kind};

async fn setup() -> (std::path::PathBuf, Client, View) {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    (dir, client, view)
}

fn pending(view: &View) -> v1::SecretRequest {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.state() == v1::SecretRequestState::Pending => Some(request.clone()),
            _ => None,
        })
        .expect("a request")
}

fn decided(view: &View, id: &str) -> v1::SecretRequest {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.id == id => Some(request.clone()),
            _ => None,
        })
        .expect("the request")
}

fn event_text(view: &View, containing: &str) -> Option<String> {
    view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.contains(containing) => Some(event.text.clone()),
        _ => None,
    })
}

fn secret(view: &View, name: &str) -> v1::SecretInfo {
    view.workspace.vault.clone().expect("vault").secrets.into_iter().find(|secret| secret.name == name).expect("the secret")
}

/// A line of a key's body: long, and only base64.
fn body_line(line: &str) -> bool {
    line.len() >= 40 && line.chars().all(|c| c.is_ascii_alphanumeric() || "+/=".contains(c))
}

fn decide(id: &str, ask_each_time: Option<bool>, comment: &str) -> client_message::Body {
    client_message::Body::DecideSecretRequest(v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: id.into(),
        approve: true,
        comment: comment.into(),
        ask_each_time,
        ..Default::default()
    })
}

#[tokio::test]
async fn the_vault_generates_a_key_and_the_agent_sees_only_its_public_half() {
    let (_dir, mut client, mut view) = setup().await;
    let ask = r#"{"name":"Deploy key","description":"Pushes to GitHub.","generate":"ssh-ed25519","reason":"To deploy the site."}"#;
    let asked = call(&mut client, &mut view, "bo", "secrets_request", ask).await;
    assert!(asked.starts_with("Asked the vault to generate Deploy key, an ssh-ed25519 key;"), "{asked}");
    let request = pending(&view);
    assert_eq!((request.generate.as_str(), request.grant_to_nobody), ("ssh-ed25519", false));
    let variable = &request.new_secret.clone().expect("wanted").variables[0];
    assert_eq!((variable.name.as_str(), variable.file), ("SSH_KEY", true));

    // Only the person approves it: there's nothing for an admin agent to find.
    let to_admin = v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: request.id.clone(),
        approve: true,
        approve_only: true,
        ..Default::default()
    };
    let refused = apply_request(&mut client, &mut view, client_message::Body::DecideSecretRequest(to_admin)).await;
    assert!(matches!(refused, v1::server_message::Body::Error(_)));

    // Approved without asking each time, so a command can use it at once.
    apply_request(&mut client, &mut view, decide(&request.id, Some(false), "Add it to the repo's deploy keys.")).await;
    view.follow(&mut client, |view| event_text(view, "the vault generated Deploy key").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault generated Deploy key").expect("told");
    let public = decided(&view, &request.id).public_key;
    assert!(public.starts_with("ssh-ed25519 AAAA") && public.ends_with(" Deploy key"), "{public}");
    assert_eq!(decided(&view, &request.id).state(), v1::SecretRequestState::Granted);
    assert!(told.contains(&format!("Its public key:\n{public}")), "{told}");
    assert!(told.contains("SSH_KEY is its private key's file"), "{told}");
    assert!(told.contains("\n\nThe person's note: Add it to the repo's deploy keys.\n\n"), "{told}");
    assert!(!told.contains("PRIVATE KEY") && !told.lines().any(body_line), "{told}");
    let kept = secret(&view, "Deploy key");
    assert!(kept.description.contains(&public) && !kept.ask_each_time, "{kept:?}");
    // Settings → Secrets tells it by its fingerprint's end, not its armor's.
    let fingerprint = ssh_key::PublicKey::from_openssh(&public).expect("public").fingerprint(ssh_key::HashAlg::Sha256).to_string();
    assert_eq!(kept.variables[0].hint, fingerprint[fingerprint.len() - 4..], "{kept:?}");
    let listed = call(&mut client, &mut view, "bo", "secrets_requests", "{}").await;
    assert!(listed.contains(&format!("Public key: {public}")) && !listed.contains("PRIVATE KEY"), "{listed}");

    // The private half is hidden from every output, from the start.
    let cat = call(&mut client, &mut view, "bo", "bash", r#"{"command":"cat \"$SSH_KEY\"","secrets":["Deploy key"]}"#).await;
    assert!(cat.contains("[secret Deploy key/SSH_KEY]"), "{cat}");
    assert!(!cat.lines().any(body_line), "{cat}");
    // It's the key the agent was told of.
    if std::process::Command::new("ssh-keygen").arg("-?").output().is_ok() {
        let derived =
            call(&mut client, &mut view, "bo", "bash", r#"{"command":"ssh-keygen -y -f \"$SSH_KEY\"","secrets":["Deploy key"]}"#).await;
        let key = public.split(' ').take(2).collect::<Vec<_>>().join(" ");
        assert!(derived.contains(&key), "{derived}");
    }
}

#[tokio::test]
async fn a_key_for_no_one_stays_with_no_one() {
    let (_dir, mut client, mut view) = setup().await;
    let ask = r#"{"name":"Release key","generate":"ssh-ed25519","grant":"nobody","reason":"For the release machine."}"#;
    call(&mut client, &mut view, "bo", "secrets_request", ask).await;
    let request = pending(&view);
    assert!(request.grant_to_nobody);
    apply_request(&mut client, &mut view, decide(&request.id, None, "")).await;
    view.follow(&mut client, |view| event_text(view, "the vault generated Release key").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault generated Release key").expect("told");
    assert!(told.contains("gave it to no one, as you asked") && told.contains("Its public key:\nssh-ed25519 AAAA"), "{told}");
    let generated = decided(&view, &request.id);
    assert_eq!(generated.state(), v1::SecretRequestState::Generated);
    let kept = secret(&view, "Release key");
    assert!(kept.grants.is_empty(), "{kept:?}");
    assert!(!kept.ask_each_time, "an app that doesn't say gets the request's default: not asking each time");
    let used = call(&mut client, &mut view, "bo", "bash", r#"{"command":"echo hi","secrets":["Release key"]}"#).await;
    assert!(!used.contains("hi\n") && !used.starts_with("hi"), "{used}");
}

#[tokio::test]
async fn a_generated_key_asks_each_time_when_the_person_turns_it_on() {
    let (_dir, mut client, mut view) = setup().await;
    let ask = r#"{"name":"Deploy key","generate":"ssh-ed25519","reason":"To deploy the site."}"#;
    call(&mut client, &mut view, "bo", "secrets_request", ask).await;
    let request = pending(&view);
    apply_request(&mut client, &mut view, decide(&request.id, Some(true), "")).await;
    view.follow(&mut client, |view| event_text(view, "the vault generated Deploy key").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault generated Deploy key").expect("told");
    assert!(told.contains("It asks the person each time it's used"), "{told}");
    assert!(secret(&view, "Deploy key").ask_each_time);
}

#[tokio::test]
async fn a_generate_request_names_one_key_type_and_one_variable() {
    let (_dir, mut client, mut view) = setup().await;
    let rsa = call(&mut client, &mut view, "bo", "secrets_request", r#"{"name":"K","generate":"rsa","reason":"x"}"#).await;
    assert!(rsa.contains("ssh-ed25519"), "{rsa}");
    let two = r#"{"name":"K","generate":"ssh-ed25519","variables":[{"name":"A"},{"name":"B"}],"reason":"x"}"#;
    let refused = call(&mut client, &mut view, "bo", "secrets_request", two).await;
    assert!(refused.contains("one variable"), "{refused}");
    let grant =
        call(&mut client, &mut view, "bo", "secrets_request", r#"{"name":"K","generate":"ssh-ed25519","grant":"all","reason":"x"}"#).await;
    assert!(grant.contains("\"me\" or \"nobody\""), "{grant}");
}

/// Every file under `dir` holding `value` in the clear, but `except`.
fn files_holding(dir: &std::path::Path, value: &str, except: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![dir.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                folders.push(path);
            } else if path != except
                && std::fs::read(&path).is_ok_and(|bytes| bytes.windows(value.len()).any(|window| window == value.as_bytes()))
            {
                found.push(path);
            }
        }
    }
    found
}

#[tokio::test]
async fn the_vault_generates_a_password_that_no_one_sees() {
    let (dir, mut client, mut view) = setup().await;
    // Weaker than the bounds: refused when asked, with the reason.
    let short = r#"{"name":"Pin","generate":"password","password":{"digits":true,"max_length":12},"reason":"x"}"#;
    let refused = call(&mut client, &mut view, "bo", "secrets_request", short).await;
    assert!(refused.contains("weaker than the vault allows"), "{refused}");
    let eleven = r#"{"name":"Pin","generate":"password","password":{"length":11},"reason":"x"}"#;
    assert!(call(&mut client, &mut view, "bo", "secrets_request", eleven).await.contains("out of bounds"));
    let not_a_key = r#"{"name":"K","generate":"ssh-ed25519","password":{"length":20},"reason":"x"}"#;
    assert!(call(&mut client, &mut view, "bo", "secrets_request", not_a_key).await.contains("goes with generate"));

    // Letters and digits, 32, no look-alikes.
    let ask = r#"{"name":"Review page","description":"Basic auth for the review page.","generate":"password","password":{"length":32,"lowercase":true,"uppercase":true,"digits":true,"avoid_similar":true},"reason":"For App Review."}"#;
    let asked = call(&mut client, &mut view, "bo", "secrets_request", ask).await;
    assert!(asked.starts_with("Asked the vault to generate Review page, a 32-character password (letters and digits, 185 bits);"), "{asked}");
    let request = pending(&view);
    let spec = request.password.clone().expect("its spec");
    assert_eq!((spec.length, spec.alphabet_size, spec.shortest, spec.longest), (32, 56, 16, 256));
    assert!(spec.lowercase && spec.uppercase && spec.digits && !spec.symbols && spec.avoid_similar);
    let variable = &request.new_secret.clone().expect("wanted").variables[0];
    assert_eq!((variable.name.as_str(), variable.file), ("PASSWORD", false));

    // The person can't make it shorter than the bounds; longer, yes.
    let too_short = v1::DecideSecretRequest { password_length: 15, ..decide_body(&request.id) };
    let refused = apply_request(&mut client, &mut view, client_message::Body::DecideSecretRequest(too_short)).await;
    assert!(matches!(refused, v1::server_message::Body::Error(_)));
    let longer = v1::DecideSecretRequest { password_length: 40, ..decide_body(&request.id) };
    apply_request(&mut client, &mut view, client_message::Body::DecideSecretRequest(longer)).await;
    view.follow(&mut client, |view| event_text(view, "the vault generated Review page").is_some() && idle(view, "bo")).await;
    let told = event_text(&view, "the vault generated Review page").expect("told");
    assert!(told.contains("a 40-character password (letters and digits, 232 bits)"), "{told}");
    assert!(told.contains("PASSWORD holds the password; you never see the password itself."), "{told}");
    let generated = decided(&view, &request.id);
    assert_eq!(generated.state(), v1::SecretRequestState::Granted);
    assert_eq!(generated.password.clone().expect("spec").length, 40);
    assert!(generated.public_key.is_empty());

    // Hidden from every output; the agent uses it without seeing it.
    let echo = call(&mut client, &mut view, "bo", "bash", r#"{"command":"echo \"$PASSWORD\"","secrets":["Review page"]}"#).await;
    assert!(echo.contains("[secret Review page/PASSWORD]"), "{echo}");
    let out = dir.join("site").join("made.txt");
    call(&mut client, &mut view, "bo", "bash", r#"{"command":"printf %s \"$PASSWORD\" > made.txt","secrets":["Review page"]}"#).await;
    let password = std::fs::read_to_string(&out).expect("written");
    assert_eq!(password.len(), 40);
    assert!(password.bytes().all(|c| c.is_ascii_alphanumeric() && !b"0Oo1lI".contains(&c)), "{password}");
    // Nowhere in the clear: not the tool results, events or requests, not
    // the list, not any file Claktory keeps (its database and logs).
    let listed = call(&mut client, &mut view, "bo", "secrets_requests", "{}").await;
    assert!(listed.contains("a 40-character password") && !listed.contains(&password), "{listed}");
    assert!(!format!("{:?}", view.entries).contains(&password));
    assert!(!format!("{:?}", view.workspace).contains(&password));
    assert_eq!(files_holding(&dir, &password, &out), Vec::<std::path::PathBuf>::new());
}

fn decide_body(id: &str) -> v1::DecideSecretRequest {
    v1::DecideSecretRequest { agent_id: "bo".into(), request_id: id.into(), approve: true, ask_each_time: Some(false), ..Default::default() }
}

/// In a Claktory with Rivet, every request names the
/// admin agent an approval would pass it to; the person generating it on
/// the card is still History's GENERATED, given to no one when asked so,
/// never ADMIN.
#[tokio::test]
async fn history_says_generated_with_rivet_there() {
    let (_dir, mut client, mut view) = setup().await;
    for ask in [
        r#"{"name":"QA DB password","generate":"password","grant":"nobody","reason":"x"}"#,
        r#"{"name":"QA API password","generate":"password","reason":"x"}"#,
    ] {
        call(&mut client, &mut view, "bo", "secrets_request", ask).await;
        let request = pending(&view);
        assert_eq!(request.admin_agent_id, "rivet", "Rivet is there, with Secrets");
        apply_request(&mut client, &mut view, client_message::Body::DecideSecretRequest(decide_body(&request.id))).await;
    }
    view.follow(&mut client, |view| event_text(view, "the vault generated QA API password").is_some() && idle(view, "bo")).await;

    let history = crate::needs_you::list(&mut client, "password", 0, 0).await;
    let outcome = |title: &str| {
        let record = history.records.iter().find(|record| record.title == title).unwrap_or_else(|| panic!("{title}: {history:?}"));
        (record.secret_outcome(), record.secret_given_name.as_str(), record.subject_agent_id.as_str(), record.secret_to_no_one)
    };
    use v1::NeedsYouSecretOutcome as Secret;
    assert_eq!(outcome("QA DB password"), (Secret::Generated, "QA DB password", "", true));
    assert_eq!(outcome("QA API password"), (Secret::Generated, "QA API password", "", false));
}
