//! Site logins for agents' browsers: a login asked for from the browser is bound to the origin Chrome reported, saved and granted
//! fill-only, so no command ever gets it; the site refusing it asks again
//! on the same card, and Sign in replaces the same login; one of the
//! person's fits only on its exact site.

use std::sync::Arc;

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, SecretLoginOutcome, SecretRequestState, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::Factory;
use factoryd::vault::logins::{ask_login_for_test, login_outcome_for_test, login_request_for_test};

const PASSWORD: &str = "correct-horse-battery-staple";

fn entry(variable: &str, value: &str) -> v1::SecretEntry {
    v1::SecretEntry { variable: variable.into(), value: value.into(), file: false }
}

fn login(password: &str) -> Vec<v1::SecretEntry> {
    vec![entry("USERNAME", "matt@example.com"), entry("PASSWORD", password)]
}

fn decide(agent: &str, id: &str, entries: Vec<v1::SecretEntry>) -> v1::DecideSecretRequest {
    v1::DecideSecretRequest { agent_id: agent.into(), request_id: id.into(), approve: true, entries, ..Default::default() }
}

fn vault(view: &View) -> v1::VaultState {
    view.workspace.vault.clone().expect("vault state")
}

fn secret(view: &View, name: &str) -> v1::SecretInfo {
    vault(view).secrets.into_iter().find(|secret| secret.name == name).unwrap_or_else(|| panic!("{name}"))
}

fn grants(secret: &v1::SecretInfo) -> Vec<(String, bool)> {
    secret.grants.iter().map(|grant| (grant.agent_id.clone(), grant.fill_only)).collect()
}

fn error(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

async fn team(dir: &std::path::Path) -> (Client, View, Arc<Factory>) {
    let (addr, factory) = start_with(dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Yukako", "")).await;
    apply_request(&mut client, &mut view, create_agent("site", "Bo", "")).await;
    (client, view, factory)
}

#[tokio::test]
async fn a_login_is_saved_for_its_site_only_and_given_to_fill_never_to_a_command() {
    let dir = temp_dir();
    let (mut client, mut view, factory) = team(&dir).await;
    let asked = ask_login_for_test(&factory, "yukako", "https://x.com");
    assert!(asked.login && asked.fill_only, "{asked:?}");
    assert_eq!((asked.sites.as_slice(), asked.login_site.as_str(), asked.secret_name.as_str()), (&["https://x.com".to_owned()][..], "x.com", "X login"));
    let variables: Vec<String> = asked.new_secret.as_ref().expect("what it needs").variables.iter().map(|variable| variable.name.clone()).collect();
    assert_eq!(variables, ["USERNAME", "PASSWORD"]);
    // Asked again while it waits: the same request, not a second.
    assert_eq!(ask_login_for_test(&factory, "yukako", "https://x.com").id, asked.id);

    // Signed in with the two values, as an older app's card sends them:
    // sealed to the site and fill-only all the same, since the backend sets
    // both from the request.
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("yukako", &asked.id, login(PASSWORD)))).await;
    let saved = secret(&view, "X login");
    assert_eq!(saved.sites, ["https://x.com"]);
    assert_eq!(grants(&saved), [("yukako".to_owned(), true)]);
    let now = login_request_for_test(&factory, "yukako", "https://x.com").expect("the request");
    assert_eq!((now.state(), now.login_outcome(), now.given_secret_name.as_str()), (SecretRequestState::Granted, SecretLoginOutcome::SigningIn, "X login"));
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "yukako".into() })).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|line| line.contains("call browser_sign_in again"))).await;
    assert!(!describe(&view.entries).iter().any(|line| line.contains("bash's `secrets`")), "never told to pass it to a command");

    // No command gets it.
    let output = call(&mut client, &mut view, "yukako", "bash", r#"{"command":"echo $PASSWORD","secrets":["X login"]}"#).await;
    assert!(output.contains("no command can have it") && !output.contains(PASSWORD), "{output}");

    // The site refused it: the same card waits again, and Sign in replaces
    // the same login's values, with no second login made.
    login_outcome_for_test(&factory, &asked.id, SecretLoginOutcome::Refused);
    let refused = login_request_for_test(&factory, "yukako", "https://x.com").expect("the request");
    assert_eq!((refused.id.as_str(), refused.state(), refused.login_outcome()), (asked.id.as_str(), SecretRequestState::Pending, SecretLoginOutcome::Refused));
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(decide("yukako", &asked.id, login("another-password-entirely")))).await;
    let logins: Vec<v1::SecretInfo> = vault(&view).secrets.into_iter().filter(|secret| secret.name.starts_with("X login")).collect();
    assert_eq!(logins.len(), 1, "{logins:?}");
    assert_eq!((logins[0].id.as_str(), logins[0].version), (saved.id.as_str(), 2));
    assert_eq!(grants(&logins[0]), [("yukako".to_owned(), true)]);
    let again = login_request_for_test(&factory, "yukako", "https://x.com").expect("the request");
    assert_eq!((again.state(), again.login_outcome()), (SecretRequestState::Granted, SecretLoginOutcome::SigningIn));
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn one_of_yours_fits_only_on_its_exact_site_and_is_given_fill_only() {
    let dir = temp_dir();
    let (mut client, mut view, factory) = team(&dir).await;
    let create = v1::CreateSecret { name: "X login".into(), entries: login(PASSWORD), ..Default::default() };
    apply_request(&mut client, &mut view, Request::CreateSecret(create)).await;
    let x = secret(&view, "X login").id;
    apply_request(&mut client, &mut view, Request::SetSecretSites(v1::SetSecretSites { id: x.clone(), sites: vec!["https://x.com".into()] })).await;
    // Bo holds it for its commands; that's no reason Yukako's would.
    apply_request(&mut client, &mut view, Request::GrantSecret(v1::GrantSecret { id: x.clone(), agent_id: "bo".into(), ..Default::default() })).await;

    let on_x = ask_login_for_test(&factory, "yukako", "https://x.com");
    let shown = login_request_for_test(&factory, "yukako", "https://x.com").expect("the request");
    assert_eq!((shown.fitting_secret_id.as_str(), shown.lookalike_secret_name.as_str()), (x.as_str(), ""));
    assert!(login_request_for_test(&factory, "yukako", "https://www.x.com").is_none());

    // A page named like it, on another site: nothing fits, and the card
    // says whose site the login is for.
    let lookalike = ask_login_for_test(&factory, "yukako", "https://x-login.com");
    let shown = login_request_for_test(&factory, "yukako", "https://x-login.com").expect("the request");
    assert_eq!(
        (shown.fitting_secret_id.as_str(), shown.lookalike_secret_name.as_str(), shown.lookalike_site.as_str()),
        ("", "X login", "x.com")
    );
    // Whatever an app sends, a login for another site isn't given there,
    // and no admin agent is asked to find one.
    let give = v1::DecideSecretRequest { secret_id: x.clone(), ..decide("yukako", &lookalike.id, Vec::new()) };
    let refused = client.request(Request::DecideSecretRequest(give)).await;
    assert!(error(&refused.body).contains("isn't for x-login.com"), "{:?}", refused.body);
    let to_admin = v1::DecideSecretRequest { approve_only: true, ..decide("yukako", &lookalike.id, Vec::new()) };
    let refused = client.request(Request::DecideSecretRequest(to_admin)).await;
    assert!(error(&refused.body).contains("yours to give"), "{:?}", refused.body);

    // Use it, on its own site: fill-only for Yukako, Bo's grant as it was.
    let use_it = v1::DecideSecretRequest { secret_id: x.clone(), ..decide("yukako", &on_x.id, Vec::new()) };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(use_it)).await;
    let mut given = grants(&secret(&view, "X login"));
    given.sort();
    assert_eq!(given, [("bo".to_owned(), false), ("yukako".to_owned(), true)]);
    assert_eq!(secret(&view, "X login").sites, ["https://x.com"], "its sites as they were");
    let output = call(&mut client, &mut view, "yukako", "bash", r#"{"command":"echo $PASSWORD","secrets":["X login"]}"#).await;
    assert!(output.contains("no command can have it") && !output.contains(PASSWORD), "{output}");

    // An agent the browser never serves isn't given a login to fill in.
    let to_rivet = v1::GrantSecret { id: x.clone(), agent_id: "rivet".into(), fill_only: true, ..Default::default() };
    let refused = client.request(Request::GrantSecret(to_rivet)).await;
    assert!(error(&refused.body).contains("Admin agents can't use the browser"), "{:?}", refused.body);
    assert!(!grants(&secret(&view, "X login")).iter().any(|(agent, _)| agent == "rivet"));
    let _ = std::fs::remove_dir_all(dir);
}

/// A website login is never given for an agent's commands on a request,
/// by the person on its card or by an admin agent, nor offered there as
/// the secret that fits; only Settings → Secrets gives one to commands.
#[tokio::test]
async fn a_website_login_is_never_given_for_commands_on_a_request() {
    let dir = temp_dir();
    let (mut client, mut view, _factory) = team(&dir).await;
    let create = v1::CreateSecret { name: "X login".into(), entries: login(PASSWORD), ..Default::default() };
    apply_request(&mut client, &mut view, Request::CreateSecret(create)).await;
    let x = secret(&view, "X login").id;
    apply_request(&mut client, &mut view, Request::SetSecretSites(v1::SetSecretSites { id: x.clone(), sites: vec!["https://x.com".into()] })).await;

    call(&mut client, &mut view, "yukako", "secrets_request", r#"{"name":"X login","variables":[{"name":"PASSWORD"}],"reason":"To ship."}"#).await;
    let asked = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::SecretRequest(request)) => Some(request.clone()),
            _ => None,
        })
        .expect("the request's card");
    assert_eq!(asked.fitting_secret_id, "", "a login isn't offered as the one that fits");

    let give = v1::DecideSecretRequest { secret_id: x.clone(), ..decide("yukako", &asked.id, Vec::new()) };
    let refused = client.request(Request::DecideSecretRequest(give)).await;
    assert!(error(&refused.body).contains("X login is a website login"), "{:?}", refused.body);

    let model = view.agent("yukako").model.clone();
    let rivet = v1::UpdateAgent { agent_id: "rivet".into(), model: Some(model), effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rivet)).await;
    let to_admin = v1::DecideSecretRequest { approve_only: true, ..decide("yukako", &asked.id, Vec::new()) };
    apply_request(&mut client, &mut view, Request::DecideSecretRequest(to_admin)).await;
    let answered = call(&mut client, &mut view, "rivet", "secrets_grant", &format!(r#"{{"secret":"X login","request":"{}"}}"#, asked.id)).await;
    assert!(answered.contains("only the person gives one"), "{answered}");
    let granted = call(&mut client, &mut view, "rivet", "secrets_grant", r#"{"secret":"X login","agents":["bo"]}"#).await;
    assert!(granted.contains("only the person gives one"), "{granted}");
    assert_eq!(grants(&secret(&view, "X login")), []);

    let output = call(&mut client, &mut view, "yukako", "bash", r#"{"command":"echo $PASSWORD","secrets":["X login"]}"#).await;
    assert!(!output.contains(PASSWORD), "{output}");

    // The person, in Settings → Secrets, still can.
    apply_request(&mut client, &mut view, Request::GrantSecret(v1::GrantSecret { id: x, agent_id: "bo".into(), ..Default::default() })).await;
    assert_eq!(grants(&secret(&view, "X login")), [("bo".to_owned(), false)]);
    let _ = std::fs::remove_dir_all(dir);
}
