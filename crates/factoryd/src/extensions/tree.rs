//! Own checkout: the agent works in a copy-on-write checkout of its
//! project's git repository of its own, on its own branch, and lands its
//! work with `tree_land`. The checkouts themselves are `crate::trees`; the
//! runner makes one before a run (`Factory::ensure_checkout`) and points the
//! agent's tools at it (`runner::places`).

pub(crate) mod caches;

use std::path::Path;
use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde_json::json;

use super::{Extension, Manifest, PromptKind, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};
use crate::home::Home;
use crate::trees;

pub(crate) const ID: &str = "tree";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Own checkout",
    icon: "git-branch",
    description: "Gives it its own copy of the project's code, so agents working at the same time don't get in each other's way. It merges its work back when it's ready. For projects kept in git.",
    default_enabled: true,
    options: &[],
};

const NEVER: &str = "Never make a git worktree, clone or copy of the repository yourself.";

const LAND: &str = "Land your work: fast-forward the main checkout's branch to your branch. Commit first, and rebase onto the main branch if it moved on. It refuses a branch that would put files back to versions the main branch had before (undoing others' work); allow_rewinds: true lands it anyway, only for a revert you mean.";

pub(crate) struct Tree;

impl Extension for Tree {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, factory: &Factory, inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let source = crate::runner::source_folder(&factory.home, &inner.config, agent);
        vec![PromptSection { kind: PromptKind::Static, text: section(&factory.home, &agent.id, &source) }]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        // Only an agent whose folder is in a git repository can have a checkout to land.
        let in_repo = {
            let inner = factory.lock();
            inner.config.agent(agent).is_some_and(|me| {
                trees::plan(&factory.home, agent, &crate::runner::source_folder(&factory.home, &inner.config, me)).is_ok()
            })
        };
        if !in_repo {
            return Vec::new();
        }
        let handler: factory_agent::Handler = super::agent_handler(factory, agent, move |factory, agent, args| async move {
            let allow_rewinds = args.get("allow_rewinds").and_then(serde_json::Value::as_bool).unwrap_or(false);
            let factory = super::live(factory)?;
            let checkout = {
                let inner = factory.lock();
                let me = super::caller(&inner.config, &agent).map_err(factory_agent::Error::Other)?;
                crate::runner::checkout(&factory.home, &inner.config, me)
            };
            let Some(checkout) = checkout else {
                return Err(factory_agent::Error::Other("You have no checkout of your own to land.".into()));
            };
            let landed = tokio::task::spawn_blocking(move || trees::land(&checkout, allow_rewinds))
                .await
                .map_err(|error| factory_agent::Error::Other(error.to_string()))?;
            let text = landed.map_err(factory_agent::Error::Other)?;
            Ok(ToolResult::text(text, json!({})))
        });
        let parameters = json!({
            "type": "object",
            "properties": {"allow_rewinds": {"type": "boolean"}},
            "additionalProperties": false,
        });
        vec![super::tool("tree_land", "Land work", LAND, parameters, handler)]
    }
}

/// What the agent's system prompt says of its checkout, as things are now.
pub(crate) fn section(home: &Home, agent: &str, source: &Path) -> String {
    let checkout = match trees::plan(home, agent, source) {
        Ok(checkout) => checkout,
        Err(why) => {
            return format!(
                "## Own checkout\n\nYou have no checkout of your own: {why}, so you work in {} directly. {NEVER}\n",
                source.display()
            );
        }
    };
    if !checkout.exists() {
        let why = trees::failure(&checkout).unwrap_or_else(|| "it isn't made yet".to_owned());
        return format!(
            "## Own checkout\n\nYou have no checkout of your own: making it failed ({why}), so you work in {} directly. {NEVER}\n",
            source.display()
        );
    }
    let repo = checkout.repo.display();
    let main = trees::main_branch(&checkout.repo);
    let on = main.as_deref().map(|main| format!(", on {main}")).unwrap_or_default();
    let main = main.unwrap_or_else(|| "the main branch".to_owned());
    format!(
        "## Own checkout

You work in your own checkout of the repository at {repo}: {folder}, on the branch {branch}. It's yours alone: it holds only what git tracks, so its build output (target/, node_modules/…) starts empty and the first build there is a full one, then stays between runs, unless it outgrows the build-cache budget while you're idle: then it's cleared, and you're told. The main checkout, {repo}{on}, is the person's and other agents': read it, but never change it, with bash either. {NEVER}

Commit your work here. To land it, rebase onto {main} (git rebase {main}: your branch and {main} share one repository), test, then call tree_land, which fast-forwards {main} in the main checkout to your branch. If {main} moved on meanwhile, rebase and land again.
",
        folder = checkout.folder.display(),
        branch = checkout.branch,
    )
}
