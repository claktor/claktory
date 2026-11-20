//! The AWS connector (IAM Identity Center) against a stand-in for its OIDC
//! service and access portal: connecting with the device flow, the roles to
//! pick from, a secret made of one role, its credentials kept until shortly
//! before they expire and then renewed, the session refreshed, and connected
//! again once AWS stops accepting it.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use factory_protocol::v1::{self, ConnectFlowState, ConnectionStatus, client_message::Body as Request, entry::Kind, server_message::Body};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};

/// One request the stand-in got.
#[derive(Clone, Debug)]
struct Got {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    token: String,
    body: Value,
}

impl Got {
    fn query(&self, name: &str) -> &str {
        self.query.iter().find(|(key, _)| key == name).map_or("", |(_, value)| value.as_str())
    }
}

/// IAM Identity Center as Claktory sees it: one portal, two accounts (the
/// second on a second page), roles in each, and what the test changes.
#[derive(Default)]
struct IdentityCenter {
    got: Mutex<Vec<Got>>,
    registrations: AtomicUsize,
    polls: AtomicUsize,
    /// Access tokens handed out; only the latest is accepted.
    tokens: AtomicUsize,
    /// How long access tokens last, in seconds.
    token_seconds: AtomicI64,
    /// The refresh grant is gone.
    dead: AtomicBool,
    /// Role credentials handed out, and how long they last, in ms.
    minted: AtomicUsize,
    credential_ms: AtomicI64,
    logged_out: AtomicBool,
}

fn now_ms() -> i64 {
    i64::try_from(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis()).expect("ms")
}

impl IdentityCenter {
    fn answer(&self, got: &Got) -> (u16, Value) {
        let current = format!("access-{}", self.tokens.load(Ordering::SeqCst));
        let tokens = || {
            let n = self.tokens.fetch_add(1, Ordering::SeqCst) + 1;
            json!({"accessToken": format!("access-{n}"), "refreshToken": format!("refresh-{n}"), "expiresIn": self.token_seconds.load(Ordering::SeqCst), "tokenType": "Bearer"})
        };
        let client = format!("client-{}", self.registrations.load(Ordering::SeqCst));
        match (got.method.as_str(), got.path.as_str()) {
            ("POST", "/client/register") => {
                assert_eq!((got.body["clientType"].as_str(), &got.body["scopes"]), (Some("public"), &json!(["sso:account:access"])));
                assert!(got.body["grantTypes"].to_string().contains("refresh_token"), "asks for refresh tokens");
                let n = self.registrations.fetch_add(1, Ordering::SeqCst) + 1;
                let expires = now_ms() / 1000 + 90 * 86400;
                (
                    200,
                    json!({"clientId": format!("client-{n}"), "clientSecret": format!("client-secret-{n}-abcdefgh"), "clientIdIssuedAt": 1, "clientSecretExpiresAt": expires}),
                )
            }
            ("POST", "/device_authorization") => {
                assert_eq!(
                    (got.body["clientId"].as_str(), got.body["startUrl"].as_str()),
                    (Some(client.as_str()), Some("https://d-1234567890.awsapps.com/start"))
                );
                (
                    200,
                    json!({"deviceCode": "device-1", "userCode": "ABCD-EFGH", "verificationUri": "https://device.sso.us-east-1.amazonaws.com/",
                    "verificationUriComplete": "https://device.sso.us-east-1.amazonaws.com/?user_code=ABCD-EFGH", "interval": 1, "expiresIn": 600}),
                )
            }
            ("POST", "/token") if got.body["grantType"] == "refresh_token" => {
                assert_eq!(got.body["clientId"].as_str(), Some(client.as_str()));
                if self.dead.load(Ordering::SeqCst) {
                    return (400, json!({"error": "invalid_grant", "error_description": "Refresh token has expired"}));
                }
                assert_eq!(got.body["refreshToken"].as_str(), Some(current.replace("access", "refresh").as_str()));
                (200, tokens())
            }
            ("POST", "/token") => {
                assert_eq!((got.body["deviceCode"].as_str(), got.body["clientId"].as_str()), (Some("device-1"), Some(client.as_str())));
                // Approved on the second poll.
                match self.polls.fetch_add(1, Ordering::SeqCst) {
                    0 => (400, json!({"error": "authorization_pending"})),
                    _ => (200, tokens()),
                }
            }
            (_, _) if got.token != current => (401, json!({"message": "Session token not found or invalid"})),
            ("GET", "/assignment/accounts") => match got.query("next_token") {
                "" => (
                    200,
                    json!({"accountList": [{"accountId": "111122223333", "accountName": "prod", "emailAddress": "p@example.com"}], "nextToken": "page-2"}),
                ),
                _ => (
                    200,
                    json!({"accountList": [{"accountId": "444455556666", "accountName": "staging", "emailAddress": "s@example.com"}]}),
                ),
            },
            ("GET", "/assignment/roles") => match got.query("account_id") {
                "111122223333" => (
                    200,
                    json!({"roleList": [{"roleName": "ReadOnly", "accountId": "111122223333"}, {"roleName": "DeploySite", "accountId": "111122223333"}]}),
                ),
                _ => (200, json!({"roleList": [{"roleName": "Admin", "accountId": "444455556666"}]})),
            },
            ("GET", "/federation/credentials") => {
                assert_eq!((got.query("account_id"), got.query("role_name")), ("111122223333", "DeploySite"));
                let n = self.minted.fetch_add(1, Ordering::SeqCst) + 1;
                let expiration = now_ms() + self.credential_ms.load(Ordering::SeqCst);
                (
                    200,
                    json!({"roleCredentials": {"accessKeyId": format!("ASIAKEY{n}AAAAAAAAA"), "secretAccessKey": format!("secret-key-{n}-abcdefghij"),
                    "sessionToken": format!("session-token-{n}-abcdefghij"), "expiration": expiration}}),
                )
            }
            ("POST", "/logout") => {
                self.logged_out.store(true, Ordering::SeqCst);
                (200, json!({}))
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    fn count(&self, path: &str) -> usize {
        self.got.lock().expect("lock").iter().filter(|got| got.path == path).count()
    }

    fn refreshes(&self) -> usize {
        self.got.lock().expect("lock").iter().filter(|got| got.body["grantType"] == "refresh_token").count()
    }

    /// Serves it on a port of its own; its base URL.
    async fn serve(self: &Arc<Self>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let center = self.clone();
        tokio::spawn(async move {
            while let Ok((mut socket, _)) = listener.accept().await {
                let center = center.clone();
                tokio::spawn(async move {
                    let Some(got) = read(&mut socket).await else { return };
                    let (status, body) = center.answer(&got);
                    center.got.lock().expect("lock").push(got);
                    let body = body.to_string();
                    let head = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes()).await;
                    let _ = socket.write_all(body.as_bytes()).await;
                });
            }
        });
        base
    }
}

async fn read(socket: &mut tokio::net::TcpStream) -> Option<Got> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let head_end = loop {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(at) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break at + 4;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut first = lines.next()?.split(' ');
    let (method, target) = (first.next()?.to_owned(), first.next()?.to_owned());
    let headers: Vec<(String, String)> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(key, value)| (key.trim().to_ascii_lowercase(), value.trim().to_owned()))
        .collect();
    let header = |name: &str| headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()).unwrap_or_default();
    let length: usize = header("content-length").parse().unwrap_or(0);
    while buffer.len() < head_end + length {
        let read = socket.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let url = reqwest::Url::parse(&format!("http://stand-in{target}")).ok()?;
    Some(Got {
        method,
        path: url.path().to_owned(),
        query: url.query_pairs().into_owned().collect(),
        token: header("x-amz-sso_bearer_token"),
        body: serde_json::from_slice(&buffer[head_end..(head_end + length).min(buffer.len())]).unwrap_or_default(),
    })
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

fn flow(view: &View, id: &str) -> Option<v1::ConnectFlow> {
    vault(view).flows.into_iter().find(|flow| flow.id == id)
}

fn status(view: &View, id: &str) -> Option<ConnectionStatus> {
    vault(view).secrets.into_iter().find(|secret| secret.id == id).and_then(|secret| secret.connection).map(|info| info.status())
}

/// What `agent`'s bash command printed.
async fn bash(client: &mut Client, view: &mut View, agent: &str, args: &str) -> String {
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call bash {args}"))).await;
    view.follow(client, |view| {
        let agent = view.agent(agent);
        view.entries.len() >= before + 3 && agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
    })
    .await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.output.clone()),
            _ => None,
        })
        .expect("a tool call")
}

/// Connects (or, with `secret_id`, connects again) and waits until it is.
async fn connect(client: &mut Client, view: &mut View, secret_id: &str) {
    let fields = [("start_url", " d-1234567890 "), ("region", "us-east-1")].map(|(key, value)| (key.to_owned(), value.to_owned())).into();
    let start = Request::StartConnect(v1::StartConnect {
        connector_id: "aws-sso".into(),
        fields,
        secret_id: secret_id.into(),
        ..Default::default()
    });
    let started = client.request(start).await;
    let Body::ConnectStarted(started) = started.body else { panic!("expected a start, got {:?}", started.body) };
    assert_eq!(
        (started.url.as_str(), started.user_code.as_str()),
        ("https://device.sso.us-east-1.amazonaws.com/?user_code=ABCD-EFGH", "ABCD-EFGH"),
        "the page to approve it on, the code filled in"
    );
    view.follow(client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Connected)).await;
}

#[tokio::test]
async fn aws_connects_with_the_device_flow_and_a_role_s_credentials_are_kept_then_renewed() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let center = Arc::new(IdentityCenter::default());
    // Tokens that need refreshing at once; credentials due for renewal at once.
    center.token_seconds.store(60, Ordering::SeqCst);
    center.credential_ms.store(60_000, Ordering::SeqCst);
    let base = center.serve().await;
    // Not offered yet (built in only in the dev Claktory, when asked): added here.
    factory.add_connector(Arc::new(factoryd::connectors::aws::Aws::new()), &[("aws.oidc", &base), ("aws.portal", &base)]);
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let aws = vault(&view).connectors.into_iter().find(|connector| connector.id == "aws-sso").expect("AWS is offered");
    assert_eq!(aws.flow(), v1::ConnectorFlowKind::Device);
    assert_eq!(aws.fields.iter().map(|field| field.id.as_str()).collect::<Vec<_>>(), ["start_url", "region"]);
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;

    // A region that isn't one is refused before anything is sent.
    let fields =
        [("start_url", "d-1234567890"), ("region", "evil.example/x")].map(|(key, value)| (key.to_owned(), value.to_owned())).into();
    let bad =
        client.request(Request::StartConnect(v1::StartConnect { connector_id: "aws-sso".into(), fields, ..Default::default() })).await;
    assert_eq!(failure(&bad.body), "\"evil.example/x\" isn't an AWS region, such as us-east-1.");

    // Connected by the device flow: registered, approved on the second poll.
    connect(&mut client, &mut view, "").await;
    assert_eq!((center.registrations.load(Ordering::SeqCst), center.polls.load(Ordering::SeqCst)), (1, 2));
    let connection = vault(&view).secrets.into_iter().find(|secret| secret.connection.is_some()).expect("a connection");
    assert_eq!(connection.name, "AWS IAM Identity Center (d-1234567890 in us-east-1)");
    let info = connection.connection.clone().expect("connection");
    assert_eq!(
        info.resources,
        [
            "AWS account prod (111122223333) · ReadOnly",
            "AWS account prod (111122223333) · DeploySite",
            "AWS account staging (444455556666) · Admin"
        ],
        "every account, on every page, with its roles"
    );
    for file in ["home/vault/vault.db", "home/factory.db", "home/config.json"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        assert!(!bytes.windows(9).any(|window| window == b"refresh-1"), "{file} holds a token");
    }

    // The choices: accounts, and roles under them. The token was refreshed first.
    let choices = client.request(Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() })).await;
    let Body::ConnectChoices(choices) = choices.body else { panic!("expected choices, got {:?}", choices.body) };
    assert_eq!(center.refreshes(), 1, "refreshed before use");
    let roles: Vec<(&str, &str, &str)> = choices
        .choices
        .iter()
        .filter(|choice| choice.kind == "role")
        .map(|choice| (choice.id.as_str(), choice.label.as_str(), choice.parent.as_str()))
        .collect();
    assert_eq!(
        roles,
        [
            ("111122223333/ReadOnly", "ReadOnly", "111122223333"),
            ("111122223333/DeploySite", "DeploySite", "111122223333"),
            ("444455556666/Admin", "Admin", "444455556666")
        ]
    );
    assert!(choices.choices.iter().any(|choice| choice.kind == "account" && choice.label == "prod (111122223333)"));
    assert!(choices.rules.starts_with("Choose one role."), "{}", choices.rules);

    // Made of exactly one role, which must be the person's.
    let derive = |permissions: &[&str], resources: &[&str]| {
        Request::DeriveSecret(v1::DeriveSecret {
            secret_id: connection.id.clone(),
            name: "AWS deploy".into(),
            permissions: permissions.iter().map(|text| (*text).to_owned()).collect(),
            resources: resources.iter().map(|text| (*text).to_owned()).collect(),
            ..Default::default()
        })
    };
    assert_eq!(
        failure(&client.request(derive(&[], &["111122223333/ReadOnly", "444455556666/Admin"])).await.body),
        "Choose exactly one role."
    );
    assert!(
        failure(&client.request(derive(&["s3:*"], &["111122223333/ReadOnly"])).await.body).starts_with("AWS has no permissions to choose")
    );
    assert_eq!(
        failure(&client.request(derive(&[], &["111122223333/Admin"])).await.body),
        "You have no role Admin in AWS account prod (111122223333)."
    );
    let events = client.ok(derive(&[], &["111122223333/DeploySite"])).await;
    let made = crate::common::last_workspace(&events)
        .and_then(|workspace| workspace.vault.clone())
        .and_then(|vault| vault.secrets.into_iter().find(|secret| secret.name == "AWS deploy"))
        .expect("the secret made");
    let export = made.export.clone().expect("export");
    assert!(export.per_use && export.expires_ms > now_ms(), "{export:?}");
    assert_eq!(export.access, ["Act as role DeploySite in AWS account prod (111122223333)"]);
    let mut names = made.variables.iter().map(|variable| variable.name.as_str()).collect::<Vec<_>>();
    names.sort_unstable();
    assert_eq!(names, ["AWS_ACCESS_KEY_ID", "AWS_DEFAULT_REGION", "AWS_REGION", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN"]);
    assert_eq!(center.count("/federation/credentials"), 1);
    let granted =
        client.request(Request::GrantSecret(v1::GrantSecret { id: made.id.clone(), agent_id: "builder".into(), proof: None })).await;
    assert!(!matches!(granted.body, Body::Error(_)), "{:?}", granted.body);

    // Due (they expire within 2 minutes): renewed before the command gets them.
    let command = r#"{"command":"echo $AWS_ACCESS_KEY_ID in $AWS_REGION; echo $AWS_ACCESS_KEY_ID | tr A-Z a-z","secrets":["AWS deploy"]}"#;
    center.credential_ms.store(3_600_000, Ordering::SeqCst);
    let output = bash(&mut client, &mut view, "builder", command).await;
    assert!(output.contains("[secret AWS deploy/AWS_ACCESS_KEY_ID] in us-east-1") && output.contains("asiakey2aaaaaaaaa"), "{output}");
    assert_eq!(center.count("/federation/credentials"), 2);
    let renewed = vault(&view).secrets.into_iter().find(|secret| secret.id == made.id).expect("still there");
    assert_eq!(renewed.version, 2);

    // Good for an hour: kept, not fetched again.
    let output = bash(&mut client, &mut view, "builder", command).await;
    assert!(output.contains("asiakey2aaaaaaaaa"), "{output}");
    assert_eq!(center.count("/federation/credentials"), 2, "kept until 2 minutes before they expire");

    // AWS stops accepting the session: it needs connecting again, and says so.
    center.dead.store(true, Ordering::SeqCst);
    let choices =
        apply_request(&mut client, &mut view, Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() }))
            .await;
    assert_eq!(
        failure(&choices),
        "AWS IAM Identity Center (d-1234567890 in us-east-1) needs to be connected again. AWS no longer accepts its sign-in."
    );
    view.follow(&mut client, |view| status(view, &connection.id) == Some(ConnectionStatus::NeedsReconnect)).await;

    // Connected again in place, with the client registered before: the secret made stays.
    center.dead.store(false, Ordering::SeqCst);
    center.polls.store(0, Ordering::SeqCst);
    center.token_seconds.store(28_800, Ordering::SeqCst);
    connect(&mut client, &mut view, &connection.id).await;
    assert_eq!(center.registrations.load(Ordering::SeqCst), 1, "the client registered before, reused");
    view.follow(&mut client, |view| status(view, &connection.id) == Some(ConnectionStatus::Ready)).await;
    assert_eq!(vault(&view).secrets.len(), 2, "the connection and the secret made from it");
    let refreshes = center.refreshes();
    let choices = client.request(Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() })).await;
    assert!(matches!(choices.body, Body::ConnectChoices(_)), "{:?}", choices.body);
    assert_eq!(center.refreshes(), refreshes, "a token good for hours isn't refreshed");

    // Disconnected and revoked: signed out at AWS, and everything goes.
    let disconnect = Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: true });
    assert!(matches!(apply_request(&mut client, &mut view, disconnect).await, Body::Ack(_)));
    view.follow(&mut client, |view| vault(view).secrets.is_empty()).await;
    assert!(center.logged_out.load(Ordering::SeqCst));
}
