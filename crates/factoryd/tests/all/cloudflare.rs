//! Connecting Cloudflare (connectors/cloudflare.rs) against a stand-in for
//! Cloudflare's OAuth and API: the person signs in with PKCE, the grant
//! stays in the vault, refreshed and rotated as it's used, and a secret made
//! from it is an account-owned token with only the permissions and zones
//! chosen, extended while it's used and deleted when it goes.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use factory_protocol::v1::{self, ConnectFlowState, ConnectionStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use factoryd::connectors::cloudflare::Cloudflare;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::common::stand_in::{Request as Call, StandIn};
use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};

const REDIRECT: &str = "http://127.0.0.1:53682/oauth/cloudflare";
const DAY_MS: i64 = 24 * 3600 * 1000;

/// What the stand-in Cloudflare knows.
#[derive(Default)]
struct Cloud {
    /// Grants handed out: the latest access and refresh tokens are good.
    issued: usize,
    /// The PKCE challenge the consent page was opened with.
    challenge: String,
    /// The person took Claktory's access away.
    revoked: bool,
    /// Minted tokens: id → (value, the body it was made or last changed with).
    tokens: BTreeMap<String, (String, Value)>,
    minted: usize,
    /// Refresh tokens used, in order.
    refreshed: Vec<String>,
    /// The account's token the person made on Cloudflare's page, while it
    /// isn't deleted.
    pasted: Option<String>,
}

const PASTED: &str = "cf-pasted-0123456789abcdef";

fn envelope(result: Value) -> (u16, Value) {
    (200, json!({"success": true, "errors": [], "result": result, "result_info": {"page": 1, "total_pages": 1}}))
}

fn grant(cloud: &mut Cloud) -> (u16, Value) {
    cloud.issued += 1;
    let n = cloud.issued;
    // Half a minute: inside the refresh margin, so every use refreshes it.
    (
        200,
        json!({"access_token": format!("cf-access-{n}-abcdefgh"), "refresh_token": format!("cf-refresh-{n}-abcdefgh"), "expires_in": 30, "scope": "account-api-tokens.write zone.read account.read"}),
    )
}

fn answer(cloud: &mut Cloud, call: &Call) -> (u16, Value) {
    let path = call.path();
    if path == "/oauth2/token" {
        assert_eq!(call.form("client_id").as_deref(), Some("test-client"));
        assert_eq!(call.form("client_secret"), None, "a public client has no secret");
        return match call.form("grant_type").as_deref() {
            Some("authorization_code") => {
                let verifier = call.form("code_verifier").unwrap_or_default();
                assert_eq!(URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes())), cloud.challenge, "PKCE");
                assert_eq!(call.form("redirect_uri").as_deref(), Some(REDIRECT));
                match call.form("code").as_deref() {
                    Some("good-code") => grant(cloud),
                    _ => (400, json!({"error": "invalid_grant", "error_description": "The code is wrong."})),
                }
            }
            Some("refresh_token") => {
                let token = call.form("refresh_token").unwrap_or_default();
                cloud.refreshed.push(token.clone());
                if cloud.revoked || token != format!("cf-refresh-{}-abcdefgh", cloud.issued) {
                    (400, json!({"error": "invalid_grant", "error_description": "The refresh token is invalid."}))
                } else {
                    grant(cloud)
                }
            }
            other => panic!("grant type {other:?}"),
        };
    }
    if path == "/oauth2/revoke" {
        assert_eq!(call.form("token"), Some(format!("cf-refresh-{}-abcdefgh", cloud.issued)), "the refresh token ends the grant");
        cloud.revoked = true;
        return (200, json!({}));
    }
    // Everything else takes the latest access token, or the pasted one.
    let pasted = cloud.pasted.as_ref().is_some_and(|token| call.header("authorization") == Some(&format!("Bearer {token}")));
    if !pasted && (cloud.revoked || call.header("authorization") != Some(&format!("Bearer cf-access-{}-abcdefgh", cloud.issued))) {
        return (401, json!({"success": false, "errors": [{"code": 10000, "message": "Authentication error"}], "result": null}));
    }
    match (call.method.as_str(), path) {
        // An account's own token can't check itself as a user's.
        ("GET", "/client/v4/user/tokens/verify") => {
            (401, json!({"success": false, "errors": [{"code": 1000, "message": "Invalid API Token"}], "result": null}))
        }
        ("GET", "/client/v4/accounts/acc/tokens/verify") => envelope(json!({"id": "pasted-1", "status": "active"})),
        ("GET", "/client/v4/accounts/acc/tokens/pasted-1") => envelope(json!({"id": "pasted-1", "policies": [
            {"permission_groups": [{"id": "tok-edit", "name": "Account API Tokens Write"}, {"id": "zone-read", "name": "Zone Read"}]},
            {"permission_groups": [{"id": "zone-read", "name": "Zone Read"}]},
        ]})),
        ("DELETE", "/client/v4/accounts/acc/tokens/pasted-1") => {
            cloud.pasted = None;
            envelope(json!({"id": "pasted-1"}))
        }
        ("GET", "/oauth2/userinfo") => (200, json!({"sub": "user-7", "email": "matt@example.com"})),
        ("GET", "/client/v4/accounts") => envelope(json!([{"id": "acc", "name": "Acme"}])),
        ("GET", "/client/v4/zones") => {
            assert_eq!(call.query("account.id").as_deref(), Some("acc"));
            let page: u32 = call.query("page").and_then(|page| page.parse().ok()).unwrap_or(1);
            let zone = if page == 1 { json!({"id": "z1", "name": "claktory.com"}) } else { json!({"id": "z2", "name": "example.com"}) };
            (200, json!({"success": true, "errors": [], "result": [zone], "result_info": {"page": page, "total_pages": 2}}))
        }
        ("GET", "/client/v4/accounts/acc/tokens/permission_groups") => envelope(json!([
            {"id": "dns-write", "name": "DNS Write", "scopes": ["com.cloudflare.api.account.zone"]},
            {"id": "pages-write", "name": "Pages Write", "scopes": ["com.cloudflare.api.account"]},
            {"id": "memberships", "name": "Memberships Read", "scopes": ["com.cloudflare.api.user"]},
        ])),
        ("POST", "/client/v4/accounts/acc/tokens") => {
            cloud.minted += 1;
            let (id, value) = (format!("tok-{}", cloud.minted), format!("minted-{}-0123456789", cloud.minted));
            cloud.tokens.insert(id.clone(), (value.clone(), call.json()));
            envelope(json!({"id": id, "value": value}))
        }
        (method, path) if path.starts_with("/client/v4/accounts/acc/tokens/") => {
            let id = path.rsplit('/').next().unwrap_or_default().to_owned();
            if !cloud.tokens.contains_key(&id) {
                return (404, json!({"success": false, "errors": [{"code": 1003, "message": "Invalid token id"}], "result": null}));
            }
            match method {
                "PUT" => {
                    cloud.tokens.get_mut(&id).expect("token").1 = call.json();
                    envelope(json!({"id": id}))
                }
                "DELETE" => {
                    cloud.tokens.remove(&id);
                    envelope(json!({"id": id}))
                }
                other => panic!("{other} {path}"),
            }
        }
        other => panic!("unexpected {other:?}"),
    }
}

fn failure(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected an error, got {other:?}"),
    }
}

fn vault(view: &View) -> v1::VaultState {
    view.workspace.vault.clone().expect("vault state")
}

fn secret(view: &View, name: &str) -> v1::SecretInfo {
    vault(view).secrets.into_iter().find(|secret| secret.name == name).unwrap_or_else(|| panic!("no secret {name}"))
}

fn query(url: &str, key: &str) -> String {
    let url = reqwest::Url::parse(url).expect("url");
    url.query_pairs().find(|(name, _)| name == key).map(|(_, value)| value.into_owned()).expect(key)
}

async fn ok(client: &mut Client, view: &mut View, body: Request) {
    let reply = apply_request(client, view, body).await;
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
}

/// What `agent`'s bash command answered.
async fn bash(client: &mut Client, view: &mut View, agent: &str, args: &str) -> String {
    call(client, view, agent, "bash", args).await
}

/// What `agent`'s call of `tool` answered.
async fn call(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call {tool} {args}"))).await;
    let idle = |view: &View| {
        let agent = view.agent(agent);
        agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
    };
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

#[tokio::test]
async fn cloudflare_connects_with_pkce_and_gives_agents_only_narrow_tokens_it_renews_and_deletes() {
    let (mut client, mut view, cloud, _stand_in) = setup().await;
    let connector = vault(&view).connectors.into_iter().find(|connector| connector.id == "cloudflare").expect("in the catalog");
    assert_eq!(connector.variables, ["CLOUDFLARE_API_TOKEN", "CLOUDFLARE_ACCOUNT_ID"]);

    // Cloudflare's consent page, for Claktory's public client, with PKCE.
    let start =
        Request::StartConnect(v1::StartConnect { connector_id: "cloudflare".into(), redirect_uri: REDIRECT.into(), ..Default::default() });
    let Body::ConnectStarted(started) = apply_request(&mut client, &mut view, start).await else { panic!("not started") };
    assert!(started.url.starts_with("https://dash.cloudflare.com/oauth2/auth?"), "{}", started.url);
    assert_eq!(query(&started.url, "client_id"), "test-client");
    assert_eq!(query(&started.url, "scope"), "account-api-tokens.write account-settings.read zone.read");
    assert_eq!(query(&started.url, "code_challenge_method"), "S256");
    cloud.lock().expect("lock").challenge = query(&started.url, "code_challenge");
    let state = query(&started.url, "state");
    let finish = Request::FinishConnect(v1::FinishConnect {
        flow_id: started.flow_id.clone(),
        callback_url: format!("{REDIRECT}?code=good-code&state={state}"),
        token: String::new(),
    });
    ok(&mut client, &mut view, finish).await;
    let connected =
        |view: &View| vault(view).flows.iter().any(|flow| flow.id == started.flow_id && flow.state() == ConnectFlowState::Connected);
    view.follow(&mut client, connected).await;
    let connection = vault(&view).secrets.into_iter().find(|secret| secret.connection.is_some()).expect("a connection");
    let info = connection.connection.clone().expect("a connection");
    assert_eq!((info.account.as_str(), info.status()), ("matt@example.com", ConnectionStatus::Ready));
    assert_eq!(info.resources, ["Cloudflare account Acme"]);
    assert_eq!(info.scopes, ["account-api-tokens.write", "zone.read", "account.read"], "what Cloudflare granted, not what was asked");
    assert_eq!(info.access, ["Make and delete API tokens in your accounts", "See your zones (domains)", "Account Read"]);
    assert!(connection.variables.is_empty() && connection.grants.is_empty());

    // What a secret can be made of: the account, its zones (every page), and
    // the permissions an account's token can have.
    let list = Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() });
    let Body::ConnectChoices(choices) = apply_request(&mut client, &mut view, list).await else { panic!("no choices") };
    let listed: Vec<(&str, &str, &str)> =
        choices.choices.iter().map(|choice| (choice.kind.as_str(), choice.id.as_str(), choice.parent.as_str())).collect();
    assert_eq!(
        listed,
        [
            ("account", "acc", ""),
            ("zone", "z1", "acc"),
            ("zone", "z2", "acc"),
            ("permission", "dns-write", ""),
            ("permission", "pages-write", "")
        ]
    );
    assert!(choices.rules.starts_with("Choose one account and at least one permission."), "{}", choices.rules);

    // Only what makes a working token.
    let derive = |name: &str, permissions: &[&str], resources: &[&str], expires_ms: i64| {
        Request::DeriveSecret(v1::DeriveSecret {
            secret_id: connection.id.clone(),
            name: name.into(),
            description: String::new(),
            permissions: permissions.iter().map(|id| (*id).to_owned()).collect(),
            resources: resources.iter().map(|id| (*id).to_owned()).collect(),
            expires_ms,
        })
    };
    let refused = |reply: Body| failure(&reply);
    assert_eq!(
        refused(apply_request(&mut client, &mut view, derive("x", &["dns-write"], &["z1"], 0)).await),
        "Choose exactly one account (one of: \"acc\" Acme), plus any of its zones to limit it to."
    );
    assert_eq!(refused(apply_request(&mut client, &mut view, derive("x", &[], &["acc"], 0)).await), "Choose at least one permission.");
    assert_eq!(
        refused(apply_request(&mut client, &mut view, derive("x", &["dns-write"], &["acc", "z9"], 0)).await),
        "\"z9\" isn't a zone in the Acme account."
    );
    assert_eq!(
        refused(apply_request(&mut client, &mut view, derive("x", &["memberships"], &["acc"], 0)).await),
        "\"memberships\" isn't a permission a Cloudflare token can have."
    );
    assert_eq!(
        refused(apply_request(&mut client, &mut view, derive("x", &["pages-write"], &["acc", "z1"], 0)).await),
        "Zones apply only to zone permissions. Choose a zone permission, or no zones."
    );
    assert!(cloud.lock().expect("lock").tokens.is_empty(), "nothing minted for a refusal");

    // DNS on one zone: a token for that, 30 days unless asked otherwise.
    ok(&mut client, &mut view, derive("Cloudflare DNS", &["dns-write"], &["acc", "z1"], 0)).await;
    let made = secret(&view, "Cloudflare DNS");
    let export = made.export.clone().expect("made from the connection");
    assert_eq!(export.access, ["DNS Write on claktory.com"]);
    assert_eq!(
        (export.permissions.as_slice(), export.resources.as_slice()),
        (["DNS Write".to_owned()].as_slice(), ["Cloudflare account Acme".to_owned(), "claktory.com".to_owned()].as_slice())
    );
    let now = factoryd::transcript::now_ms();
    assert!((export.expires_ms - (now + 30 * DAY_MS)).abs() < 60_000, "30 days");
    assert!(!export.per_use);
    let mut variables: Vec<String> = made.variables.iter().map(|variable| variable.name.clone()).collect();
    variables.sort();
    assert_eq!(variables, ["CLOUDFLARE_ACCOUNT_ID", "CLOUDFLARE_API_TOKEN"]);
    {
        let cloud = cloud.lock().expect("lock");
        let (_, body) = &cloud.tokens["tok-1"];
        assert_eq!(body["name"], "Claktory: Cloudflare DNS");
        assert_eq!(
            body["policies"],
            json!([{"effect": "allow", "permission_groups": [{"id": "dns-write"}], "resources": {"com.cloudflare.api.account.zone.z1": "*"}}])
        );
    }

    // A short one is due at once: a command's use extends the same token.
    assert_eq!(
        refused(apply_request(&mut client, &mut view, derive("Pages", &["pages-write"], &["acc"], now - 1)).await),
        "That end date has passed. Choose a later one, or none for 30 days."
    );
    ok(&mut client, &mut view, derive("Pages", &["pages-write"], &["acc"], factoryd::transcript::now_ms() + 1000)).await;
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    let pages = secret(&view, "Pages");
    let given = apply_request(
        &mut client,
        &mut view,
        Request::GrantSecret(v1::GrantSecret { id: pages.id.clone(), agent_id: "builder".into(), proof: None }),
    )
    .await;
    assert!(matches!(given, Body::VaultSession(_)), "{given:?}");
    let command = r#"{"command":"echo \"$CLOUDFLARE_ACCOUNT_ID $CLOUDFLARE_API_TOKEN\"","secrets":["Pages"]}"#;
    let said = bash(&mut client, &mut view, "builder", command).await;
    assert_eq!(said.lines().next(), Some("acc [secret Pages/CLOUDFLARE_API_TOKEN]"), "{said}");
    {
        let cloud = cloud.lock().expect("lock");
        let (value, body) = &cloud.tokens["tok-2"];
        assert_eq!((value.as_str(), body["status"].as_str()), ("minted-2-0123456789", Some("active")), "extended, not replaced");
        assert_eq!(body["policies"][0]["resources"], json!({"com.cloudflare.api.account.acc": "*"}));
    }
    assert_eq!(secret(&view, "Pages").version, 1, "the same value");

    // Deleted at Cloudflare meanwhile: a new token, as it was.
    cloud.lock().expect("lock").tokens.remove("tok-2");
    tokio::time::sleep(std::time::Duration::from_millis(600)).await;
    bash(&mut client, &mut view, "builder", command).await;
    view.follow(&mut client, |view| secret(view, "Pages").version == 2).await;
    assert!(cloud.lock().expect("lock").tokens.contains_key("tok-3"));

    // Every use refreshed the grant, each time with the newest refresh token.
    {
        let cloud = cloud.lock().expect("lock");
        let expected: Vec<String> = (1..cloud.issued).map(|n| format!("cf-refresh-{n}-abcdefgh")).collect();
        assert!(expected.len() >= 4, "{expected:?}");
        assert_eq!(cloud.refreshed, expected);
    }

    // Disconnected and revoked: its tokens deleted at Cloudflare, the grant ended.
    ok(&mut client, &mut view, Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: true })).await;
    view.follow(&mut client, |view| vault(view).secrets.is_empty()).await;
    let cloud = cloud.lock().expect("lock");
    assert!(cloud.revoked && cloud.tokens.is_empty(), "{:?}", cloud.tokens.keys());
}

#[tokio::test]
async fn a_cloudflare_grant_taken_away_needs_connecting_again() {
    let (mut client, mut view, cloud, _stand_in) = setup().await;
    let start =
        Request::StartConnect(v1::StartConnect { connector_id: "cloudflare".into(), redirect_uri: REDIRECT.into(), ..Default::default() });
    let Body::ConnectStarted(started) = apply_request(&mut client, &mut view, start).await else { panic!("not started") };
    cloud.lock().expect("lock").challenge = query(&started.url, "code_challenge");
    let state = query(&started.url, "state");

    // A code Cloudflare doesn't take fails the flow, saying so.
    let finish = |code: &str| {
        Request::FinishConnect(v1::FinishConnect {
            flow_id: started.flow_id.clone(),
            callback_url: format!("{REDIRECT}?code={code}&state={state}"),
            token: String::new(),
        })
    };
    ok(&mut client, &mut view, finish("bad-code")).await;
    let failed =
        |view: &View| vault(view).flows.into_iter().find(|flow| flow.id == started.flow_id && flow.state() == ConnectFlowState::Failed);
    view.follow(&mut client, |view| failed(view).is_some()).await;
    assert_eq!(failed(&view).expect("failed").error, "Cloudflare didn't accept the sign-in: The code is wrong. Start again.");

    let connection = connect(&mut client, &mut view, &cloud).await;

    // The person removes Claktory's access at Cloudflare.
    cloud.lock().expect("lock").revoked = true;
    let list = Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() });
    let said = failure(&apply_request(&mut client, &mut view, list).await);
    assert_eq!(said, format!("{} needs to be connected again. Cloudflare no longer accepts Claktory's access.", connection.name));
    view.follow(&mut client, |view| {
        vault(view)
            .secrets
            .iter()
            .any(|secret| secret.connection.as_ref().is_some_and(|info| info.status() == ConnectionStatus::NeedsReconnect))
    })
    .await;
}

/// A Claktory that can connect Cloudflare (as the stand-in plays it),
/// with Rivet and a Builder on the scripted model.
async fn setup() -> (Client, View, Arc<Mutex<Cloud>>, StandIn) {
    let (client, view, cloud, stand_in, _) = setup_with(Cloudflare::new("test-client")).await;
    (client, view, cloud, stand_in)
}

/// The same with `recipe`, and where the Claktory is.
async fn setup_with(recipe: Cloudflare) -> (Client, View, Arc<Mutex<Cloud>>, StandIn, std::net::SocketAddr) {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let cloud = Arc::new(Mutex::new(Cloud::default()));
    let stand_in = StandIn::start({
        let cloud = cloud.clone();
        move |call| answer(&mut cloud.lock().expect("lock"), call)
    })
    .await;
    let api = format!("{}/client/v4", stand_in.base);
    factory.add_connector(Arc::new(recipe), &[("cloudflare.dash", &stand_in.base), ("cloudflare.api", &api)]);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    for agent in ["builder", "rivet"] {
        let model = v1::UpdateAgent { agent_id: agent.into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
        apply_request(&mut client, &mut view, Request::UpdateAgent(model)).await;
    }
    (client, view, cloud, stand_in, addr)
}

/// Connects Cloudflare, the person approving: the connection.
async fn connect(client: &mut Client, view: &mut View, cloud: &Mutex<Cloud>) -> v1::SecretInfo {
    let start =
        Request::StartConnect(v1::StartConnect { connector_id: "cloudflare".into(), redirect_uri: REDIRECT.into(), ..Default::default() });
    let Body::ConnectStarted(started) = apply_request(client, view, start).await else { panic!("not started") };
    cloud.lock().expect("lock").challenge = query(&started.url, "code_challenge");
    let state = query(&started.url, "state");
    let finish = Request::FinishConnect(v1::FinishConnect {
        flow_id: started.flow_id.clone(),
        callback_url: format!("{REDIRECT}?code=good-code&state={state}"),
        token: String::new(),
    });
    ok(client, view, finish).await;
    view.follow(client, |view| vault(view).secrets.iter().any(|secret| secret.connection.is_some())).await;
    vault(view).secrets.into_iter().find(|secret| secret.connection.is_some()).expect("connected")
}

#[tokio::test]
async fn rivet_reads_a_connection_and_gives_an_agent_only_a_narrow_secret_made_from_it() {
    let (mut client, mut view, cloud, _stand_in) = setup().await;
    let connection = connect(&mut client, &mut view, &cloud).await;
    let name = connection.name.clone();

    // What the connection reaches, never a value; nothing made from it yet.
    let said = call(&mut client, &mut view, "rivet", "secrets_inspect", &json!({"secret": name}).to_string()).await;
    assert!(said.starts_with(&format!("{name}: a connected Cloudflare service (ready). Given to no one")), "{said}");
    assert!(said.contains("Account: matt@example.com") && said.contains("Resources: Cloudflare account Acme"), "{said}");
    assert!(said.contains("No secret has been made from it yet."), "{said}");
    assert!(!said.contains("cf-access-") && !said.contains("cf-refresh-"), "no token: {said}");
    let listed = call(&mut client, &mut view, "rivet", "secrets_list", "{}").await;
    assert_eq!(
        listed,
        format!(
            "{name} (a connected Cloudflare service, given to no one: make a secret from it with secrets_derive): \
             Cloudflare: Make and delete API tokens in your accounts; See your zones (domains); Account Read"
        ),
        "described as what it reaches"
    );

    // It can't be given, only what's made from it.
    let given = call(&mut client, &mut view, "rivet", "secrets_grant", &json!({"secret": name, "agents": ["builder"]}).to_string()).await;
    assert!(given.contains("itself. Make a secret from it with only the access the work needs"), "{given}");

    let choices = call(&mut client, &mut view, "rivet", "secrets_choices", &json!({"secret": name}).to_string()).await;
    assert!(choices.starts_with("Choose one account and at least one permission."), "{choices}");
    assert!(choices.contains("Accounts:\n- acc: Acme"), "{choices}");
    assert!(choices.contains("Zones:\n- z1: claktory.com (in acc)\n- z2: example.com (in acc)"), "{choices}");
    assert!(choices.contains("- dns-write: DNS Write — Applies to the zones you choose, or all of them."), "{choices}");

    // Refused as the person would be; then made, for 7 days, given to no one.
    let derive = |permissions: &[&str], resources: &[&str]| {
        json!({"secret": name, "name": "Site DNS", "description": "DNS for the site.", "permissions": permissions, "resources": resources, "days": 7})
            .to_string()
    };
    let refused = call(&mut client, &mut view, "rivet", "secrets_derive", &derive(&[], &["acc"])).await;
    assert!(refused.contains("Choose at least one permission."), "{refused}");
    let made = call(&mut client, &mut view, "rivet", "secrets_derive", &derive(&["dns-write"], &["acc", "z1"])).await;
    assert!(made.starts_with("Made Site DNS: DNS Write on claktory.com. It lasts until "), "{made}");
    assert!(made.ends_with("It's given to no one yet: give it with secrets_grant, and tell the agent what it got."), "{made}");
    let site = secret(&view, "Site DNS");
    let export = site.export.clone().expect("made from the connection");
    let week = factoryd::transcript::now_ms() + 7 * DAY_MS;
    assert!((export.expires_ms - week).abs() < 60_000, "7 days");
    assert!(site.grants.is_empty());
    assert_eq!(cloud.lock().expect("lock").tokens.len(), 1);

    // Given, it shows who has it, from either side.
    let given =
        call(&mut client, &mut view, "rivet", "secrets_grant", &json!({"secret": "Site DNS", "agents": ["builder"]}).to_string()).await;
    assert_eq!(given, "Gave Site DNS to Builder (builder).");
    let said = call(&mut client, &mut view, "rivet", "secrets_inspect", &json!({"secret": name}).to_string()).await;
    assert!(said.contains("Secrets made from it:\n- Site DNS: DNS Write on claktory.com — given to Builder (builder)"), "{said}");
    let said = call(&mut client, &mut view, "rivet", "secrets_inspect", &json!({"secret": "Site DNS"}).to_string()).await;
    assert!(said.starts_with(&format!("Site DNS: made from {name}, given to Builder (builder).")), "{said}");
    assert!(said.contains("Permissions: DNS Write") && said.contains("Sets CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID"), "{said}");
    let listed = call(&mut client, &mut view, "rivet", "secrets_list", "{}").await;
    assert!(
        listed.contains(&format!(
            "Site DNS (sets CLOUDFLARE_API_TOKEN, CLOUDFLARE_ACCOUNT_ID; made from {name}): DNS for the site. — given to builder"
        )),
        "{listed}"
    );

    // Only admin agents have them.
    let builder = call(&mut client, &mut view, "builder", "secrets_choices", &json!({"secret": name}).to_string()).await;
    assert!(!builder.contains("Accounts:"), "{builder}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cloudflare_connects_with_a_pasted_token_that_mints_narrow_ones_and_is_deleted_with_them() {
    let (mut client, mut view, cloud, stand_in, addr) = setup_with(Cloudflare::pasted()).await;
    cloud.lock().expect("lock").pasted = Some(PASTED.to_owned());
    let connector = vault(&view).connectors.into_iter().find(|connector| connector.id == "cloudflare").expect("in the catalog");
    assert_eq!(connector.flow(), v1::ConnectorFlowKind::Paste);
    assert!(connector.redirect_uris.is_empty());
    assert!(
        connector.token_url.starts_with("https://dash.cloudflare.com/?to=/:account/api-tokens&permissionGroupKeys="),
        "{}",
        connector.token_url
    );
    let keys: Value = serde_json::from_str(&query(&connector.token_url, "permissionGroupKeys")).expect("JSON");
    assert_eq!(keys[0], json!({"key": "account_api_tokens", "type": "edit"}));
    assert_eq!(query(&connector.token_url, "name"), "Claktory");

    // Cloudflare's page to make the token; the flow waits for it.
    let start = Request::StartConnect(v1::StartConnect { connector_id: "cloudflare".into(), ..Default::default() });
    let Body::ConnectStarted(started) = apply_request(&mut client, &mut view, start).await else { panic!("not started") };
    assert_eq!(started.url, connector.token_url);
    let flow = |view: &View| vault(view).flows.into_iter().find(|flow| flow.id == started.flow_id).expect("the flow");
    assert_eq!(flow(&view).state(), ConnectFlowState::Waiting);
    assert!(flow(&view).expires_ms - factoryd::transcript::now_ms() > 20 * 60 * 1000, "time to make a token");

    let paste = |token: &str| {
        Request::FinishConnect(v1::FinishConnect { flow_id: started.flow_id.clone(), token: token.into(), ..Default::default() })
    };
    assert_eq!(failure(&apply_request(&mut client, &mut view, paste("  ")).await), "Paste the token to connect.");
    // Only the device that started it finishes it.
    let (mut other, _) = crate::remote::another_device(addr, "Phone").await;
    let elsewhere = other.request(paste(PASTED)).await.body;
    assert!(failure(&elsewhere).starts_with("This connection was started on another device."), "{elsewhere:?}");

    // A token Cloudflare doesn't take: the flow waits for another, saying why.
    ok(&mut client, &mut view, paste("cf-wrong-token")).await;
    view.follow(&mut client, |view| !flow(view).error.is_empty()).await;
    assert_eq!(flow(&view).state(), ConnectFlowState::Waiting);
    assert_eq!(flow(&view).error, "Cloudflare didn't accept that token. Check that you copied all of it, or make a new one.");

    ok(&mut client, &mut view, paste(&format!(" {PASTED}\n"))).await;
    view.follow(&mut client, |view| flow(view).state() == ConnectFlowState::Connected).await;
    assert_eq!(flow(&view).error, "");
    let connection = vault(&view).secrets.into_iter().find(|secret| secret.connection.is_some()).expect("a connection");
    let info = connection.connection.clone().expect("a connection");
    assert_eq!((info.account.as_str(), info.status()), ("Acme", ConnectionStatus::Ready));
    assert_eq!(info.access, ["Account API Tokens Write", "Zone Read"], "what the token may do, as Cloudflare says");
    assert!(connection.variables.is_empty() && connection.grants.is_empty(), "given to no one");

    // A narrow token, minted with the pasted one.
    let derive = Request::DeriveSecret(v1::DeriveSecret {
        secret_id: connection.id.clone(),
        name: "Cloudflare DNS".into(),
        description: String::new(),
        permissions: vec!["dns-write".into()],
        resources: vec!["acc".into(), "z1".into()],
        expires_ms: 0,
    });
    ok(&mut client, &mut view, derive).await;
    let made = secret(&view, "Cloudflare DNS");
    assert_eq!(made.export.expect("made from it").access, ["DNS Write on claktory.com"]);
    assert_eq!(made.description, "Cloudflare: DNS Write on claktory.com", "it says what it grants");
    assert_eq!(cloud.lock().expect("lock").tokens.len(), 1);
    assert!(
        stand_in.requests().iter().any(|call| call.method == "POST" && call.header("authorization") == Some(&format!("Bearer {PASTED}"))),
        "minted with the pasted token"
    );

    // Disconnected and revoked: what it made, and the pasted token itself, deleted at Cloudflare.
    ok(&mut client, &mut view, Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: true })).await;
    view.follow(&mut client, |view| vault(view).secrets.is_empty()).await;
    let cloud = cloud.lock().expect("lock");
    assert!(cloud.tokens.is_empty() && cloud.pasted.is_none(), "{:?}", cloud.tokens.keys());
}
