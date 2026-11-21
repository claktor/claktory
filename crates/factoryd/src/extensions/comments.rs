//! The Comments extension: the person comments on files shown in panes (see
//! `crate::annotations`), and the agent reads the comments, answers on their
//! threads and resolves them, or leaves comments of its own on a file.
//!
//! The person's comments reach the agent as an event when sent; these tools
//! are how it answers.

use std::sync::Arc;

use factory_agent::{Tool, ToolResult};
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection, tool, typed_handler};
use crate::annotations::anchor::Quote;
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};

pub(crate) const ID: &str = "comments";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Comments",
    icon: "message-square-text",
    description: "Lets you comment on files it shows you, and lets it answer your comments and leave its own.",
    default_enabled: true,
    options: &[],
};

const LIST: &str = "List the comment threads on files addressed to you: each thread's id, the file and lines, the text it's on and its comments. path: only that file (absolute, or relative to your working folder). status: open (the default), resolved or all.";
const REPLY: &str =
    "Answer on a comment thread: the person sees your reply beside the text in the file. thread: its id, such as t-41. body: Markdown.";
const RESOLVE: &str = "Resolve a comment thread once it's dealt with, with an optional note saying what you did (\"Split it into two sentences\"). thread: its id, such as t-41.";
const SUGGEST: &str = "Suggest a change to a file for the person to accept or reject, rather than making it yourself: they see the text and your replacement side by side, and accepting makes the change. path: absolute, or relative to your working folder. quote.exact: the text to replace, exactly as the file has it; prefix and suffix pick between copies of it. replacement: what replaces it, as the file should have it (empty to delete it). body: an optional comment saying why. You hear whether it was accepted.";
const ADD: &str = "Leave the person a comment on a file, on the text you quote, to point them at something (\"I cut this paragraph; check the transition\"). path: absolute, or relative to your working folder. quote.exact: the text, exactly as the file has it; prefix and suffix (a few words before and after) pick between copies of it. body: Markdown.";

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Deserialize)]
struct ReplyArgs {
    thread: String,
    body: String,
}

#[derive(Deserialize)]
struct ResolveArgs {
    thread: String,
    #[serde(default)]
    note: Option<String>,
}

#[derive(Deserialize)]
struct QuoteArgs {
    exact: String,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    suffix: String,
}

#[derive(Deserialize)]
struct SuggestArgs {
    path: String,
    quote: QuoteArgs,
    replacement: String,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct AddArgs {
    path: String,
    quote: QuoteArgs,
    body: String,
}

/// The agent's working folder; an error if it's gone.
fn folder(factory: &Factory, agent: &str) -> Result<std::path::PathBuf, String> {
    let inner = factory.lock();
    let me = super::caller(&inner.config, agent)?;
    Ok(crate::runner::places(&factory.home, &inner.config, me, &[]).folder)
}

fn list(factory: &Arc<Factory>, agent: &str, args: ListArgs) -> Result<ToolResult, String> {
    let status = args.status.as_deref().map(str::trim).filter(|status| !status.is_empty()).unwrap_or("open");
    if !["open", "resolved", "all"].contains(&status) {
        return Err(format!("status is open, resolved or all; got \"{status}\"."));
    }
    let path = match args.path.as_deref().map(str::trim).filter(|path| !path.is_empty()) {
        Some(path) => {
            let asked = std::path::Path::new(path);
            let path = if asked.is_absolute() { asked.to_path_buf() } else { folder(factory, agent)?.join(asked) };
            Some(std::fs::canonicalize(&path).unwrap_or(path).display().to_string())
        }
        None => None,
    };
    let text = crate::annotations::agent_list(factory, agent, path.as_deref(), status)?;
    Ok(ToolResult::text(text, json!({})))
}

fn reply(factory: &Arc<Factory>, agent: &str, args: ReplyArgs) -> Result<ToolResult, String> {
    if args.body.trim().is_empty() {
        return Err("body is empty: say what your reply is.".to_owned());
    }
    let text = crate::annotations::agent_reply(factory, agent, &args.thread, &args.body, false)?;
    Ok(ToolResult::text(text, json!({"thread": args.thread.trim()})))
}

fn resolve(factory: &Arc<Factory>, agent: &str, args: ResolveArgs) -> Result<ToolResult, String> {
    let note = args.note.unwrap_or_default();
    let text = crate::annotations::agent_reply(factory, agent, &args.thread, &note, true)?;
    Ok(ToolResult::text(text, json!({"thread": args.thread.trim()})))
}

fn add(factory: &Arc<Factory>, agent: &str, args: AddArgs) -> Result<ToolResult, String> {
    let folder = folder(factory, agent)?;
    let quote = Quote::new(&args.quote.exact, &args.quote.prefix, &args.quote.suffix);
    let text = crate::annotations::agent_add(factory, agent, &folder, &args.path, &quote, &args.body, None)?;
    Ok(ToolResult::text(text, json!({"path": args.path})))
}

fn suggest(factory: &Arc<Factory>, agent: &str, args: SuggestArgs) -> Result<ToolResult, String> {
    let folder = folder(factory, agent)?;
    let quote = Quote::new(&args.quote.exact, &args.quote.prefix, &args.quote.suffix);
    let text = crate::annotations::agent_add(factory, agent, &folder, &args.path, &quote, &args.body, Some(&args.replacement))?;
    Ok(ToolResult::text(text, json!({"path": args.path})))
}

pub(crate) struct Comments;

impl Extension for Comments {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        // The comments' event and the tools' descriptions say it all.
        Vec::new()
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let thread = json!({"type": "string"});
        let list_parameters = json!({
            "type": "object",
            "properties": {"path": {"type": "string"}, "status": {"type": "string", "enum": ["open", "resolved", "all"]}},
            "additionalProperties": false,
        });
        let reply_parameters = json!({
            "type": "object",
            "required": ["thread", "body"],
            "properties": {"thread": thread, "body": {"type": "string"}},
            "additionalProperties": false,
        });
        let resolve_parameters = json!({
            "type": "object",
            "required": ["thread"],
            "properties": {"thread": thread, "note": {"type": "string"}},
            "additionalProperties": false,
        });
        let add_parameters = json!({
            "type": "object",
            "required": ["path", "quote", "body"],
            "properties": {
                "path": {"type": "string"},
                "quote": {
                    "type": "object",
                    "required": ["exact"],
                    "properties": {"exact": {"type": "string"}, "prefix": {"type": "string"}, "suffix": {"type": "string"}},
                    "additionalProperties": false,
                },
                "body": {"type": "string"},
            },
            "additionalProperties": false,
        });
        let suggest_parameters = json!({
            "type": "object",
            "required": ["path", "quote", "replacement"],
            "properties": {
                "path": {"type": "string"},
                "quote": {
                    "type": "object",
                    "required": ["exact"],
                    "properties": {"exact": {"type": "string"}, "prefix": {"type": "string"}, "suffix": {"type": "string"}},
                    "additionalProperties": false,
                },
                "replacement": {"type": "string"},
                "body": {"type": "string"},
            },
            "additionalProperties": false,
        });
        vec![
            tool("comments_list", "List comments", LIST, list_parameters, typed_handler(factory, agent, list)),
            tool("comments_reply", "Reply to a comment", REPLY, reply_parameters, typed_handler(factory, agent, reply)),
            tool("comments_resolve", "Resolve a comment", RESOLVE, resolve_parameters, typed_handler(factory, agent, resolve)),
            tool("comments_add", "Comment on a file", ADD, add_parameters, typed_handler(factory, agent, add)),
            tool("comments_suggest", "Suggest a change", SUGGEST, suggest_parameters, typed_handler(factory, agent, suggest)),
        ]
    }
}
