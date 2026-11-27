//! Agents' proposals to connect a service (`secrets_propose_connection`),
//! for agents with the Secrets extension's `connect` option on. A proposal
//! shows in the agent's conversation after the call, and at the top of
//! Settings → Secrets while it waits; the person connects the service for
//! it (StartConnect with its id) or dismisses it. The connection is given to
//! no one, the proposer included: admin agents such as Rivet are told what
//! it reaches and what was asked, and give the agent a secret made from it
//! with the least access the work needs.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1::{self, ConnectProposalState};
use serde::Deserialize;
use serde_json::json;

use super::Spec;
use crate::factory::{Factory, RequestError, host_event};
use crate::vault::connections::ConnectionMeta;

/// The tool agents propose with.
pub(crate) const TOOL: &str = "secrets_propose_connection";
/// The Secrets extension's option that gives an agent the tool.
pub(crate) const OPTION: &str = "connect";

/// The longest purpose, access or reason an agent may give.
const MAX_TEXT: usize = 2000;

const DESCRIPTION: &str = "Propose that the person connect a service to Claktory, such as Cloudflare or AWS. service: its id or name; purpose: what it's for; access: the access the work needs (which permissions, on which zones, accounts or roles); reason: why you need it. For AWS also give region and statements: the exact IAM Allow statements the work needs, at most 10, each with actions (\"ec2:RunInstances\"), resources (ARNs with * for the account, or *) and an optional condition (IAM's Condition block as a JSON string); the connection gets exactly those and nothing more. The person connects it on the service's own site, or dismisses the proposal, and you're told. Connecting gives you nothing yet: an admin agent then gives you a secret made from the connection, with only the access the work needs.";

/// The proposing tool, for an agent with the option on.
pub(crate) fn tool(factory: &Arc<Factory>, agent: &str) -> Tool {
    let services: Vec<String> = factory.broker.catalog().into_iter().map(|connector| connector.id).collect();
    let mut service = json!({"type": "string"});
    if !services.is_empty() {
        service["description"] = json!(format!("One of: {}.", services.join(", ")));
    }
    let parameters = json!({
        "type": "object",
        "required": ["service", "purpose", "access", "reason"],
        "properties": {
            "service": service,
            "purpose": {"type": "string"},
            "access": {"type": "string"},
            "reason": {"type": "string"},
            "access_id": {"type": "string"},
            "region": {"type": "string"},
            "statements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["actions", "resources"],
                    "properties": {
                        "actions": {"type": "array", "items": {"type": "string"}},
                        "resources": {"type": "array", "items": {"type": "string"}},
                        "condition": {"type": "string"},
                    },
                    "additionalProperties": false,
                },
            },
        },
        "additionalProperties": false,
    });
    crate::extensions::tool(
        TOOL,
        "Propose connecting a service",
        DESCRIPTION,
        parameters,
        crate::extensions::typed_handler(factory, agent, propose),
    )
}

#[derive(Deserialize)]
struct ProposeArgs {
    service: String,
    purpose: String,
    access: String,
    reason: String,
    #[serde(default)]
    access_id: String,
    #[serde(default)]
    region: String,
    #[serde(default)]
    statements: Vec<StatementArg>,
}

#[derive(Deserialize)]
struct StatementArg {
    actions: Vec<String>,
    resources: Vec<String>,
    #[serde(default)]
    condition: String,
}

fn check(field: &str, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Err(format!("Say {field}: the person reads it."));
    }
    if text.chars().count() > MAX_TEXT {
        return Err(format!("Keep {field} under {MAX_TEXT} characters."));
    }
    Ok(text.to_owned())
}

fn propose(factory: &Arc<Factory>, agent: &str, args: ProposeArgs) -> Result<ToolResult, String> {
    let allowed = factory.lock().config.agent(agent).is_some_and(|config| {
        crate::extensions::enabled(config, crate::extensions::secrets::ID)
            && crate::extensions::switch(config, crate::extensions::secrets::ID, OPTION)
    });
    if !allowed {
        return Err("You can't propose connecting services: the person or an admin agent turns that on for you.".to_owned());
    }
    let catalog = factory.broker.catalog();
    let wanted = args.service.trim().to_lowercase();
    let Some(connector) = catalog.iter().find(|connector| connector.id == wanted || connector.name.to_lowercase() == wanted) else {
        let known: Vec<&str> = catalog.iter().map(|connector| connector.id.as_str()).collect();
        return Err(if known.is_empty() {
            "Claktory can't connect any service yet.".to_owned()
        } else {
            format!("Claktory can't connect \"{}\". It can connect: {}.", args.service.trim(), known.join(", "))
        });
    };
    let access_id = args.access_id.trim().to_owned();
    if !access_id.is_empty() && !connector.accesses.iter().any(|access| access.id == access_id) {
        let ids: Vec<&str> = connector.accesses.iter().map(|access| access.id.as_str()).collect();
        return Err(format!("{} has no access \"{access_id}\"; its accesses: {}.", connector.name, ids.join(", ")));
    }
    let (statements, region) = exact_access(&connector.id, &connector.name, &args)?;
    let proposal = v1::ConnectProposal {
        agent_id: agent.to_owned(),
        connector_id: connector.id.clone(),
        access_id,
        purpose: check("purpose", &args.purpose)?,
        access: check("access", &args.access)?,
        reason: check("reason", &args.reason)?,
        statements,
        region,
        ..Default::default()
    };
    let proposal = factory.vault.propose(&proposal).map_err(|error| error.to_string())?;
    factory.vault_changed();
    let person = factory.lock().config.person().to_owned();
    let said = format!(
        "Proposed connecting {}; {person} will connect it or dismiss the proposal, and you'll be told. Connecting gives you nothing yet: an admin agent decides what you get.",
        connector.name
    );
    Ok(ToolResult::text(said, details(&proposal)))
}

/// The exact statements and region a proposal gives, checked: required for
/// a service that takes them (AWS), refused for the others.
fn exact_access(connector: &str, name: &str, args: &ProposeArgs) -> Result<(Vec<v1::PolicyStatement>, String), String> {
    let given: Vec<v1::PolicyStatement> = args
        .statements
        .iter()
        .map(|statement| v1::PolicyStatement {
            actions: statement.actions.clone(),
            resources: statement.resources.clone(),
            condition: statement.condition.clone(),
        })
        .collect();
    if connector != super::aws_stack::ID {
        if !given.is_empty() || !args.region.trim().is_empty() {
            return Err(format!("{name} takes no statements or region: say the access in access."));
        }
        return Ok((Vec::new(), String::new()));
    }
    if args.region.trim().is_empty() {
        return Err(format!("Give the region the work is in, such as eu-central-1: {name} is connected for one region."));
    }
    let region = super::aws::region(&args.region).map_err(|problem| problem.text().to_owned())?;
    let statements = super::aws_stack::read_statements(&given)?;
    Ok((statements.iter().map(super::aws_stack::Statement::to_proto).collect(), region))
}

/// What a proposing call's result's details hold: the proposal as made.
fn details(proposal: &v1::ConnectProposal) -> serde_json::Value {
    json!({
        "connect_proposal": proposal.id,
        "agent": proposal.agent_id,
        "service": proposal.connector_id,
        "access_id": proposal.access_id,
        "purpose": proposal.purpose,
        "access": proposal.access,
        "reason": proposal.reason,
        "created_ms": proposal.created_ms,
        "statements": super::aws_stack::store_statements(&proposal.statements),
        "region": proposal.region,
    })
}

/// The entry a proposing call's result shows (called directly, or through
/// extension_call), pending; None for any other result.
pub(crate) fn proposed(tool: &str, is_error: bool, details: &serde_json::Value) -> Option<v1::ConnectProposal> {
    if !matches!(tool, TOOL | "extension_call") || is_error {
        return None;
    }
    let id = details.get("connect_proposal")?.as_str()?.to_owned();
    let text = crate::transcript::text_fields(details);
    Some(v1::ConnectProposal {
        id,
        agent_id: text("agent"),
        connector_id: text("service"),
        access_id: text("access_id"),
        purpose: text("purpose"),
        access: text("access"),
        reason: text("reason"),
        state: ConnectProposalState::Pending.into(),
        created_ms: details.get("created_ms").and_then(serde_json::Value::as_i64).unwrap_or_default(),
        statements: super::aws_stack::stored_statements(&text("statements")),
        region: text("region"),
        ..Default::default()
    })
}

/// Shows a proposal as it is now in its agent's conversation.
fn show(factory: &Factory, proposal: &v1::ConnectProposal) {
    let mut inner = factory.lock();
    let entry = inner
        .agents
        .get_mut(&proposal.agent_id)
        .and_then(|runtime| runtime.transcript.as_mut())
        .and_then(|transcript| transcript.set_connect_proposal(proposal.clone()));
    if let Some(entry) = entry {
        factory.send_change(&inner, &proposal.agent_id, crate::transcript::Change::Entry(entry));
        // Whether it waits for the person shows with the agent.
        factory.broadcast_workspace(&inner);
    }
}

/// The person dismissed a proposal, or replied to it with a comment, which connects
/// nothing either: its card shows it, and its agent is told.
pub(crate) fn dismiss(factory: &Arc<Factory>, request: &v1::DismissConnectProposal) -> Result<(), RequestError> {
    let comment = crate::decisions::comment(&request.comment)?;
    if request.reply && comment.is_empty() {
        return Err(crate::vault::VaultError::Invalid("Write a reply, or connect or dismiss it.".to_owned()).into());
    }
    let proposal = factory.vault.decide_proposal(&request.proposal_id, None, comment, request.reply)?;
    factory.vault_changed();
    show(factory, &proposal);
    let (person, service) = {
        let inner = factory.lock();
        (inner.config.person_capitalized().to_owned(), service_name(factory, &proposal.connector_id))
    };
    let (summary, text) = if request.reply {
        let summary = format!("{person} replied to your proposal to connect {service}.");
        let text = format!(
            "{person} replied to your proposal to connect {service}; nothing was connected. They say:\n\n{comment}\n\nIf it still fits, propose it again with that in mind.\n\nYou proposed it for: {}",
            proposal.purpose
        );
        (summary, text)
    } else {
        let summary = format!("{person} dismissed your proposal to connect {service}.");
        let text = format!("{summary}{}\n\nYou proposed it for: {}", crate::decisions::note(&person, comment), proposal.purpose);
        (summary, text)
    };
    factory.deliver_event(&proposal.agent_id, text, host_event(summary));
    Ok(())
}

fn service_name(factory: &Factory, connector: &str) -> String {
    factory.broker.recipe(connector).map(|recipe| recipe.spec().name.clone()).unwrap_or_else(|| connector.to_owned())
}

/// A connection was made for a proposal: its card shows it, its agent
/// learns it has nothing yet, and admin agents are asked to give it the
/// least that does the job.
pub(crate) fn connected(
    factory: &Arc<Factory>,
    proposal_id: &str,
    (secret_id, secret_name): (&str, &str),
    spec: &Spec,
    meta: &ConnectionMeta,
    comment: &str,
) {
    let proposal = match factory.vault.decide_proposal(proposal_id, Some((secret_id, secret_name)), comment, false) {
        Ok(proposal) => proposal,
        Err(error) => {
            tracing::warn!(proposal = proposal_id, %error, "could not mark a proposal connected");
            return;
        }
    };
    show(factory, &proposal);
    let (person, proposer, admins) = {
        let inner = factory.lock();
        let config = &inner.config;
        let proposer = config.agent(&proposal.agent_id).map(|agent| agent.name.clone()).unwrap_or_else(|| proposal.agent_id.clone());
        let proposer = match config.agent(&proposal.agent_id).map(|agent| agent.role.trim()).filter(|role| !role.is_empty()) {
            Some(role) => format!("{proposer} ({role}, {})", proposal.agent_id),
            None => format!("{proposer} ({})", proposal.agent_id),
        };
        let admins: Vec<String> = config
            .agents
            .iter()
            .filter(|agent| {
                agent.id != proposal.agent_id
                    && crate::proposals::is_admin(agent)
                    && crate::extensions::enabled(agent, crate::extensions::secrets::ID)
            })
            .map(|agent| agent.id.clone())
            .collect();
        (config.person_capitalized().to_owned(), proposer, admins)
    };
    let account = if meta.account.is_empty() { String::new() } else { format!(" ({})", meta.account) };
    let summary =
        format!("{person} connected {}{account} for your proposal. You don't have it: an admin agent decides what you get.", spec.name);
    let note = crate::decisions::note(&person, comment);
    let text = format!(
        "{person} connected {}{account} for your proposal, as {secret_name}. You don't have it yet: an admin agent compares it with what you asked for and gives you a secret made from it with only the access the work needs, and you'll be told.{note}",
        spec.name
    );
    factory.deliver_event(&proposal.agent_id, text, host_event(summary));

    let list = |items: &[String]| if items.is_empty() { "nothing listed".to_owned() } else { items.join("; ") };
    let exact = if proposal.statements.is_empty() {
        String::new()
    } else {
        let lines: Vec<String> =
            proposal.statements.iter().map(|statement| format!("\n  - {}", super::aws_stack::describe(statement))).collect();
        format!("\n- Exact permissions asked, in {}:{}", proposal.region, lines.concat())
    };
    let summary = format!("{person} connected {} for {proposer}'s proposal: give it the least that does the job.", spec.name);
    let text = format!(
        "{person} connected {}{account} for {proposer}'s proposal, as {secret_name}. No agent has it.\n\n\
         The proposal:\n- Purpose: {}\n- Access asked: {}{exact}\n- Reason: {}\n\n\
         What the connection reaches:\n- Access: {}\n- Scopes: {}\n- Resources: {}\n\n\
         Compare the two. Make a secret from {secret_name} with only the access the proposal's work needs, give that to {proposer}, and tell it what it got. Never give the connection itself. If it asks for more than the work needs, give the narrower thing or ask {person} first.{note}",
        spec.name,
        proposal.purpose,
        proposal.access,
        proposal.reason,
        list(&meta.access),
        list(&meta.scopes),
        list(&meta.resources),
    );
    for admin in admins {
        factory.deliver_event(&admin, text.clone(), host_event(summary.clone()));
    }
}
