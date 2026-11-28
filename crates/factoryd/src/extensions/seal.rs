//! The Seal extension (`crate::seals`): an agent seals a file whose content
//! must stay fixed from then on (a pass line written before a run), and
//! checks later whether it still matches. Off unless turned on for an
//! agent: sealing is for the few files where an edit afterwards would
//! matter, never every file.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection, async_tool};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::seals::{self, Seal};

pub(crate) const ID: &str = "seal";
const CHECK: &str = "seal_check";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Seal",
    icon: "stamp",
    description: "Lets it seal a file it must not change afterwards, such as a pass line written before a run: Claktory records that version, shows you when the file changes, and keeps the sealed version for you to open.",
    default_enabled: false,
    options: &[],
};

const SEAL: &str = "Seal a file whose content must stay fixed from now on, such as a pass line or a metric's definition written before a run: Claktory records its SHA-256, you, the time and your note in its own records, and keeps a copy of this version (files up to 64 MB), so anyone can tell later whether the file changed since and open the version sealed. Only for files like that, never as a habit. Your tools can't change or remove a seal; the person sees it in History, and a viewer showing the file says whether it still matches. Sealing again records a new seal and keeps the old ones. path: absolute, or relative to your working folder. note: what the seal fixes, in a short line.";
const CHECK_ABOUT: &str = "Whether a file still matches its latest seal, and its seals, newest first: who sealed it, when, the SHA-256 and the note. path: absolute, or relative to your working folder.";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SealArgs {
    path: String,
    note: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CheckArgs {
    path: String,
}

/// The agent's working folder; an error if it's gone.
fn folder(factory: &Factory, agent: &str) -> Result<std::path::PathBuf, String> {
    let inner = factory.lock();
    let me = super::caller(&inner.config, agent)?;
    Ok(crate::runner::places(&factory.home, &inner.config, me, &[]).folder)
}

fn when(seal: &Seal) -> String {
    crate::transcript::local_time(seal.at, "%-d %b %Y at %H:%M")
}

async fn seal(factory: Arc<Factory>, agent: String, args: SealArgs) -> Result<ToolResult, String> {
    let sealed = tokio::task::spawn_blocking(move || {
        let folder = folder(&factory, &agent)?;
        seals::seal(&factory, &agent, &folder, &args.path, &args.note)
    })
    .await
    .map_err(|error| format!("Sealing stopped: {error}"))??;
    let seal = &sealed.seal;
    let earlier = sealed.seals.seals.len() - 1;
    let kept = if seal.kept {
        "A copy of this version is kept, for the person to open."
    } else {
        "It's over 64 MB, so no copy is kept: the seal is its SHA-256 alone."
    };
    let earlier = match earlier {
        0 => String::new(),
        1 => " It had one seal before; that one stays.".to_owned(),
        many => format!(" It had {many} seals before; those stay."),
    };
    let text = format!(
        "Sealed {} (SHA-256 {}) on {}.{earlier} {kept} Your tools can't change or remove this seal; seal_check tells whether the file still matches.",
        seal.path,
        seal.sha256,
        when(seal),
    );
    Ok(ToolResult::text(text, json!({seals::DETAIL: seal.id, "path": seal.path, "sha256": seal.sha256})))
}

async fn check(factory: Arc<Factory>, agent: String, args: CheckArgs) -> Result<ToolResult, String> {
    let (path, list, state) = tokio::task::spawn_blocking(move || {
        let folder = folder(&factory, &agent)?;
        seals::check(&factory, &folder, &args.path)
    })
    .await
    .map_err(|error| format!("Checking stopped: {error}"))??;
    let Some(latest) = list.first() else {
        return Ok(ToolResult::text(format!("{path} has no seals."), json!({"path": path, "seals": 0})));
    };
    let mut text = match state {
        v1::FileSealState::Matches => format!("{path} matches its latest seal, by {} on {}.", latest.agent_name, when(latest)),
        _ if !std::path::Path::new(&path).is_file() => {
            format!("{path} is gone. Its latest seal is {}'s, on {}.", latest.agent_name, when(latest))
        }
        _ => format!("{path} changed since its latest seal, by {} on {}.", latest.agent_name, when(latest)),
    };
    text.push_str("\n\nIts seals, newest first:");
    for seal in &list {
        let kept = if seal.kept { "" } else { " (no copy kept)" };
        text.push_str(&format!("\n- {} by {}: {}{kept}. Note: {}", when(seal), seal.agent_name, seal.sha256, seal.note));
    }
    let matches = state == v1::FileSealState::Matches;
    Ok(ToolResult::text(text, json!({"path": path, "seals": list.len(), "matches": matches})))
}

pub(crate) struct Seals;

impl Extension for Seals {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        // The tools' descriptions say it all.
        Vec::new()
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let seal_parameters = json!({
            "type": "object",
            "required": ["path", "note"],
            "properties": {"path": {"type": "string", "minLength": 1}, "note": {"type": "string", "minLength": 1}},
            "additionalProperties": false,
        });
        let check_parameters = json!({
            "type": "object",
            "required": ["path"],
            "properties": {"path": {"type": "string", "minLength": 1}},
            "additionalProperties": false,
        });
        vec![
            async_tool(factory, agent, seals::TOOL, "Seal a file", SEAL, seal_parameters, seal),
            async_tool(factory, agent, CHECK, "Check a seal", CHECK_ABOUT, check_parameters, check),
        ]
    }
}
