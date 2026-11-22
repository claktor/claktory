//! Keys the vault generates (#252): an agent asks, the person approves, and
//! the private half is made and sealed in the vault, never shown to anyone;
//! the agent hears only the public half. And notes on a decision, each as
//! its writer's (#253).

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
    assert!(!kept.ask_each_time, "an app that doesn't say gets the card's default: not asking each time");
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
