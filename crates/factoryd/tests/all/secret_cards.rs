//! The request cards (#360): the agent's link and steps (K3), the vault's
//! secret that fits a request (K4), the form's answer marked as added (K2),
//! a generated key put in place (K6), and an import for no one (K7).

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::pipes::{call, idle};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message::Body};

async fn setup() -> (std::path::PathBuf, Client, View) {
    let dir = temp_dir();
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    (dir, client, view)
}

/// The request `id` as Bo's conversation shows it now.
fn shown(view: &View, id: &str) -> v1::SecretRequest {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) if request.id == id => Some(request.clone()),
            _ => None,
        })
        .expect("the request")
}

/// The latest request in Bo's conversation.
fn latest(view: &View) -> v1::SecretRequest {
    view.entries
        .iter()
        .rev()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) => Some(request.clone()),
            _ => None,
        })
        .expect("a request")
}

fn asking(link: &str) -> String {
    serde_json::json!({
        "name": "GitHub token",
        "description": "Push to claktory/factory.",
        "variables": [{"name": "GITHUB_TOKEN"}],
        "reason": "To push.",
        "link": link,
    })
    .to_string()
}

#[tokio::test]
async fn an_agents_link_is_https_to_a_public_site_and_its_steps_one_plain_line() {
    let (_dir, mut client, mut view) = setup().await;
    for (link, why) in [
        ("http://github.com/settings/tokens/new", "only https:// links"),
        ("https://user:pass@github.com/settings/tokens/new", "no user name or password"),
        ("https://github.com:8443/settings/tokens/new", "no port"),
        ("https://127.0.0.1/token", "not an IP address"),
        ("https://[::1]/token", "not an IP address"),
        ("https://localhost/token", "a public site"),
        ("https://intranet/token", "a public site"),
        ("https://printer.local/token", "a public site"),
        ("https://vault.corp.internal/token", "a public site"),
        ("javascript:alert(1)", "only https:// links"),
    ] {
        let refused = call(&mut client, &mut view, "bo", "secrets_request", &asking(link)).await;
        assert!(refused.contains(why), "{link}: {refused}");
    }
    let long = format!("https://github.com/{}", "a".repeat(2000));
    let refused = call(&mut client, &mut view, "bo", "secrets_request", &asking(&long)).await;
    assert!(refused.contains("longer than 2,000"), "{refused}");

    // An international name is kept, and labelled, in punycode; steps are
    // one line, with hidden and reordering characters taken out.
    let ask = serde_json::json!({
        "name": "GitHub token",
        "variables": [{"name": "GITHUB_TOKEN"}],
        "reason": "To push.",
        "link": "https://bücher.example/tokens/new?scopes=repo",
        "steps": format!("Tick repo,\nthen\u{202E}Generate\u{200B} token.{}", " x".repeat(150)),
    });
    let asked = call(&mut client, &mut view, "bo", "secrets_request", &ask.to_string()).await;
    assert!(asked.starts_with("Asked for GitHub token"), "{asked}");
    let request = latest(&view);
    assert_eq!(request.link, "https://xn--bcher-kva.example/tokens/new?scopes=repo");
    assert_eq!(request.link_host, "xn--bcher-kva.example");
    assert!(request.steps.starts_with("Tick repo, thenGenerate token. x x"), "{}", request.steps);
    assert_eq!(request.steps.chars().count(), 200);
}

#[tokio::test]
async fn the_card_names_the_secret_that_fits_and_follows_the_vault() {
    let (_dir, mut client, mut view) = setup().await;
    call(&mut client, &mut view, "bo", "secrets_request", &asking("https://github.com/settings/tokens/new?scopes=repo")).await;
    let request = latest(&view);
    assert_eq!(request.link_host, "github.com");
    assert_eq!(request.fitting_secret_id, "", "an empty vault: the request is the form");

    // A secret that sets what it asks for, added while it waits: the card
    // names it now.
    let secret = v1::CreateSecret {
        name: "GitHub".into(),
        entries: vec![v1::SecretEntry { variable: "GITHUB_TOKEN".into(), value: "ghp_test_value_000000".into(), file: false }],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::CreateSecret(secret)).await;
    view.follow(&mut client, |view| shown(view, &request.id).fitting_secret_name == "GitHub").await;
    let id = view.workspace.vault.clone().expect("vault").secrets[0].id.clone();
    assert_eq!(shown(&view, &request.id).fitting_secret_id, id);

    // A request made while it's there names it from the start.
    let again = r#"{"name":"Repo token","variables":[{"name":"GITHUB_TOKEN"}],"reason":"To tag."}"#;
    call(&mut client, &mut view, "bo", "secrets_request", again).await;
    assert_eq!(latest(&view).fitting_secret_name, "GitHub");

    // Deleted, it fits no more.
    let delete = v1::DeleteSecret { id: id.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, Request::DeleteSecret(delete)).await;
    view.follow(&mut client, |view| shown(view, &request.id).fitting_secret_id.is_empty()).await;

    // Answered with the form: added, and given.
    let answer = v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: request.id.clone(),
        approve: true,
        entries: vec![v1::SecretEntry { variable: "GITHUB_TOKEN".into(), value: "ghp_test_value_111111".into(), file: false }],
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(answer)).await;
    view.follow(&mut client, |view| shown(view, &request.id).state() == v1::SecretRequestState::Granted && idle(view, "bo")).await;
    let decided = shown(&view, &request.id);
    assert!(decided.added && decided.fitting_secret_id.is_empty(), "{decided:?}");
}

#[tokio::test]
async fn a_generated_key_put_in_place_tells_the_agent_once() {
    let (_dir, mut client, mut view) = setup().await;
    let ask = r#"{"name":"Deploy key","generate":"ssh-ed25519","reason":"To deploy.","place":"claktory/factory","link":"https://github.com/claktory/factory/settings/keys/new"}"#;
    call(&mut client, &mut view, "bo", "secrets_request", ask).await;
    let request = latest(&view);
    assert_eq!(request.place, "claktory/factory");
    let done = || Request::SecretRequestDone(v1::SecretRequestDone { agent_id: "bo".into(), request_id: request.id.clone() });
    let early = apply_request(&mut client, &mut view, done()).await;
    assert!(matches!(early, Body::Error(ref error) if error.message == "Only a generated key can be put in place."), "{early:?}");

    let generate = v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: request.id.clone(),
        approve: true,
        ask_each_time: Some(false),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(generate)).await;
    view.follow(&mut client, |view| shown(view, &request.id).state() == v1::SecretRequestState::Granted && idle(view, "bo")).await;
    // Told once, in the answer: not also in a note of what changed.
    let notes: Vec<&v1::Entry> = view
        .entries
        .iter()
        .filter(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("You can now use the secrets")))
        .collect();
    assert!(notes.is_empty(), "{notes:?}");
    assert!(matches!(apply_request(&mut client, &mut view, done()).await, Body::Ack(_)));
    view.follow(&mut client, |view| shown(view, &request.id).done_ms > 0 && idle(view, "bo")).await;
    assert!(matches!(apply_request(&mut client, &mut view, done()).await, Body::Ack(_)), "again: nothing more");
    let told: Vec<String> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text.contains("it's in place") => Some(event.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(told.len(), 1, "{told:?}");
    assert!(told[0].contains("added the public key of Deploy key to claktory/factory"), "{told:?}");

    // A place that isn't an owner/repo or a host is refused, saying why.
    for place in ["the repo's deploy keys page", "../etc", "owner/repo/extra", "evil\u{202e}com"] {
        let odd = serde_json::json!({"name": "Other key", "generate": "ssh-ed25519", "reason": "To deploy.", "place": place}).to_string();
        let said = call(&mut client, &mut view, "bo", "secrets_request", &odd).await;
        assert!(said.contains("place: an owner/repo"), "{place}: {said}");
    }
    assert_eq!(latest(&view).id, request.id, "no request made");
}

#[tokio::test]
async fn an_import_asked_for_no_one_cant_be_given_to_the_agent() {
    let (dir, mut client, mut view) = setup().await;
    // On Windows as people write it, without canonicalize's \\?\ (which
    // an import refuses).
    #[cfg(unix)]
    let files = dir.canonicalize().expect("real").join("files");
    #[cfg(windows)]
    let files = factoryd::windows::fs::canonical(&dir).expect("real").join("files");
    std::fs::create_dir_all(&files).expect("files");
    std::fs::write(files.join("cert.p12"), [0u8, 1, 2, 3, 255, 254]).expect("file");
    let ask = serde_json::json!({
        "name": "Release cert",
        "import": [{"variable": "CERT", "path": files.join("cert.p12").to_str().expect("text")}],
        "grant": "nobody",
        "reason": "For the release machine.",
    });
    call(&mut client, &mut view, "bo", "secrets_request", &ask.to_string()).await;
    let request = latest(&view);
    assert!(request.grant_to_nobody);
    let give = v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: request.id.clone(),
        approve: true,
        grant_to_asker: true,
        ask_each_time: Some(false),
        ..Default::default()
    };
    let refused = apply_request(&mut client, &mut view, Request::DecideSecretRequest(give)).await;
    assert!(matches!(refused, Body::Error(ref error) if error.message.contains("asked for this to go to no one")), "{refused:?}");
    assert_eq!(shown(&view, &request.id).state(), v1::SecretRequestState::Pending);
}

/// Submit (#370): a comment instead of Give or Dismiss is a reply. Nothing
/// is given, and the agent gets the person's words to ask again with.
#[tokio::test]
async fn a_reply_to_a_request_gives_nothing_and_tells_the_agent_the_persons_words() {
    let (_dir, mut client, mut view) = setup().await;
    call(&mut client, &mut view, "bo", "secrets_request", &asking("https://github.com/settings/tokens/new?scopes=repo")).await;
    let request = latest(&view);
    let reply = |comment: &str| v1::DecideSecretRequest {
        agent_id: "bo".into(),
        request_id: request.id.clone(),
        approve: true,
        comment: comment.into(),
        reply: true,
        ..Default::default()
    };
    let empty = apply_request(&mut client, &mut view, Request::DecideSecretRequest(reply(" "))).await;
    let Body::Error(error) = empty else { panic!("expected a refusal, got {empty:?}") };
    assert_eq!(error.message, "Write a reply, or give a secret or dismiss the request.");

    let replied = apply_request(&mut client, &mut view, Request::DecideSecretRequest(reply("Use the deploy key instead."))).await;
    assert!(matches!(replied, Body::Ack(_)), "{replied:?}");
    let told = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.summary.starts_with("The person replied to your request") => Some(event.clone()),
            _ => None,
        })
    };
    view.follow(&mut client, |view| told(view).is_some() && idle(view, "bo")).await;
    let event = told(&view).expect("told");
    assert_eq!(event.summary, "The person replied to your request for GitHub token.");
    assert_eq!(
        event.text,
        "The person replied to your request for GitHub token; nothing was given. They say:\n\nUse the deploy key instead.\n\nIf you still \
         need it, ask again with that in mind.\n\nYou asked because: To push."
    );
    let decided = shown(&view, &request.id);
    assert_eq!(
        (decided.state(), decided.replied, decided.comment.as_str(), decided.given_secret_id.as_str()),
        (v1::SecretRequestState::Dismissed, true, "Use the deploy key instead.", "")
    );
    assert!(view.workspace.vault.clone().unwrap_or_default().secrets.is_empty(), "nothing was added or given");
}
