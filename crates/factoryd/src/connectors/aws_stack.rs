//! AWS through a CloudFormation stack the person creates in their own
//! account (docs/CONNECTORS.md), with nothing of AWS's sign-in reaching
//! Claktory. An agent proposes the exact Allow statements the work needs;
//! connecting opens AWS's quick-create page for Claktory's template with
//! them filled in; the stack makes a broker user that can only assume a
//! role holding exactly those statements, and hands its key back sealed to
//! a key the backend made for this flow alone (the receipt, which the person
//! pastes). The backend opens it, checks with AWS that the user and role are
//! what was proposed and nothing more, and keeps the key as the connection's
//! control. Secrets made from it are 15-minute sessions of the role.
//!
//! The template is `crates/factoryd/aws/claktory-aws-v1.yaml`; its
//! parameters and the receipt are the contract here.
//!
//! STS and IAM are called with SigV4 (`sign`), their answers are XML read
//! for a handful of leaf elements (`xml_values`). The receipt is a compact
//! JWE (RSA-OAEP-256 and A256GCM, nothing else), opened with aws-lc-rs.

use aws_lc_rs::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use aws_lc_rs::encoding::AsDer;
use aws_lc_rs::rsa::{KeySize, OAEP_SHA256_MGF1SHA256, OaepPrivateDecryptingKey, PrivateDecryptingKey};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use factory_protocol::v1;
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    AccessSpec, BoxFuture, Choice, Choices, Control, Derive, Derived, ExportMeta, FlowKind, Http, Inspected, Problem, Recipe, Spec, Start,
    Variable,
};
use crate::encoding::hex;

/// The connector's id.
pub const ID: &str = "aws";
/// What a receipt starts with.
pub const RECEIPT_PREFIX: &str = "claktory-aws-v1.";
/// The receipt's JWE `typ`.
const RECEIPT_TYPE: &str = "claktory-aws-v1";
/// The template's statement slots.
pub const MAX_STATEMENTS: usize = 10;
/// CloudFormation's limit on a parameter's value, in bytes.
const MAX_PARAMETER: usize = 4096;
/// IAM's limit on a role's inline policies, in characters (without spaces).
const MAX_ROLE_POLICY: usize = 10_240;
/// The one-time key's size: RSA-3072.
const KEY_BITS: usize = 3072;

// ---- Statements ----

/// One Allow statement, as proposed and as checked: actions, resources,
/// and the condition as canonical JSON ("" for none).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Statement {
    pub actions: Vec<String>,
    pub resources: Vec<String>,
    #[serde(default)]
    pub condition: String,
}

impl Statement {
    pub fn to_proto(&self) -> v1::PolicyStatement {
        v1::PolicyStatement { actions: self.actions.clone(), resources: self.resources.clone(), condition: self.condition.clone() }
    }

    /// What it is with every way of writing it the same, to compare: lists
    /// sorted, actions in lower case (IAM's are), condition values as lists.
    fn key(&self) -> (Vec<String>, Vec<String>, String) {
        let mut actions: Vec<String> = self.actions.iter().map(|action| action.to_ascii_lowercase()).collect();
        actions.sort();
        actions.dedup();
        let mut resources = self.resources.clone();
        resources.sort();
        resources.dedup();
        let condition = if self.condition.is_empty() {
            String::new()
        } else {
            serde_json::from_str::<Value>(&self.condition).map(|value| canonical(&condition_values(&value))).unwrap_or_default()
        };
        (actions, resources, condition)
    }

    /// In words, one line: "ec2:RunInstances, ec2:StartInstances on
    /// arn:aws:ec2:*:*:instance/* when {…}".
    pub fn describe(&self) -> String {
        let mut text = format!("{} on {}", self.actions.join(", "), self.resources.join(", "));
        if !self.condition.is_empty() {
            text.push_str(&format!(" when {}", self.condition));
        }
        text
    }
}

/// A condition with each key's value a sorted list of strings, as IAM
/// reads `"x"`, `["x"]` and `true` alike.
fn condition_values(condition: &Value) -> Value {
    let Value::Object(operators) = condition else { return condition.clone() };
    let mut out = serde_json::Map::new();
    for (operator, keys) in operators {
        let Value::Object(keys) = keys else {
            out.insert(operator.clone(), keys.clone());
            continue;
        };
        let mut read = serde_json::Map::new();
        for (key, value) in keys {
            let mut values: Vec<String> = match value {
                Value::Array(items) => items.iter().map(scalar).collect(),
                other => vec![scalar(other)],
            };
            values.sort();
            values.dedup();
            read.insert(key.clone(), json!(values));
        }
        out.insert(operator.clone(), Value::Object(read));
    }
    Value::Object(out)
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// JSON with every object's keys sorted and no spaces.
pub fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> =
                keys.iter().map(|key| format!("{}:{}", Value::String((*key).clone()), canonical(&map[*key]))).collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(items) => format!("[{}]", items.iter().map(canonical).collect::<Vec<_>>().join(",")),
        other => other.to_string(),
    }
}

/// An action as IAM writes it: "ec2:RunInstances", "ec2:Describe*".
fn valid_action(action: &str) -> bool {
    let Some((service, name)) = action.split_once(':') else { return false };
    !service.is_empty()
        && service.len() <= 64
        && service.chars().all(|char| char.is_ascii_lowercase() || char.is_ascii_digit() || char == '-')
        && !name.is_empty()
        && name.len() <= 128
        && name.chars().all(|char| char.is_ascii_alphanumeric() || char == '*' || char == '?')
}

/// A resource: an ARN, or "*" for actions that take none.
fn valid_resource(resource: &str) -> bool {
    (resource == "*" || resource.starts_with("arn:") && resource.split(':').count() >= 6)
        && resource.len() <= 2048
        && !resource.chars().any(|char| char == ',' || char.is_whitespace() || char.is_control())
}

/// The statements an agent proposes, checked: what the template's slots
/// take, or why not, in words for the agent.
pub fn read_statements(given: &[v1::PolicyStatement]) -> Result<Vec<Statement>, String> {
    if given.is_empty() {
        return Err("Give the statements the work needs: each with its actions, resources and, if any, condition.".to_owned());
    }
    if given.len() > MAX_STATEMENTS {
        return Err(format!("Give at most {MAX_STATEMENTS} statements: merge those with the same resources and condition."));
    }
    let mut statements = Vec::new();
    for (index, statement) in given.iter().enumerate() {
        let number = index + 1;
        let actions: Vec<String> = statement.actions.iter().map(|action| action.trim().to_owned()).collect();
        let resources: Vec<String> = statement.resources.iter().map(|resource| resource.trim().to_owned()).collect();
        if actions.is_empty() || resources.is_empty() {
            return Err(format!("Statement {number} needs both actions and resources."));
        }
        if let Some(bad) = actions.iter().find(|action| !valid_action(action)) {
            return Err(format!("Statement {number}: \"{bad}\" isn't an IAM action such as ec2:RunInstances (a lone * isn't allowed)."));
        }
        if let Some(bad) = resources.iter().find(|resource| !valid_resource(resource)) {
            return Err(format!("Statement {number}: \"{bad}\" isn't an ARN or *."));
        }
        let condition = match statement.condition.trim() {
            "" => String::new(),
            text => match serde_json::from_str::<Value>(text) {
                Ok(value @ Value::Object(_)) if value.as_object().is_some_and(|map| !map.is_empty()) => canonical(&value),
                _ => {
                    return Err(format!(
                        "Statement {number}: the condition is IAM's Condition block as a JSON object, such as {{\"StringEquals\":{{\"aws:RequestedRegion\":\"eu-central-1\"}}}}."
                    ));
                }
            },
        };
        for (what, value) in [("actions", actions.join(",")), ("resources", resources.join(",")), ("condition", condition.clone())] {
            if value.len() > MAX_PARAMETER {
                return Err(format!("Statement {number}: its {what} are over {MAX_PARAMETER} characters; split it in two."));
            }
        }
        statements.push(Statement { actions, resources, condition });
    }
    let policy = role_policy(&statements);
    if policy.len() > MAX_ROLE_POLICY {
        return Err(format!("Together the statements are over IAM's {MAX_ROLE_POLICY} characters for a role's policy."));
    }
    Ok(statements)
}

/// A proposed statement in words, one line.
pub(crate) fn describe(statement: &v1::PolicyStatement) -> String {
    Statement { actions: statement.actions.clone(), resources: statement.resources.clone(), condition: statement.condition.clone() }
        .describe()
}

/// A proposal's statements as the vault keeps them: JSON, '' for none.
pub(crate) fn store_statements(statements: &[v1::PolicyStatement]) -> String {
    if statements.is_empty() {
        return String::new();
    }
    let kept: Vec<Statement> = statements
        .iter()
        .map(|statement| Statement {
            actions: statement.actions.clone(),
            resources: statement.resources.clone(),
            condition: statement.condition.clone(),
        })
        .collect();
    serde_json::to_string(&kept).unwrap_or_default()
}

/// Reads what `store_statements` kept.
pub(crate) fn stored_statements(text: &str) -> Vec<v1::PolicyStatement> {
    serde_json::from_str::<Vec<Statement>>(text).unwrap_or_default().iter().map(Statement::to_proto).collect()
}

/// The capability role's policy as the template writes it.
fn role_policy(statements: &[Statement]) -> String {
    let statement: Vec<Value> = statements
        .iter()
        .enumerate()
        .map(|(index, statement)| {
            let mut value = json!({
                "Sid": format!("Statement{}", index + 1),
                "Effect": "Allow",
                "Action": statement.actions,
                "Resource": statement.resources,
            });
            if !statement.condition.is_empty() {
                value["Condition"] = serde_json::from_str(&statement.condition).unwrap_or_default();
            }
            value
        })
        .collect();
    canonical(&json!({"Version": "2012-10-17", "Statement": statement}))
}

/// The proposal's SHA-256, which the stack carries into the receipt.
pub fn proposal_hash(region: &str, statements: &[Statement]) -> String {
    let statements: Vec<Value> = statements
        .iter()
        .map(|statement| {
            let condition =
                if statement.condition.is_empty() { json!({}) } else { serde_json::from_str(&statement.condition).unwrap_or_default() };
            json!({"actions": statement.actions, "resources": statement.resources, "condition": condition})
        })
        .collect();
    hex(&Sha256::digest(canonical(&json!({"v": 1, "region": region, "statements": statements})).as_bytes()))
}

/// The template's parameters for `statements`: `ActionsN`, `ResourcesN`
/// and `ConditionN` for the slots used.
pub fn slot_parameters(statements: &[Statement]) -> Vec<(String, String)> {
    let mut parameters = Vec::new();
    for (index, statement) in statements.iter().enumerate() {
        let number = index + 1;
        parameters.push((format!("Actions{number}"), statement.actions.join(",")));
        parameters.push((format!("Resources{number}"), statement.resources.join(",")));
        if !statement.condition.is_empty() {
            parameters.push((format!("Condition{number}"), statement.condition.clone()));
        }
    }
    parameters
}

// ---- Policies as IAM keeps them ----

/// A policy document's Allow statements, or why it isn't only those.
fn policy_statements(document: &str) -> Result<Vec<Statement>, String> {
    let value: Value = serde_json::from_str(document).map_err(|_| "a policy that isn't JSON".to_owned())?;
    let list = match &value["Statement"] {
        Value::Array(items) => items.clone(),
        item @ Value::Object(_) => vec![item.clone()],
        _ => return Err("a policy with no statements".to_owned()),
    };
    let strings = |value: &Value| -> Option<Vec<String>> {
        match value {
            Value::String(text) => Some(vec![text.clone()]),
            Value::Array(items) => items.iter().map(|item| item.as_str().map(str::to_owned)).collect(),
            _ => None,
        }
    };
    let mut statements = Vec::new();
    for item in &list {
        let Value::Object(map) = item else { return Err("a statement that isn't an object".to_owned()) };
        if let Some(key) = map.keys().find(|key| !["Sid", "Effect", "Action", "Resource", "Condition"].contains(&key.as_str())) {
            return Err(format!("a statement with {key}"));
        }
        if map.get("Effect").and_then(Value::as_str) != Some("Allow") {
            return Err("a statement that isn't Allow".to_owned());
        }
        let (Some(actions), Some(resources)) = (map.get("Action").and_then(strings), map.get("Resource").and_then(strings)) else {
            return Err("a statement without actions or resources".to_owned());
        };
        let condition = match map.get("Condition") {
            None => String::new(),
            Some(Value::Object(condition)) if condition.is_empty() => String::new(),
            Some(condition @ Value::Object(_)) => canonical(condition),
            Some(_) => return Err("a statement whose condition isn't an object".to_owned()),
        };
        statements.push(Statement { actions, resources, condition });
    }
    Ok(statements)
}

/// Whether two lists of statements allow the same, written either way.
fn same_statements(found: &[Statement], expected: &[Statement]) -> bool {
    let mut found: Vec<_> = found.iter().map(Statement::key).collect();
    let mut expected: Vec<_> = expected.iter().map(Statement::key).collect();
    found.sort();
    expected.sort();
    found == expected
}

/// What the broker user may do: assume the role, and read its own and the
/// role's policies, nothing else. Some statement of its policies that
/// reaches further is why not.
fn broker_only(statements: &[Statement], user_arn: &str, role_arn: &str) -> Result<(), String> {
    let on_user = ["iam:getuser", "iam:listuserpolicies", "iam:getuserpolicy", "iam:listattacheduserpolicies", "iam:listgroupsforuser"];
    let on_role = ["iam:getrole", "iam:listrolepolicies", "iam:getrolepolicy", "iam:listattachedrolepolicies"];
    for statement in statements {
        for action in &statement.actions {
            let action = action.to_ascii_lowercase();
            for resource in &statement.resources {
                let allowed = (action == "sts:assumerole" && resource == role_arn)
                    || (on_user.contains(&action.as_str()) && resource == user_arn)
                    || (on_role.contains(&action.as_str()) && resource == role_arn);
                if !allowed {
                    return Err(format!("its user may also {action} on {resource}"));
                }
            }
        }
    }
    Ok(())
}

/// Whether a role's trust policy lets only `user_arn` assume it.
fn trusts_only(document: &str, user_arn: &str) -> Result<(), String> {
    let value: Value = serde_json::from_str(document).map_err(|_| "its role's trust policy isn't JSON".to_owned())?;
    let list = match &value["Statement"] {
        Value::Array(items) => items.clone(),
        item @ Value::Object(_) => vec![item.clone()],
        _ => return Err("its role's trust policy has no statements".to_owned()),
    };
    let only = |value: &Value, expected: &str| match value {
        Value::String(text) => text == expected,
        Value::Array(items) => !items.is_empty() && items.iter().all(|item| item.as_str() == Some(expected)),
        _ => false,
    };
    for statement in &list {
        if statement["Effect"].as_str() != Some("Allow") {
            continue;
        }
        let principal = &statement["Principal"];
        let by_user = principal.as_object().is_some_and(|map| map.len() == 1 && map.get("AWS").is_some_and(|aws| only(aws, user_arn)));
        if !by_user || !only(&statement["Action"], "sts:AssumeRole") {
            return Err("its role can be assumed by more than its user".to_owned());
        }
    }
    Ok(())
}

// ---- The receipt ----

/// The one-time key a flow's receipt is sealed to: its private half, kept
/// only in the flow, as PKCS#8.
pub struct FlowKey {
    pub kid: String,
    pub pkcs8: Zeroizing<Vec<u8>>,
}

/// A new key for the flow `kid`, and its public half as the template's
/// PublicKeyJwk.
pub fn flow_key(kid: &str) -> Result<(FlowKey, String), Problem> {
    let failed = || Problem::Refused("Claktory couldn't make a key for this connection.".to_owned());
    let private = PrivateDecryptingKey::generate(KeySize::Rsa3072).map_err(|_| failed())?;
    let pkcs8 = Zeroizing::new(AsDer::<aws_lc_rs::encoding::Pkcs8V1Der>::as_der(&private).map_err(|_| failed())?.as_ref().to_vec());
    let spki = AsDer::<aws_lc_rs::encoding::PublicKeyX509Der>::as_der(&private.public_key()).map_err(|_| failed())?;
    let (n, e) = rsa_numbers(spki.as_ref()).ok_or_else(failed)?;
    let jwk = canonical(&json!({
        "kty": "RSA",
        "n": URL_SAFE_NO_PAD.encode(n),
        "e": URL_SAFE_NO_PAD.encode(e),
        "alg": "RSA-OAEP-256",
        "use": "enc",
        "kid": kid,
    }));
    Ok((FlowKey { kid: kid.to_owned(), pkcs8 }, jwk))
}

/// One DER element at the start of `input`: its tag, contents and the rest.
fn der(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = input.split_first()?;
    let (&first, rest) = rest.split_first()?;
    let (length, rest) = if first < 0x80 {
        (usize::from(first), rest)
    } else {
        let count = usize::from(first & 0x7f);
        if count == 0 || count > 4 || rest.len() < count {
            return None;
        }
        let length = rest[..count].iter().fold(0usize, |length, &byte| length << 8 | usize::from(byte));
        (length, &rest[count..])
    };
    if rest.len() < length {
        return None;
    }
    Some((tag, &rest[..length], &rest[length..]))
}

/// The modulus and exponent of an RSA SubjectPublicKeyInfo, without
/// leading zeros.
fn rsa_numbers(spki: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let (0x30, info, _) = der(spki)? else { return None };
    let (0x30, _algorithm, rest) = der(info)? else { return None };
    let (0x03, bits, _) = der(rest)? else { return None };
    let (0, key) = bits.split_first()? else { return None };
    let (0x30, numbers, _) = der(key)? else { return None };
    let (0x02, n, rest) = der(numbers)? else { return None };
    let (0x02, e, _) = der(rest)? else { return None };
    let trim = |bytes: &[u8]| bytes.iter().skip_while(|&&byte| byte == 0).copied().collect::<Vec<u8>>();
    Some((trim(n), trim(e)))
}

/// What the receipt carries.
#[derive(Deserialize)]
pub struct Receipt {
    pub v: u32,
    pub flow: String,
    pub proposal: String,
    pub account: String,
    pub region: String,
    pub stack: String,
    pub user_arn: String,
    pub role_arn: String,
    pub access_key_id: String,
    pub secret_access_key: String,
    #[serde(default)]
    pub issued_at: String,
}

impl Drop for Receipt {
    fn drop(&mut self) {
        zeroize::Zeroize::zeroize(&mut self.secret_access_key);
    }
}

/// Opens a receipt pasted for the flow `flow` with its key: what it
/// carries, or why it isn't this flow's receipt. It's still only what the
/// stack says, to check with AWS.
pub fn open_receipt(pasted: &str, key: &FlowKey, flow: &str) -> Result<Receipt, Problem> {
    let not_it = || Problem::Refused("That isn't this connection's receipt. Copy the Receipt from your new stack's Outputs.".to_owned());
    let compact: String = pasted.chars().filter(|char| !char.is_whitespace()).collect();
    let Some(jwe) = compact.strip_prefix(RECEIPT_PREFIX) else {
        return Err(Problem::Refused(
            "That isn't a Claktory receipt. Copy the value of Receipt, starting claktory-aws-v1., from your new stack's Outputs."
                .to_owned(),
        ));
    };
    let parts: Vec<&str> = jwe.split('.').collect();
    let [header_text, wrapped, iv, ciphertext, tag] = parts.as_slice() else { return Err(not_it()) };
    let decode = |part: &str| URL_SAFE_NO_PAD.decode(part).map_err(|_| not_it());
    let header: Value = serde_json::from_slice(&decode(header_text)?).map_err(|_| not_it())?;
    let says = |name: &str, expected: &str| header[name].as_str() == Some(expected);
    let known =
        header.as_object().is_some_and(|map| map.keys().all(|key| ["alg", "enc", "kid", "typ", "flow", "cty"].contains(&key.as_str())));
    if !(known && says("alg", "RSA-OAEP-256") && says("enc", "A256GCM") && says("typ", RECEIPT_TYPE)) {
        return Err(not_it());
    }
    if !says("kid", &key.kid) || !says("flow", flow) {
        return Err(Problem::Refused(
            "That receipt is from another connection's stack. Copy the Receipt from the stack this connection opened.".to_owned(),
        ));
    }
    let (wrapped, iv, ciphertext, tag) = (decode(wrapped)?, decode(iv)?, decode(ciphertext)?, decode(tag)?);
    if wrapped.len() != KEY_BITS / 8 || iv.len() != 12 || tag.len() != 16 {
        return Err(not_it());
    }
    let private = PrivateDecryptingKey::from_pkcs8(&key.pkcs8).map_err(|_| not_it())?;
    let private = OaepPrivateDecryptingKey::new(private).map_err(|_| not_it())?;
    let mut unwrapped = Zeroizing::new(vec![0u8; private.min_output_size()]);
    let content_key = private.decrypt(&OAEP_SHA256_MGF1SHA256, &wrapped, &mut unwrapped, None).map_err(|_| not_it())?;
    if content_key.len() != 32 {
        return Err(not_it());
    }
    let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, content_key).map_err(|_| not_it())?);
    let nonce = Nonce::try_assume_unique_for_key(&iv).map_err(|_| not_it())?;
    let mut sealed = Zeroizing::new([ciphertext, tag].concat());
    let opened = cipher.open_in_place(nonce, Aad::from(header_text.as_bytes()), &mut sealed).map_err(|_| not_it())?;
    let receipt: Receipt = serde_json::from_slice(opened).map_err(|_| not_it())?;
    if receipt.v != 1 || receipt.flow != flow {
        return Err(not_it());
    }
    Ok(receipt)
}

// ---- STS and IAM ----

/// An access key to sign with; `token` for a session's.
pub struct Keys {
    pub id: String,
    pub secret: Zeroizing<String>,
    pub token: String,
}

/// RFC 3986's encoding, as SigV4 wants it.
fn uri_encode(text: &str) -> String {
    let mut out = String::new();
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(char::from(byte));
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Undoes percent-encoding ("+" stays "+"), as IAM sends policy documents.
fn uri_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(byte) = text.get(index + 1..index + 3).and_then(|pair| u8::from_str_radix(pair, 16).ok())
        {
            out.push(byte);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = <Hmac<Sha256> as hmac::KeyInit>::new_from_slice(key).expect("any key length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// A request to sign: its headers are signed all, names in lower case.
struct Signing<'a> {
    method: &'a str,
    path: &'a str,
    query: &'a [(String, String)],
    headers: &'a [(String, String)],
    body: &'a [u8],
    region: &'a str,
    service: &'a str,
    /// "20150830T123600Z".
    time: &'a str,
}

/// SigV4's Authorization header for `request`.
fn sign(keys: &Keys, request: &Signing<'_>) -> String {
    let mut query: Vec<(String, String)> = request.query.iter().map(|(name, value)| (uri_encode(name), uri_encode(value))).collect();
    query.sort();
    let query = query.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&");
    let mut headers: Vec<(String, String)> =
        request.headers.iter().map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned())).collect();
    headers.sort();
    let canonical_headers: String = headers.iter().map(|(name, value)| format!("{name}:{value}\n")).collect();
    let signed = headers.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(";");
    let canonical_request =
        format!("{}\n{}\n{query}\n{canonical_headers}\n{signed}\n{}", request.method, request.path, hex(&Sha256::digest(request.body)));
    let date = &request.time[..8];
    let scope = format!("{date}/{}/{}/aws4_request", request.region, request.service);
    let to_sign = format!("AWS4-HMAC-SHA256\n{}\n{scope}\n{}", request.time, hex(&Sha256::digest(canonical_request.as_bytes())));
    let mut key = hmac(format!("AWS4{}", keys.secret.as_str()).as_bytes(), date);
    for part in [request.region, request.service, "aws4_request"] {
        key = hmac(&key, part);
    }
    let signature = hex(&hmac(&key, &to_sign));
    format!("AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed}, Signature={signature}", keys.id)
}

/// Where STS and IAM are: AWS's own, or a stand-in's.
pub struct Endpoints {
    pub sts: String,
    pub iam: String,
}

impl Endpoints {
    pub fn new(http: &Http, region: &str, stand_in: Option<&str>) -> Endpoints {
        let sts = format!("https://sts.{region}.amazonaws.com");
        let iam = "https://iam.amazonaws.com".to_owned();
        Endpoints { sts: http.base("aws.sts", stand_in.unwrap_or(&sts)), iam: http.base("aws.iam", stand_in.unwrap_or(&iam)) }
    }
}

fn unreachable(error: reqwest::Error) -> Problem {
    Problem::Unreachable(format!("Couldn't reach AWS: {}", error.without_url()))
}

/// The time now as SigV4 writes it.
fn amz_time() -> String {
    jiff::Timestamp::now().strftime("%Y%m%dT%H%M%SZ").to_string()
}

/// One call of a query API (STS's or IAM's), signed with `keys`: its XML.
async fn call(http: &Http, keys: &Keys, base: &str, region: &str, service: &str, params: &[(&str, &str)]) -> Result<String, Problem> {
    let url = reqwest::Url::parse(base).map_err(|_| Problem::Unreachable(format!("{base} isn't an address.")))?;
    let host = match url.port() {
        Some(port) => format!("{}:{port}", url.host_str().unwrap_or_default()),
        None => url.host_str().unwrap_or_default().to_owned(),
    };
    let body = params.iter().map(|(name, value)| format!("{}={}", uri_encode(name), uri_encode(value))).collect::<Vec<_>>().join("&");
    let time = amz_time();
    let content_type = "application/x-www-form-urlencoded; charset=utf-8";
    let mut headers =
        vec![("content-type".to_owned(), content_type.to_owned()), ("host".to_owned(), host), ("x-amz-date".to_owned(), time.clone())];
    if !keys.token.is_empty() {
        headers.push(("x-amz-security-token".to_owned(), keys.token.clone()));
    }
    let authorization = sign(
        keys,
        &Signing { method: "POST", path: "/", query: &[], headers: &headers, body: body.as_bytes(), region, service, time: &time },
    );
    let mut request = http.client().post(format!("{}/", base.trim_end_matches('/'))).body(body).header("authorization", authorization);
    for (name, value) in &headers {
        if name != "host" {
            request = request.header(name.as_str(), value.as_str());
        }
    }
    let response = request.send().await.map_err(unreachable)?;
    let status = response.status();
    let text = response.text().await.map_err(unreachable)?;
    if status.is_success() {
        return Ok(text);
    }
    let code = xml_value(&text, "Code").unwrap_or_default();
    let message = xml_value(&text, "Message").unwrap_or_default();
    let said = if message.is_empty() { code.clone() } else { message };
    let said = if said.is_empty() { format!("AWS answered with an error ({}).", status.as_u16()) } else { format!("AWS said: {said}") };
    Err(match code.as_str() {
        "InvalidClientTokenId" | "SignatureDoesNotMatch" | "ExpiredToken" => {
            Problem::Reconnect("AWS no longer accepts Claktory's key for this connection.".to_owned())
        }
        _ if status.is_server_error() || status.as_u16() == 429 || code == "Throttling" => Problem::Unreachable(said),
        _ => Problem::Refused(said),
    })
}

/// Every `<name>…</name>`'s text in `xml`, unescaped. AWS's query APIs
/// answer with plain XML whose leaves are all this needs.
fn xml_values(xml: &str, name: &str) -> Vec<String> {
    let (open, close) = (format!("<{name}>"), format!("</{name}>"));
    let mut values = Vec::new();
    let mut rest = xml;
    while let Some(start) = rest.find(&open) {
        let after = &rest[start + open.len()..];
        let Some(end) = after.find(&close) else { break };
        values.push(unescape(&after[..end]));
        rest = &after[end + close.len()..];
    }
    values
}

fn xml_value(xml: &str, name: &str) -> Option<String> {
    xml_values(xml, name).into_iter().next()
}

fn unescape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let after = &rest[at..];
        let Some(end) = after.find(';') else {
            out.push_str(after);
            return out;
        };
        let entity = &after[1..end];
        let char = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                .or_else(|| entity.strip_prefix('#').and_then(|decimal| decimal.parse().ok()))
                .and_then(char::from_u32),
        };
        match char {
            Some(char) => {
                out.push(char);
                rest = &after[end + 1..];
            }
            None => {
                out.push('&');
                rest = &after[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// A list call's members; more than one page is more than a stack makes.
async fn listed(http: &Http, keys: &Keys, endpoints: &Endpoints, params: &[(&str, &str)], member: &str) -> Result<Vec<String>, Problem> {
    let xml = call(http, keys, &endpoints.iam, "us-east-1", "iam", params).await?;
    if xml_value(&xml, "IsTruncated").as_deref() == Some("true") {
        return Err(Problem::Refused("The stack's user or role has more policies than Claktory's template makes.".to_owned()));
    }
    Ok(xml_values(&xml, member))
}

/// The ARN's last part: a user's or role's name.
fn arn_name(arn: &str) -> &str {
    arn.rsplit('/').next().unwrap_or_default()
}

/// What a connection was checked to be, with AWS itself.
pub struct Verified {
    pub account: String,
    pub statements: Vec<Statement>,
}

/// Checks with AWS, with the receipt's own key, that its user and role are
/// exactly what was proposed: the user can only assume the role and read
/// their policies, belongs to no group and has no other policy; the role
/// trusts only the user and holds exactly `expected`. Anything else is a
/// refusal saying what, and to delete the stack.
pub async fn verify(http: &Http, endpoints: &Endpoints, receipt: &Receipt, expected: &[Statement]) -> Result<Verified, Problem> {
    let keys = Keys { id: receipt.access_key_id.clone(), secret: Zeroizing::new(receipt.secret_access_key.clone()), token: String::new() };
    let refuse = |why: String| {
        Problem::Refused(format!(
            "Claktory didn't keep this connection: {why}. Delete the stack {} in AWS CloudFormation, then connect again.",
            arn_name_of_stack(&receipt.stack)
        ))
    };
    // A new key can take a few seconds to work everywhere.
    let mut identity = Err(Problem::Unreachable(String::new()));
    for attempt in 0..6 {
        identity =
            call(http, &keys, &endpoints.sts, &receipt.region, "sts", &[("Action", "GetCallerIdentity"), ("Version", "2011-06-15")]).await;
        match &identity {
            Err(Problem::Reconnect(_)) if attempt < 5 => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
            _ => break,
        }
    }
    let identity = identity.map_err(|problem| match problem {
        Problem::Reconnect(_) => refuse("AWS doesn't accept the key in the receipt".to_owned()),
        other => other,
    })?;
    if xml_value(&identity, "Arn").as_deref() != Some(receipt.user_arn.as_str())
        || xml_value(&identity, "Account").as_deref() != Some(receipt.account.as_str())
    {
        return Err(refuse("the key in the receipt belongs to another user".to_owned()));
    }
    let user = arn_name(&receipt.user_arn);
    let role = arn_name(&receipt.role_arn);
    let iam = |action: &'static str| [("Action", action), ("Version", "2010-05-08")];

    // The user: its own policies, nothing attached, no group.
    let names = listed(http, &keys, endpoints, &[&iam("ListUserPolicies")[..], &[("UserName", user)]].concat(), "member").await?;
    let mut broker = Vec::new();
    for name in &names {
        let xml = call(
            http,
            &keys,
            &endpoints.iam,
            "us-east-1",
            "iam",
            &[&iam("GetUserPolicy")[..], &[("UserName", user), ("PolicyName", name)]].concat(),
        )
        .await?;
        let document = uri_decode(&xml_value(&xml, "PolicyDocument").unwrap_or_default());
        broker.extend(policy_statements(&document).map_err(|what| refuse(format!("its user has {what}")))?);
    }
    broker_only(&broker, &receipt.user_arn, &receipt.role_arn).map_err(refuse)?;
    let attached =
        listed(http, &keys, endpoints, &[&iam("ListAttachedUserPolicies")[..], &[("UserName", user)]].concat(), "PolicyArn").await?;
    if let Some(policy) = attached.first() {
        return Err(refuse(format!("its user also has the policy {policy}")));
    }
    let groups = listed(http, &keys, endpoints, &[&iam("ListGroupsForUser")[..], &[("UserName", user)]].concat(), "GroupName").await?;
    if let Some(group) = groups.first() {
        return Err(refuse(format!("its user is in the group {group}")));
    }

    // The role: trusted by the user alone, holding exactly what was proposed.
    let xml = call(http, &keys, &endpoints.iam, "us-east-1", "iam", &[&iam("GetRole")[..], &[("RoleName", role)]].concat()).await?;
    if xml_value(&xml, "Arn").as_deref() != Some(receipt.role_arn.as_str()) {
        return Err(refuse("its role isn't the one in the receipt".to_owned()));
    }
    trusts_only(&uri_decode(&xml_value(&xml, "AssumeRolePolicyDocument").unwrap_or_default()), &receipt.user_arn).map_err(refuse)?;
    let names = listed(http, &keys, endpoints, &[&iam("ListRolePolicies")[..], &[("RoleName", role)]].concat(), "member").await?;
    let mut statements = Vec::new();
    for name in &names {
        let xml = call(
            http,
            &keys,
            &endpoints.iam,
            "us-east-1",
            "iam",
            &[&iam("GetRolePolicy")[..], &[("RoleName", role), ("PolicyName", name)]].concat(),
        )
        .await?;
        let document = uri_decode(&xml_value(&xml, "PolicyDocument").unwrap_or_default());
        statements.extend(policy_statements(&document).map_err(|what| refuse(format!("its role has {what}")))?);
    }
    let attached =
        listed(http, &keys, endpoints, &[&iam("ListAttachedRolePolicies")[..], &[("RoleName", role)]].concat(), "PolicyArn").await?;
    if let Some(policy) = attached.first() {
        return Err(refuse(format!("its role also has the policy {policy}")));
    }
    if !same_statements(&statements, expected) {
        return Err(refuse("its role's permissions aren't the ones proposed".to_owned()));
    }
    Ok(Verified { account: receipt.account.clone(), statements })
}

/// A session of the role: its keys and when they expire, in ms since 1970.
pub struct Session {
    pub keys: Keys,
    pub expires_ms: i64,
}

/// A 15-minute session of `role_arn`, narrowed by `policy` (JSON; empty
/// for the whole role).
pub async fn assume_role(
    http: &Http,
    endpoints: &Endpoints,
    broker: &Keys,
    region: &str,
    role_arn: &str,
    session_name: &str,
    policy: &str,
) -> Result<Session, Problem> {
    let seconds = SESSION_SECONDS.to_string();
    let mut params = vec![
        ("Action", "AssumeRole"),
        ("Version", "2011-06-15"),
        ("RoleArn", role_arn),
        ("RoleSessionName", session_name),
        ("DurationSeconds", seconds.as_str()),
    ];
    if !policy.is_empty() {
        params.push(("Policy", policy));
    }
    let xml = call(http, broker, &endpoints.sts, region, "sts", &params).await?;
    let (Some(id), Some(secret), Some(token), Some(expiration)) = (
        xml_value(&xml, "AccessKeyId"),
        xml_value(&xml, "SecretAccessKey"),
        xml_value(&xml, "SessionToken"),
        xml_value(&xml, "Expiration"),
    ) else {
        return Err(Problem::Unreachable("AWS sent a session without its keys.".to_owned()));
    };
    let expires_ms = expiration.parse::<jiff::Timestamp>().map(|time| time.as_millisecond()).unwrap_or(0);
    Ok(Session { keys: Keys { id, secret: Zeroizing::new(secret), token }, expires_ms })
}

// ---- The recipe ----

/// AWS isn't offered until the template is published where quick-create
/// can read it (an S3 URL) and it's been tried against a real account.
const SHIPPED: bool = false;
/// The published template; empty until it is.
const TEMPLATE_URL: &str = "";
/// A session lasts this long…
const SESSION_SECONDS: u32 = 15 * 60;
/// …and is renewed this long before it ends.
const RENEW_BEFORE_MS: i64 = 5 * 60 * 1000;
/// AWS's limit on a session policy, in characters.
const MAX_SESSION_POLICY: usize = 2048;

pub struct AwsStack {
    /// In the dev Claktory only: a stand-in playing STS and IAM
    /// (`FACTORY_AWS_BASE`).
    stand_in: Option<String>,
    /// The template's S3 URL: the published one, or `FACTORY_AWS_TEMPLATE_URL`
    /// in the dev Claktory.
    template_url: String,
}

impl Default for AwsStack {
    fn default() -> AwsStack {
        AwsStack::new()
    }
}

impl AwsStack {
    /// The recipe, with AWS's own services and the published template.
    pub fn new() -> AwsStack {
        AwsStack { stand_in: None, template_url: TEMPLATE_URL.to_owned() }
    }

    /// The recipe with `template_url` for the template, for tests.
    #[doc(hidden)]
    pub fn with_template(template_url: &str) -> AwsStack {
        AwsStack { stand_in: None, template_url: template_url.to_owned() }
    }

    /// The built-in recipe, while AWS isn't shipped only in the dev
    /// Claktory: with `FACTORY_AWS_ENABLED=1` against AWS itself (and the
    /// template at `FACTORY_AWS_TEMPLATE_URL`), or with `FACTORY_AWS_BASE`
    /// against a stand-in playing STS and IAM.
    pub(crate) fn built_in() -> Option<AwsStack> {
        let set = |name: &str| std::env::var(name).ok().map(|value| value.trim().to_owned()).filter(|value| !value.is_empty());
        let dev = std::env::args().any(|arg| arg == "--dev");
        let stand_in = set("FACTORY_AWS_BASE").filter(|_| dev).map(|base| base.trim_end_matches('/').to_owned());
        let enabled = dev && set("FACTORY_AWS_ENABLED").is_some_and(|value| value != "0");
        if !(SHIPPED || enabled || stand_in.is_some()) {
            return None;
        }
        let template_url = set("FACTORY_AWS_TEMPLATE_URL").filter(|_| dev).unwrap_or_else(|| TEMPLATE_URL.to_owned());
        Some(AwsStack { stand_in, template_url })
    }

    /// The flow's key and the quick-create page for the proposal.
    fn quick_create(&self, start: &Start<'_>) -> Result<(String, Control), Problem> {
        let Some(proposal) = start.proposal.filter(|proposal| !proposal.statements.is_empty()) else {
            return Err(Problem::Refused(
                "AWS is connected for an agent's proposal, which says exactly what it allows. Ask an agent to propose it.".to_owned(),
            ));
        };
        if self.template_url.is_empty() {
            return Err(Problem::Refused("Claktory's AWS template isn't published yet.".to_owned()));
        }
        let region = super::aws::region(&proposal.region)?;
        let statements = read_statements(&proposal.statements).map_err(Problem::Refused)?;
        let mut random = [0u8; 18];
        getrandom::fill(&mut random).map_err(|_| Problem::Refused("Claktory couldn't make a key for this connection.".to_owned()))?;
        let flow = URL_SAFE_NO_PAD.encode(random);
        let (key, jwk) = flow_key(&flow)?;
        let hash = proposal_hash(&region, &statements);
        let mut suffix = [0u8; 2];
        getrandom::fill(&mut suffix).unwrap_or_default();
        let stack = format!("Claktory-{}-{}", proposal.id.chars().filter(char::is_ascii_alphanumeric).collect::<String>(), hex(&suffix));
        let mut parameters = vec![
            ("templateURL".to_owned(), self.template_url.clone()),
            ("stackName".to_owned(), stack),
            ("param_FlowId".to_owned(), flow.clone()),
            ("param_PublicKeyJwk".to_owned(), jwk),
            ("param_ProposalHash".to_owned(), hash.clone()),
        ];
        parameters.extend(slot_parameters(&statements).into_iter().map(|(name, value)| (format!("param_{name}"), value)));
        let query = parameters.iter().map(|(name, value)| format!("{name}={}", uri_encode(value))).collect::<Vec<_>>().join("&");
        let url = format!("https://{region}.console.aws.amazon.com/cloudformation/home?region={region}#/stacks/create/review?{query}");
        let pending = Pending { flow, secret_key: STANDARD.encode(key.pkcs8.as_slice()), hash, region, statements };
        Ok((url, Control::from_value(&pending)))
    }

    async fn pasted(&self, http: &Http, pending: &Control, token: &str) -> Result<Control, Problem> {
        let pending: Pending = pending.parse()?;
        let key = FlowKey {
            kid: pending.flow.clone(),
            pkcs8: Zeroizing::new(
                STANDARD
                    .decode(&pending.secret_key)
                    .map_err(|_| Problem::Refused("This connection's key is gone. Start again.".to_owned()))?,
            ),
        };
        let receipt = open_receipt(token, &key, &pending.flow)?;
        if receipt.proposal != pending.hash || receipt.region != pending.region {
            return Err(Problem::Refused(
                "That receipt is from a stack with other rules or in another region. Copy the Receipt from the stack this connection opened."
                    .to_owned(),
            ));
        }
        let verified = verify(http, &self.endpoints(http, &receipt.region), &receipt, &pending.statements).await?;
        Ok(Control::from_value(&Broker {
            account: verified.account,
            region: receipt.region.clone(),
            stack: receipt.stack.clone(),
            user_arn: receipt.user_arn.clone(),
            role_arn: receipt.role_arn.clone(),
            access_key_id: receipt.access_key_id.clone(),
            secret_access_key: receipt.secret_access_key.clone(),
            statements: pending.statements,
        }))
    }

    fn endpoints(&self, http: &Http, region: &str) -> Endpoints {
        Endpoints::new(http, region, self.stand_in.as_deref())
    }
}

static SPEC: std::sync::LazyLock<Spec> = std::sync::LazyLock::new(|| {
    Spec {
    id: ID.into(),
    name: "AWS".into(),
    description: "Give agents exactly the AWS permissions an agent proposed, in one region of one of your accounts, with keys that last 15 minutes and renew by themselves.".into(),
    accesses: vec![AccessSpec {
        id: "proposed".into(),
        label: "The permissions proposed".into(),
        why: "You create a stack in your AWS account that allows exactly what the agent proposed, and nothing more.".into(),
        scopes: Vec::new(),
        preselected: true,
    }],
    flow: FlowKind::Paste,
    variables: ["AWS_ACCESS_KEY_ID", "AWS_SECRET_ACCESS_KEY", "AWS_SESSION_TOKEN", "AWS_REGION", "AWS_DEFAULT_REGION"].map(str::to_owned).to_vec(),
    setup_note: "On AWS's page, check that the rules are what you want, tick the box about IAM resources, and choose Create stack. When the stack is created, copy Receipt from its Outputs tab and paste it here.".into(),
    ..Spec::default()
}
});

/// What a flow keeps while the person creates the stack: its one-time key
/// and what the stack must hold.
#[derive(Serialize, Deserialize)]
struct Pending {
    flow: String,
    /// The private key, PKCS#8 in base64: "secret" so it's never shown.
    secret_key: String,
    hash: String,
    region: String,
    statements: Vec<Statement>,
}

/// A connection's control: the broker user's key and what it was checked
/// to reach.
#[derive(Serialize, Deserialize)]
struct Broker {
    account: String,
    region: String,
    stack: String,
    user_arn: String,
    role_arn: String,
    access_key_id: String,
    secret_access_key: String,
    statements: Vec<Statement>,
}

impl Broker {
    fn keys(&self) -> Keys {
        Keys { id: self.access_key_id.clone(), secret: Zeroizing::new(self.secret_access_key.clone()), token: String::new() }
    }

    /// The stack's page in the CloudFormation console.
    fn stack_page(&self) -> String {
        format!(
            "https://{0}.console.aws.amazon.com/cloudformation/home?region={0}#/stacks/stackinfo?stackId={1}",
            self.region,
            uri_encode(&self.stack)
        )
    }
}

/// What a secret made from it remembers, to renew it.
#[derive(Serialize, Deserialize)]
struct Made {
    /// The session policy; empty for the whole role.
    policy: String,
    session: String,
}

/// Whether `text` matches IAM's pattern `pattern` (* and ?), as IAM
/// compares: actions in any case.
fn glob(pattern: &str, text: &str) -> bool {
    let (pattern, text): (Vec<char>, Vec<char>) = (pattern.chars().collect(), text.chars().collect());
    let (mut p, mut t, mut star, mut mark) = (0, 0, None, 0);
    while t < text.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|&char| char == '*')
}

/// The session policy for what's asked, checked against what the role
/// allows: empty for all of it.
fn session_policy(statements: &[Statement], permissions: &[String], resources: &[String]) -> Result<String, Problem> {
    let permissions: Vec<String> = permissions.iter().map(|text| text.trim().to_owned()).filter(|text| !text.is_empty()).collect();
    let resources: Vec<String> = resources.iter().map(|text| text.trim().to_owned()).filter(|text| !text.is_empty()).collect();
    if permissions.is_empty() && resources.is_empty() {
        return Ok(String::new());
    }
    for permission in &permissions {
        let lower = permission.to_ascii_lowercase();
        if !statements.iter().any(|statement| statement.actions.iter().any(|action| glob(&action.to_ascii_lowercase(), &lower))) {
            return Err(Problem::Refused(format!("{permission} isn't one of this connection's permissions.")));
        }
    }
    for resource in &resources {
        if !statements.iter().any(|statement| statement.resources.iter().any(|allowed| glob(allowed, resource))) {
            return Err(Problem::Refused(format!("{resource} isn't one of this connection's resources.")));
        }
    }
    let action = if permissions.is_empty() { json!("*") } else { json!(permissions) };
    let resource = if resources.is_empty() { json!("*") } else { json!(resources) };
    let policy = canonical(&json!({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": action, "Resource": resource}]}));
    if policy.len() > MAX_SESSION_POLICY {
        return Err(Problem::Refused(format!(
            "What's asked is over AWS's {MAX_SESSION_POLICY} characters for a session: ask with wildcards (ec2:Describe*) or for fewer."
        )));
    }
    Ok(policy)
}

/// A session's name, as AWS takes it and CloudTrail shows it.
fn session_name(secret: &str) -> String {
    let name: String =
        secret.chars().map(|char| if char.is_ascii_alphanumeric() || "=,.@-".contains(char) { char } else { '-' }).take(54).collect();
    format!("claktory-{name}")
}

async fn session(recipe: &AwsStack, http: &Http, broker: &Broker, made: &Made) -> Result<Derived, Problem> {
    let endpoints = recipe.endpoints(http, &broker.region);
    let session = assume_role(http, &endpoints, &broker.keys(), &broker.region, &broker.role_arn, &made.session, &made.policy)
        .await
        .map_err(|problem| match problem {
            Problem::Refused(text) => Problem::Reconnect(format!("AWS no longer lets this connection take on its role. {text}")),
            other => other,
        })?;
    let variable = |name: &str, value: &str| Variable { name: name.to_owned(), value: Zeroizing::new(value.to_owned()) };
    let narrowed: Option<Value> = serde_json::from_str(&made.policy).ok();
    let list = |key: &str| -> Vec<String> {
        match narrowed.as_ref().map(|policy| &policy["Statement"][0][key]) {
            Some(Value::Array(items)) => items.iter().filter_map(|item| item.as_str().map(str::to_owned)).collect(),
            _ => Vec::new(),
        }
    };
    let (permissions, resources) = (list("Action"), list("Resource"));
    let access = if made.policy.is_empty() {
        format!("Everything this connection allows in AWS account {} ({})", broker.account, broker.region)
    } else {
        format!("Part of what this connection allows in AWS account {} ({})", broker.account, broker.region)
    };
    Ok(Derived {
        values: Some(vec![
            variable("AWS_ACCESS_KEY_ID", &session.keys.id),
            variable("AWS_SECRET_ACCESS_KEY", &session.keys.secret),
            variable("AWS_SESSION_TOKEN", &session.keys.token),
            variable("AWS_REGION", &broker.region),
            variable("AWS_DEFAULT_REGION", &broker.region),
        ]),
        meta: ExportMeta {
            access: vec![access],
            permissions,
            resources,
            expires_ms: session.expires_ms,
            per_use: true,
            renew_at_ms: (session.expires_ms - RENEW_BEFORE_MS).max(1),
            recipe: serde_json::to_value(made).unwrap_or_default(),
            public: vec!["AWS_REGION".to_owned(), "AWS_DEFAULT_REGION".to_owned()],
        },
    })
}

impl Recipe for AwsStack {
    fn spec(&self) -> &Spec {
        &SPEC
    }

    /// Makes the flow's key and the quick-create page for the proposal.
    fn start_paste<'a>(&'a self, _http: &'a Http, start: &'a Start<'a>) -> BoxFuture<'a, Result<(String, Control), Problem>> {
        let started = self.quick_create(start);
        Box::pin(async move { started })
    }

    /// Opens the receipt and checks it with AWS: the connection's control.
    fn paste<'a>(
        &'a self,
        http: &'a Http,
        _start: &'a Start<'a>,
        pending: &'a Control,
        token: &'a str,
    ) -> BoxFuture<'a, Result<Control, Problem>> {
        Box::pin(async move { self.pasted(http, pending, token).await })
    }

    fn inspect<'a>(&'a self, http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Inspected, Problem>> {
        Box::pin(async move {
            let broker: Broker = control.parse()?;
            let endpoints = self.endpoints(http, &broker.region);
            let identity = call(
                http,
                &broker.keys(),
                &endpoints.sts,
                &broker.region,
                "sts",
                &[("Action", "GetCallerIdentity"), ("Version", "2011-06-15")],
            )
            .await
            .map_err(|problem| match problem {
                Problem::Refused(text) => Problem::Reconnect(text),
                other => other,
            })?;
            if xml_value(&identity, "Arn").as_deref() != Some(broker.user_arn.as_str()) {
                return Err(Problem::Reconnect("AWS says its key belongs to another user now.".to_owned()));
            }
            let mut resources: Vec<String> = broker.statements.iter().flat_map(|statement| statement.resources.clone()).collect();
            resources.sort();
            resources.dedup();
            Ok(Inspected {
                subject: broker.user_arn.clone(),
                account: format!("{} in {}", broker.account, broker.region),
                access: broker.statements.iter().map(Statement::describe).collect(),
                scopes: Vec::new(),
                resources,
                expires_ms: 0,
            })
        })
    }

    fn choices<'a>(&'a self, _http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<Choices, Problem>> {
        Box::pin(async move {
            let broker: Broker = control.parse()?;
            let mut choices = Vec::new();
            for statement in &broker.statements {
                let why = statement.describe();
                for action in &statement.actions {
                    if !choices.iter().any(|choice: &Choice| choice.kind == "permission" && &choice.id == action) {
                        choices.push(Choice {
                            kind: "permission".into(),
                            id: action.clone(),
                            label: action.clone(),
                            parent: String::new(),
                            why: why.clone(),
                        });
                    }
                }
                for resource in &statement.resources {
                    if !choices.iter().any(|choice: &Choice| choice.kind == "resource" && &choice.id == resource) {
                        choices.push(Choice {
                            kind: "resource".into(),
                            id: resource.clone(),
                            label: resource.clone(),
                            parent: String::new(),
                            why: why.clone(),
                        });
                    }
                }
            }
            Ok(Choices {
                choices,
                rules: "Choose none for everything the connection allows, or only the permissions and resources the work needs (narrower ARNs and wildcards such as ec2:Describe* work too). The connection's conditions always apply. A secret is a 15-minute session that renews by itself while it's used.".into(),
            })
        })
    }

    fn derive<'a>(&'a self, http: &'a Http, control: &'a Control, ask: &'a Derive) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            let broker: Broker = control.parse()?;
            let policy = session_policy(&broker.statements, &ask.permissions, &ask.resources)?;
            let made = Made { policy, session: session_name(&ask.name) };
            session(self, http, &broker, &made).await
        })
    }

    fn renew<'a>(&'a self, http: &'a Http, control: &'a Control, export: &'a ExportMeta) -> BoxFuture<'a, Result<Derived, Problem>> {
        Box::pin(async move {
            let made: Made = serde_json::from_value(export.recipe.clone())
                .map_err(|_| Problem::Refused("Claktory can't tell what this AWS secret allows. Make it again.".to_owned()))?;
            let broker: Broker = control.parse()?;
            session(self, http, &broker, &made).await
        })
    }

    /// AWS's side is the stack, which its own key can't delete.
    fn revoke<'a>(&'a self, _http: &'a Http, control: &'a Control) -> BoxFuture<'a, Result<(), Problem>> {
        Box::pin(async move {
            let broker: Broker = control.parse()?;
            Err(Problem::Refused(format!(
                "Claktory can't delete its stack in AWS. Delete {} in AWS CloudFormation: {}",
                arn_name_of_stack(&broker.stack),
                broker.stack_page()
            )))
        })
    }

    /// The broker's key, both halves.
    fn hidden(&self, control: &Control) -> Vec<Zeroizing<String>> {
        let Ok(broker) = control.parse::<Broker>() else { return Vec::new() };
        vec![Zeroizing::new(broker.access_key_id.clone()), Zeroizing::new(broker.secret_access_key.clone())]
    }
}

/// A stack's name from its id ("arn:…:stack/Claktory-c1-ab12/uuid").
fn arn_name_of_stack(stack: &str) -> &str {
    stack.split('/').nth(1).unwrap_or(stack)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proposed(actions: &[&str], resources: &[&str], condition: &str) -> v1::PolicyStatement {
        v1::PolicyStatement {
            actions: actions.iter().map(|text| (*text).to_owned()).collect(),
            resources: resources.iter().map(|text| (*text).to_owned()).collect(),
            condition: condition.to_owned(),
        }
    }

    #[test]
    fn statements_are_checked_as_the_template_takes_them() {
        let good = read_statements(&[proposed(
            &["ec2:RunInstances", " ec2:Describe* "],
            &["arn:aws:ec2:eu-central-1:*:instance/*"],
            r#"{ "StringEquals": {"aws:RequestedRegion": "eu-central-1"} }"#,
        )])
        .unwrap();
        assert_eq!(good[0].actions, ["ec2:RunInstances", "ec2:Describe*"]);
        assert_eq!(good[0].condition, r#"{"StringEquals":{"aws:RequestedRegion":"eu-central-1"}}"#);
        for (bad, why) in [
            (vec![], "Give the statements"),
            (vec![proposed(&["*"], &["*"], "")], "isn't an IAM action"),
            (vec![proposed(&["ec2:RunInstances"], &[], "")], "needs both"),
            (vec![proposed(&["ec2:RunInstances"], &["arn:aws:s3:::a,b"], "")], "isn't an ARN"),
            (vec![proposed(&["ec2:RunInstances"], &["*"], "[1]")], "JSON object"),
            (vec![proposed(&["ec2:RunInstances"], &["*"], "{}")], "JSON object"),
            (vec![proposed(&["ec2:X"], &["*"], ""); 11], "at most 10"),
        ] {
            let error = read_statements(&bad).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
        let parameters = slot_parameters(&good);
        assert_eq!(parameters[0], ("Actions1".to_owned(), "ec2:RunInstances,ec2:Describe*".to_owned()));
        assert_eq!(parameters[2].0, "Condition1");
        assert_eq!(proposal_hash("eu-central-1", &good).len(), 64);
        assert_ne!(proposal_hash("eu-central-1", &good), proposal_hash("eu-west-1", &good));
    }

    #[test]
    fn a_role_policy_matches_the_proposal_however_iam_writes_it() {
        let expected = read_statements(&[
            proposed(
                &["ec2:RunInstances"],
                &["arn:aws:ec2:*:*:instance/*"],
                r#"{"StringEqualsIfExists":{"ec2:InstanceType":["t3.small","t3a.small"]}}"#,
            ),
            proposed(&["ec2:DescribeInstances"], &["*"], ""),
        ])
        .unwrap();
        let found = policy_statements(
            r#"{"Version":"2012-10-17","Statement":[
                {"Sid":"Statement2","Effect":"Allow","Action":"ec2:describeinstances","Resource":"*"},
                {"Sid":"Statement1","Effect":"Allow","Action":["ec2:RunInstances"],"Resource":["arn:aws:ec2:*:*:instance/*"],
                 "Condition":{"StringEqualsIfExists":{"ec2:InstanceType":["t3a.small","t3.small"]}}}]}"#,
        )
        .unwrap();
        assert!(same_statements(&found, &expected));
        let broader = policy_statements(r#"{"Statement":[{"Effect":"Allow","Action":"ec2:*","Resource":"*"}]}"#).unwrap();
        assert!(!same_statements(&broader, &expected));
        assert!(policy_statements(r#"{"Statement":{"Effect":"Allow","NotAction":"iam:*","Resource":"*"}}"#).is_err());
        assert!(policy_statements(r#"{"Statement":{"Effect":"Deny","Action":"iam:*","Resource":"*"}}"#).is_err());
        assert!(role_policy(&expected).len() < MAX_ROLE_POLICY);
    }

    #[test]
    fn the_broker_may_only_assume_its_role_and_read_policies() {
        let user = "arn:aws:iam::123456789012:user/claktory/Broker-1";
        let role = "arn:aws:iam::123456789012:role/claktory/Role-1";
        let policy = format!(
            r#"{{"Statement":[{{"Effect":"Allow","Action":"sts:AssumeRole","Resource":"{role}"}},
                {{"Effect":"Allow","Action":["iam:GetUser","iam:ListUserPolicies","iam:GetUserPolicy"],"Resource":"{user}"}},
                {{"Effect":"Allow","Action":["iam:GetRole","iam:GetRolePolicy"],"Resource":"{role}"}}]}}"#
        );
        assert!(broker_only(&policy_statements(&policy).unwrap(), user, role).is_ok());
        let more = policy_statements(r#"{"Statement":[{"Effect":"Allow","Action":"sts:AssumeRole","Resource":"*"}]}"#).unwrap();
        assert!(broker_only(&more, user, role).is_err());
        let trust = format!(r#"{{"Statement":[{{"Effect":"Allow","Principal":{{"AWS":"{user}"}},"Action":"sts:AssumeRole"}}]}}"#);
        assert!(trusts_only(&trust, user).is_ok());
        let anyone = r#"{"Statement":[{"Effect":"Allow","Principal":{"AWS":"arn:aws:iam::123456789012:root"},"Action":"sts:AssumeRole"}]}"#;
        assert!(trusts_only(anyone, user).is_err());
    }

    #[test]
    fn a_secret_asks_only_for_what_the_role_allows() {
        assert!(glob("ec2:describe*", "ec2:describeinstances") && glob("ec2:*", "ec2:describe*") && glob("a?c", "abc"));
        assert!(!glob("ec2:describe*", "ec2:runinstances") && !glob("arn:aws:s3:::a/*", "arn:aws:s3:::b/x"));
        let statements = read_statements(&[proposed(&["ec2:Describe*", "ec2:RunInstances"], &["arn:aws:ec2:*:*:instance/*"], "")]).unwrap();
        assert_eq!(session_policy(&statements, &[], &[]).ok().as_deref(), Some(""), "nothing asked: the whole role");
        let policy = session_policy(&statements, &["ec2:DescribeImages".to_owned()], &[]).ok().unwrap();
        assert_eq!(policy, r#"{"Statement":[{"Action":["ec2:DescribeImages"],"Effect":"Allow","Resource":"*"}],"Version":"2012-10-17"}"#);
        assert!(session_policy(&statements, &["s3:GetObject".to_owned()], &[]).is_err());
        assert!(session_policy(&statements, &[], &["arn:aws:ec2:eu-west-1:1:volume/v".to_owned()]).is_err());
        assert!(session_policy(&statements, &[], &["arn:aws:ec2:eu-west-1:1:instance/i-1".to_owned()]).is_ok());
        assert_eq!(session_name("AWS describe (Builder)"), "claktory-AWS-describe--Builder-");
    }

    #[test]
    fn xml_leaves_are_read_and_unescaped() {
        let xml = "<R><PolicyNames><member>a&amp;b</member><member>c</member></PolicyNames><IsTruncated>false</IsTruncated>\
                   <PolicyDocument>%7B%22a%22%3A1%7D</PolicyDocument><X>&#65;&#x42;&bogus</X></R>";
        assert_eq!(xml_values(xml, "member"), ["a&b", "c"]);
        assert_eq!(uri_decode(&xml_value(xml, "PolicyDocument").unwrap()), r#"{"a":1}"#);
        assert_eq!(xml_value(xml, "X").unwrap(), "AB&bogus");
        assert_eq!(uri_decode("a+b%2Bc%zz"), "a+b+c%zz");
    }

    /// AWS's own example of a signed request (IAM ListUsers, in the SigV4
    /// documentation).
    #[test]
    fn requests_are_signed_as_aws_documents() {
        let keys = Keys {
            id: "AKIDEXAMPLE".to_owned(),
            secret: Zeroizing::new("wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY".to_owned()),
            token: String::new(),
        };
        let query = [("Action".to_owned(), "ListUsers".to_owned()), ("Version".to_owned(), "2010-05-08".to_owned())];
        let headers = [
            ("Content-Type".to_owned(), "application/x-www-form-urlencoded; charset=utf-8".to_owned()),
            ("Host".to_owned(), "iam.amazonaws.com".to_owned()),
            ("X-Amz-Date".to_owned(), "20150830T123600Z".to_owned()),
        ];
        let authorization = sign(
            &keys,
            &Signing {
                method: "GET",
                path: "/",
                query: &query,
                headers: &headers,
                body: b"",
                region: "us-east-1",
                service: "iam",
                time: "20150830T123600Z",
            },
        );
        assert_eq!(
            authorization,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/iam/aws4_request, \
             SignedHeaders=content-type;host;x-amz-date, \
             Signature=5d672d79c15b13162d9279b0855cfba6789a8edb4c82c400e06b5924a6f2b5d7"
        );
    }

    /// Seals `payload` to `key` as the template's Lambda does, with
    /// aws-lc-rs in its place.
    fn seal(key: &FlowKey, header: &Value, payload: &[u8]) -> String {
        use aws_lc_rs::rand::{SecureRandom, SystemRandom};
        use aws_lc_rs::rsa::OaepPublicEncryptingKey;
        let public = PrivateDecryptingKey::from_pkcs8(&key.pkcs8).unwrap().public_key();
        let public = OaepPublicEncryptingKey::new(public).unwrap();
        let random = SystemRandom::new();
        let mut content_key = [0u8; 32];
        random.fill(&mut content_key).unwrap();
        let mut iv = [0u8; 12];
        random.fill(&mut iv).unwrap();
        let mut wrapped = vec![0u8; public.ciphertext_size()];
        let wrapped = public.encrypt(&OAEP_SHA256_MGF1SHA256, &content_key, &mut wrapped, None).unwrap().to_vec();
        let header_text = URL_SAFE_NO_PAD.encode(header.to_string());
        let cipher = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &content_key).unwrap());
        let mut sealed = payload.to_vec();
        let tag =
            cipher.seal_in_place_separate_tag(Nonce::assume_unique_for_key(iv), Aad::from(header_text.as_bytes()), &mut sealed).unwrap();
        format!(
            "{RECEIPT_PREFIX}{header_text}.{}.{}.{}.{}",
            URL_SAFE_NO_PAD.encode(wrapped),
            URL_SAFE_NO_PAD.encode(iv),
            URL_SAFE_NO_PAD.encode(sealed),
            URL_SAFE_NO_PAD.encode(tag.as_ref())
        )
    }

    fn payload(flow: &str) -> Vec<u8> {
        json!({
            "v": 1, "flow": flow, "proposal": "00", "account": "123456789012", "region": "eu-central-1",
            "stack": "arn:aws:cloudformation:eu-central-1:123456789012:stack/Claktory-x/1",
            "user_arn": "arn:aws:iam::123456789012:user/claktory/B", "role_arn": "arn:aws:iam::123456789012:role/claktory/R",
            "access_key_id": "AKIAEXAMPLE", "secret_access_key": "secret", "issued_at": "2026-01-01T00:00:00Z",
        })
        .to_string()
        .into_bytes()
    }

    #[test]
    fn a_receipt_opens_only_with_its_flows_key_and_header() {
        let flow = "flowflowflowflowflowflow";
        let (key, jwk) = flow_key(flow).unwrap();
        let read: Value = serde_json::from_str(&jwk).unwrap();
        assert_eq!(URL_SAFE_NO_PAD.decode(read["n"].as_str().unwrap()).unwrap().len(), 384);
        assert_eq!(read["e"], "AQAB");
        let header = json!({"alg": "RSA-OAEP-256", "enc": "A256GCM", "kid": flow, "typ": RECEIPT_TYPE, "flow": flow});
        let receipt = seal(&key, &header, &payload(flow));
        let opened = open_receipt(&format!("  {receipt}\n"), &key, flow).unwrap();
        assert_eq!(opened.access_key_id, "AKIAEXAMPLE");
        assert_eq!(opened.secret_access_key, "secret");

        // Another flow's key, a changed header, another algorithm, a changed byte.
        let (other, _) = flow_key(flow).unwrap();
        assert!(open_receipt(&receipt, &other, flow).is_err());
        assert!(open_receipt(&receipt, &key, "anotherflowanotherflow").is_err());
        let mut tampered = header.clone();
        tampered["alg"] = json!("RSA1_5");
        assert!(open_receipt(&seal(&key, &tampered, &payload(flow)), &key, flow).is_err());
        let jwe = receipt.strip_prefix(RECEIPT_PREFIX).unwrap();
        let parts: Vec<&str> = jwe.split('.').collect();
        let changed_header = URL_SAFE_NO_PAD.encode(header.to_string().replace("A256GCM", "A256GCM "));
        let swapped = format!("{RECEIPT_PREFIX}{changed_header}.{}", parts[1..].join("."));
        assert!(open_receipt(&swapped, &key, flow).is_err(), "the header is authenticated");
        let mut bytes = URL_SAFE_NO_PAD.decode(parts[3]).unwrap();
        bytes[0] ^= 1;
        let flipped = format!("{RECEIPT_PREFIX}{}.{}.{}.{}.{}", parts[0], parts[1], parts[2], URL_SAFE_NO_PAD.encode(bytes), parts[4]);
        assert!(open_receipt(&flipped, &key, flow).is_err(), "the ciphertext is authenticated");
        assert!(open_receipt("AKIA…", &key, flow).err().unwrap().text().contains("isn't a Claktory receipt"));
    }
}
