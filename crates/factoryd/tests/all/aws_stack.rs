//! AWS through a CloudFormation stack, against a stand-in for STS and IAM:
//! an agent proposes exact statements, connecting opens AWS's quick-create
//! page with them, the receipt the template's own function seals (run with
//! node; skipped where node isn't installed) is pasted and checked with
//! "AWS", a stack that allows more is refused, and secrets made from the
//! connection are sessions narrowed to what's asked and no more.

use std::sync::{Arc, Mutex};

use factory_protocol::v1::{self, ConnectFlowState, client_message::Body as Request, entry::Kind, server_message::Body};
use serde_json::{Value, json};

use crate::common::stand_in::{Request as Got, StandIn};
use crate::common::{
    Client, MODEL, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir,
};

const ACCOUNT: &str = "123456789012";
const USER: &str = "arn:aws:iam::123456789012:user/claktory/Claktory-c1-BrokerUser-ABC";
const ROLE: &str = "arn:aws:iam::123456789012:role/claktory/Claktory-c1-CapabilityRole-DEF";
const STACK: &str = "arn:aws:cloudformation:eu-central-1:123456789012:stack/Claktory-c1-ab12/0a1b2c3d";
const BROKER_KEY: &str = "AKIATEST00000001";
const BROKER_SECRET: &str = "broker-secret-0123456789abcdef";

/// What the stand-in account holds.
#[derive(Default)]
struct Account {
    /// The capability role's policy, as IAM keeps it.
    role_policy: String,
    /// The user is also in a group.
    in_group: bool,
    sessions: usize,
}

fn encode(text: &str) -> String {
    text.bytes()
        .map(
            |byte| {
                if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) { char::from(byte).to_string() } else { format!("%{byte:02X}") }
            },
        )
        .collect()
}

fn xml(action: &str, result: &str) -> Value {
    Value::String(format!("<{action}Response><{action}Result>{result}</{action}Result></{action}Response>"))
}

fn answer(account: &mut Account, got: &Got) -> (u16, Value) {
    let authorization = got.header("authorization").unwrap_or_default().to_owned();
    assert!(authorization.starts_with("AWS4-HMAC-SHA256 Credential="), "signed: {authorization}");
    let action = got.form("Action").unwrap_or_default();
    let signed_by_broker = authorization.contains(&format!("Credential={BROKER_KEY}/"));
    if !signed_by_broker {
        return (403, Value::String("<ErrorResponse><Error><Code>InvalidClientTokenId</Code></Error></ErrorResponse>".into()));
    }
    let broker = json!({"Version": "2012-10-17", "Statement": [
        {"Effect": "Allow", "Action": "sts:AssumeRole", "Resource": ROLE},
        {"Effect": "Allow", "Action": ["iam:GetUser", "iam:ListUserPolicies", "iam:GetUserPolicy", "iam:ListAttachedUserPolicies", "iam:ListGroupsForUser"], "Resource": USER},
        {"Effect": "Allow", "Action": ["iam:GetRole", "iam:ListRolePolicies", "iam:GetRolePolicy", "iam:ListAttachedRolePolicies"], "Resource": ROLE},
    ]});
    let trust =
        json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Principal": {"AWS": USER}, "Action": "sts:AssumeRole"}]});
    match action.as_str() {
        "GetCallerIdentity" => (200, xml(&action, &format!("<Arn>{USER}</Arn><UserId>AIDA1</UserId><Account>{ACCOUNT}</Account>"))),
        "ListUserPolicies" => {
            (200, xml(&action, "<PolicyNames><member>claktory-broker</member></PolicyNames><IsTruncated>false</IsTruncated>"))
        }
        "GetUserPolicy" => (200, xml(&action, &format!("<PolicyDocument>{}</PolicyDocument>", encode(&broker.to_string())))),
        "ListAttachedUserPolicies" | "ListAttachedRolePolicies" => {
            (200, xml(&action, "<AttachedPolicies/><IsTruncated>false</IsTruncated>"))
        }
        "ListGroupsForUser" => {
            let groups = if account.in_group { "<Groups><member><GroupName>Admins</GroupName></member></Groups>" } else { "<Groups/>" };
            (200, xml(&action, &format!("{groups}<IsTruncated>false</IsTruncated>")))
        }
        "GetRole" => (
            200,
            xml(
                &action,
                &format!(
                    "<Role><Arn>{ROLE}</Arn><AssumeRolePolicyDocument>{}</AssumeRolePolicyDocument></Role>",
                    encode(&trust.to_string())
                ),
            ),
        ),
        "ListRolePolicies" => {
            (200, xml(&action, "<PolicyNames><member>claktory-capability</member></PolicyNames><IsTruncated>false</IsTruncated>"))
        }
        "GetRolePolicy" => (200, xml(&action, &format!("<PolicyDocument>{}</PolicyDocument>", encode(&account.role_policy)))),
        "AssumeRole" => {
            assert_eq!(got.form("RoleArn").as_deref(), Some(ROLE));
            assert_eq!(got.form("DurationSeconds").as_deref(), Some("900"));
            account.sessions += 1;
            let n = account.sessions;
            let expiration = jiff::Timestamp::now() + jiff::SignedDuration::from_mins(15);
            (
                200,
                xml(
                    &action,
                    &format!(
                        "<Credentials><AccessKeyId>ASIASESSION{n:05}</AccessKeyId><SecretAccessKey>session-secret-{n}-abcdefghij</SecretAccessKey>\
                         <SessionToken>session-token-{n}-abcdefghij</SessionToken><Expiration>{expiration}</Expiration></Credentials>"
                    ),
                ),
            )
        }
        other => panic!("unexpected {other}"),
    }
}

/// The role policy the template writes for `statements`, Action and
/// Resource always lists.
fn role_policy(statements: &Value) -> String {
    let statement: Vec<Value> = statements
        .as_array()
        .expect("statements")
        .iter()
        .enumerate()
        .map(|(index, statement)| {
            let mut written = json!({"Sid": format!("Statement{}", index + 1), "Effect": "Allow", "Action": statement["actions"], "Resource": statement["resources"]});
            if let Some(condition) = statement["condition"].as_str().filter(|text| !text.is_empty()) {
                written["Condition"] = serde_json::from_str(condition).expect("condition");
            }
            written
        })
        .collect();
    json!({"Version": "2012-10-17", "Statement": statement}).to_string()
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

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

async fn ok(client: &mut Client, view: &mut View, body: Request) {
    let reply = apply_request(client, view, body).await;
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
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

/// The quick-create page's parameters (after the #…?).
fn parameters(url: &str) -> Vec<(String, String)> {
    let query = url.split_once("#/stacks/create/review?").expect("a quick-create page").1;
    reqwest::Url::parse(&format!("http://x/?{query}"))
        .expect("query")
        .query_pairs()
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect()
}

fn parameter<'a>(parameters: &'a [(String, String)], name: &str) -> &'a str {
    parameters.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str()).unwrap_or_else(|| panic!("no {name}"))
}

/// A receipt sealed by the template's own function, or None without node.
fn seal(jwk: &str, payload: &Value) -> Option<String> {
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("aws/seal-fixture.js");
    let output = match std::process::Command::new("node").arg(script).arg(jwk).arg(payload.to_string()).output() {
        Ok(output) => output,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipped: node isn't installed");
            return None;
        }
        Err(error) => panic!("node: {error}"),
    };
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    Some(String::from_utf8(output.stdout).expect("utf-8").trim().to_owned())
}

#[tokio::test]
async fn aws_connects_with_the_proposals_exact_statements_and_gives_narrowed_sessions() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    factory.set_vault_passphrase_cost("argon2id m=1024 t=1 p=1");
    let account = Arc::new(Mutex::new(Account::default()));
    let stand_in = StandIn::start({
        let account = account.clone();
        move |got| answer(&mut account.lock().expect("lock"), got)
    })
    .await;
    let template = "https://claktory-templates.s3.amazonaws.com/v1/claktory-aws-v1.yaml";
    factory.add_connector(
        Arc::new(factoryd::connectors::aws_stack::AwsStack::with_template(template)),
        &[("aws.sts", &stand_in.base), ("aws.iam", &stand_in.base)],
    );
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    for agent in ["builder", "rivet"] {
        let model = v1::UpdateAgent { agent_id: agent.into(), model: Some(MODEL.into()), effort: Some("off".into()), ..Default::default() };
        apply_request(&mut client, &mut view, Request::UpdateAgent(model)).await;
    }
    let value = v1::OptionValue { on: true, agent_ids: Vec::new() };
    let change = v1::ExtensionChange { id: "secrets".into(), option_id: "connect".into(), value: Some(value), ..Default::default() };
    apply_request(
        &mut client,
        &mut view,
        Request::UpdateAgent(v1::UpdateAgent { agent_id: "builder".into(), extension: Some(change), ..Default::default() }),
    )
    .await;

    // The agent proposes exact statements, in a region.
    let statements = json!([
        {"actions": ["ec2:DescribeInstances", "ec2:DescribeImages"], "resources": ["*"], "condition": r#"{"StringEquals":{"aws:RequestedRegion":"eu-central-1"}}"#},
        {"actions": ["ec2:RunInstances"], "resources": ["arn:aws:ec2:eu-central-1:*:instance/*"], "condition": r#"{"StringEqualsIfExists":{"ec2:InstanceType":["t3.small","t3a.small"]}}"#},
    ]);
    let mut args = json!({"service": "aws", "purpose": "Run the account server", "access": "Launch one small instance", "reason": "It needs a server."});
    let without = call(&mut client, &mut view, "builder", "secrets_propose_connection", &args.to_string()).await;
    assert!(without.starts_with("Give the region the work is in"), "{without}");
    args["region"] = json!("eu-central-1");
    args["statements"] = json!([{"actions": ["*"], "resources": ["*"]}]);
    let broad = call(&mut client, &mut view, "builder", "secrets_propose_connection", &args.to_string()).await;
    assert!(broad.contains("isn't an IAM action"), "{broad}");
    args["statements"] = statements.clone();
    let said = call(&mut client, &mut view, "builder", "secrets_propose_connection", &args.to_string()).await;
    assert!(said.starts_with("Proposed connecting AWS;"), "{said}");
    let proposal = vault(&view).proposals[0].clone();
    assert_eq!((proposal.region.as_str(), proposal.statements.len()), ("eu-central-1", 2));
    assert_eq!(proposal.statements[1].condition, r#"{"StringEqualsIfExists":{"ec2:InstanceType":["t3.small","t3a.small"]}}"#);
    let shown = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::ConnectProposal(shown)) => Some(shown.clone()),
        _ => None,
    });
    assert_eq!(shown.expect("the card").statements, proposal.statements, "the card shows them too");

    // Without a proposal there's nothing to allow; with it, AWS's page.
    let start = |proposal_id: &str| {
        Request::StartConnect(v1::StartConnect { connector_id: "aws".into(), proposal_id: proposal_id.into(), ..Default::default() })
    };
    let alone = apply_request(&mut client, &mut view, start("")).await;
    assert!(failure(&alone).starts_with("AWS is connected for an agent's proposal"), "{alone:?}");
    let Body::ConnectStarted(started) = apply_request(&mut client, &mut view, start(&proposal.id)).await else { panic!("not started") };
    assert!(
        started
            .url
            .starts_with("https://eu-central-1.console.aws.amazon.com/cloudformation/home?region=eu-central-1#/stacks/create/review?")
    );
    let given = parameters(&started.url);
    assert_eq!(parameter(&given, "templateURL"), template);
    assert!(parameter(&given, "stackName").starts_with(&format!("Claktory-{}-", proposal.id)));
    assert_eq!(parameter(&given, "param_Actions1"), "ec2:DescribeInstances,ec2:DescribeImages");
    assert_eq!(parameter(&given, "param_Resources2"), "arn:aws:ec2:eu-central-1:*:instance/*");
    assert_eq!(parameter(&given, "param_Condition1"), r#"{"StringEquals":{"aws:RequestedRegion":"eu-central-1"}}"#);
    assert!(!given.iter().any(|(key, _)| key == "param_Condition3" || key == "param_Actions3"), "unused slots are left out");
    let flow_id = parameter(&given, "param_FlowId").to_owned();
    let jwk: Value = serde_json::from_str(parameter(&given, "param_PublicKeyJwk")).expect("jwk");
    assert_eq!(
        (jwk["kty"].as_str(), jwk["kid"].as_str(), jwk["alg"].as_str()),
        (Some("RSA"), Some(flow_id.as_str()), Some("RSA-OAEP-256"))
    );

    let paste = |token: &str| {
        Request::FinishConnect(v1::FinishConnect { flow_id: started.flow_id.clone(), callback_url: String::new(), token: token.into() })
    };
    // Not a receipt: said, and the flow waits for another.
    ok(&mut client, &mut view, paste("AKIAIOSFODNN7EXAMPLE")).await;
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.error.contains("isn't a Claktory receipt"))).await;

    let payload = json!({
        "v": 1, "flow": flow_id, "proposal": parameter(&given, "param_ProposalHash"), "account": ACCOUNT, "region": "eu-central-1",
        "stack": STACK, "user_arn": USER, "role_arn": ROLE, "access_key_id": BROKER_KEY, "secret_access_key": BROKER_SECRET,
        "issued_at": "2026-05-01T12:00:00Z",
    });
    let Some(receipt) = seal(&jwk.to_string(), &payload) else { return };
    assert!(receipt.starts_with("claktory-aws-v1."));

    // A stack whose role allows more, or whose user is in a group, is refused.
    account.lock().expect("lock").role_policy = role_policy(&json!([{"actions": ["ec2:*"], "resources": ["*"]}]));
    ok(&mut client, &mut view, paste(&receipt)).await;
    view.follow(&mut client, |view| {
        flow(view, &started.flow_id)
            .is_some_and(|flow| !flow.error.is_empty() && flow.state() == ConnectFlowState::Waiting && !flow.error.contains("receipt"))
    })
    .await;
    assert_eq!(
        flow(&view, &started.flow_id).expect("flow").error,
        "Claktory didn't keep this connection: its role's permissions aren't the ones proposed. Delete the stack Claktory-c1-ab12 in AWS CloudFormation, then connect again."
    );
    account.lock().expect("lock").role_policy = role_policy(&statements);
    account.lock().expect("lock").in_group = true;
    ok(&mut client, &mut view, paste(&receipt)).await;
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.error.contains("in the group Admins"))).await;

    // Exactly what was proposed: connected.
    account.lock().expect("lock").in_group = false;
    ok(&mut client, &mut view, paste(&receipt)).await;
    view.follow(&mut client, |view| flow(view, &started.flow_id).is_some_and(|flow| flow.state() == ConnectFlowState::Connected)).await;
    let connection = vault(&view).secrets.into_iter().find(|secret| secret.connection.is_some()).expect("a connection");
    let info = connection.connection.clone().expect("info");
    assert_eq!(connection.name, "AWS (123456789012 in eu-central-1)");
    assert_eq!(info.account, "123456789012 in eu-central-1");
    assert_eq!(info.access.len(), 2);
    assert!(connection.description.starts_with("AWS: ec2:DescribeInstances, ec2:DescribeImages on * when "), "{}", connection.description);
    assert!(info.access[1].starts_with("ec2:RunInstances on arn:aws:ec2:eu-central-1:*:instance/* when "), "{:?}", info.access);
    for file in ["home/vault/vault.db", "home/factory.db", "home/config.json"] {
        let bytes = std::fs::read(dir.join(file)).unwrap_or_default();
        assert!(!bytes.windows(BROKER_SECRET.len()).any(|window| window == BROKER_SECRET.as_bytes()), "{file} holds the key");
    }

    // Rivet is told the exact statements.
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    view.follow(&mut client, |view| describe(&view.entries).iter().any(|line| line.contains("for Builder (builder)'s proposal"))).await;
    let told = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.contains("Exact permissions asked") => Some(event.text.clone()),
        _ => None,
    });
    let told = told.expect("Rivet is told the statements");
    assert!(
        told.contains("- Exact permissions asked, in eu-central-1:\n  - ec2:DescribeInstances, ec2:DescribeImages on * when {"),
        "{told}"
    );

    // Secrets: only what the connection allows, as a narrowed session.
    let choices =
        apply_request(&mut client, &mut view, Request::ListConnectChoices(v1::ListConnectChoices { secret_id: connection.id.clone() }))
            .await;
    let Body::ConnectChoices(choices) = choices else { panic!("expected choices, got {choices:?}") };
    assert!(choices.choices.iter().any(|choice| choice.kind == "permission" && choice.id == "ec2:RunInstances"));
    let derive = |permissions: &[&str]| {
        Request::DeriveSecret(v1::DeriveSecret {
            secret_id: connection.id.clone(),
            name: "AWS describe".into(),
            permissions: permissions.iter().map(|text| (*text).to_owned()).collect(),
            ..Default::default()
        })
    };
    assert_eq!(
        failure(&apply_request(&mut client, &mut view, derive(&["s3:GetObject"])).await),
        "s3:GetObject isn't one of this connection's permissions."
    );
    ok(&mut client, &mut view, derive(&["ec2:DescribeInstances"])).await;
    let assumed = stand_in.requests().into_iter().rev().find(|got| got.form("Action").as_deref() == Some("AssumeRole")).expect("a session");
    let policy: Value = serde_json::from_str(&assumed.form("Policy").expect("a session policy")).expect("json");
    assert_eq!(policy["Statement"][0]["Action"], json!(["ec2:DescribeInstances"]));
    let made = vault(&view).secrets.into_iter().find(|secret| secret.name == "AWS describe").expect("made");
    let export = made.export.clone().expect("export");
    assert_eq!(export.permissions, ["ec2:DescribeInstances"]);
    assert_eq!(made.description, "AWS: ec2:DescribeInstances on * in AWS account 123456789012 in eu-central-1");
    apply_request(
        &mut client,
        &mut view,
        Request::GrantSecret(v1::GrantSecret { id: made.id.clone(), agent_id: "builder".into(), proof: None }),
    )
    .await;
    let output =
        call(&mut client, &mut view, "builder", "bash", r#"{"command":"echo $AWS_REGION $AWS_ACCESS_KEY_ID","secrets":["AWS describe"]}"#)
            .await;
    assert_eq!(output.lines().next(), Some("eu-central-1 [secret AWS describe/AWS_ACCESS_KEY_ID]"), "{output}");

    // Claktory can't delete the stack: cancelling says where to.
    let revoked = apply_request(
        &mut client,
        &mut view,
        Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: true }),
    )
    .await;
    assert!(
        failure(&revoked).contains("Delete Claktory-c1-ab12 in AWS CloudFormation: https://eu-central-1.console.aws.amazon.com/"),
        "{revoked:?}"
    );
    ok(&mut client, &mut view, Request::DisconnectService(v1::DisconnectService { secret_id: connection.id.clone(), revoke: false })).await;
    view.follow(&mut client, |view| vault(view).secrets.is_empty()).await;
}
