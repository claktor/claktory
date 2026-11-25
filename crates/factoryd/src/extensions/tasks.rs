//! Tasks: the agent sees and stops background tasks (bash's
//! run_in_background), its own or, with `see_all`, every agent's. The owner
//! of a stopped task gets the usual event saying it finished, stopped by
//! whom. Factory also watches for tasks that run long: once one has run
//! `task_alert_hours`, every admin agent is told, once.

use std::io::{Read, Seek, SeekFrom};
use std::sync::{Arc, Weak};

use factory_agent::{Tool, ToolResult};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, OptionKind, OptionSpec, PromptSection, factory_tool, switch};
use crate::config::{AgentConfig, Config};
use crate::factory::{Factory, Inner, host_event};
use crate::store::TaskRow;
use crate::transcript::now_ms;

pub(crate) const ID: &str = "tasks";

/// Finished tasks are listed for this long.
const RECENT_MS: i64 = 24 * 60 * 60 * 1000;
/// At most this many finished tasks are listed.
const RECENT_SHOWN: usize = 20;
/// The panel shows this many of a task's last lines of output…
const TAIL_LINES: usize = 20;
/// …read from at most this much of the end of its log.
const TAIL_BYTES: u64 = 16 * 1024;

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Tasks",
    icon: "terminal",
    description: "Lets it see and stop its background tasks.",
    default_enabled: true,
    options: &[OptionSpec {
        id: "see_all",
        label: "See all agents' tasks",
        description: "It can also see and stop other agents' tasks.",
        kind: OptionKind::Switch,
        default_on: false,
        includes_project: false,
    }],
};

const PROMPT: &str = "## Tasks

Background tasks (bash with run_in_background) run until they end or are stopped, even across restarts. tasks_list shows your running ones, one line each (`all`: recently finished ones too); tasks_stop stops one, and you get the usual event when it ends. Stop servers and watchers once you no longer need them.
";

const SEE_ALL: &str = "You can see and stop every agent's tasks: `agent` (an id, or \"*\" for everyone) in tasks_list, and `agent` in tasks_stop. The owner of a task you stop is told who stopped it.
";

/// Sees and stops every agent's tasks.
fn sees_all(agent: &AgentConfig) -> bool {
    switch(agent, ID, "see_all") || is_admin(agent)
}

fn is_admin(agent: &AgentConfig) -> bool {
    super::enabled(agent, super::agents::ID) && switch(agent, super::agents::ID, "admin")
}

pub(crate) struct Tasks;

impl Extension for Tasks {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, agent: &AgentConfig) -> Vec<PromptSection> {
        let text = if sees_all(agent) { format!("{PROMPT}{SEE_ALL}") } else { PROMPT.to_owned() };
        vec![PromptSection::static_text(text)]
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        // Optional parameters are the ones left out of `required` (see `super::tool`).
        let list = json!({
            "type": "object",
            "properties": {"agent": {"type": "string"}, "all": {"type": "boolean"}},
            "additionalProperties": false,
        });
        let stop = json!({
            "type": "object",
            "required": ["task"],
            "properties": {"task": {"type": "string"}, "agent": {"type": "string"}},
            "additionalProperties": false,
        });
        vec![
            factory_tool(factory, agent, "tasks_list", "List tasks", LIST, list, run_list),
            factory_tool(factory, agent, "tasks_stop", "Stop task", STOP, stop, run_stop),
        ]
    }

    fn view(&self, factory: &Factory, inner: &Inner, agent: &str) -> Option<v1::ExtensionView> {
        let me = inner.config.agent(agent)?;
        let whose = (!sees_all(me)).then_some(agent);
        let tasks = match listed(factory, whose, true, now_ms()) {
            Ok(tasks) => tasks,
            Err(error) => {
                tracing::error!(%error, "could not read tasks for the panel");
                Vec::new()
            }
        };
        let tasks = tasks
            .into_iter()
            .map(|task| v1::BackgroundTask {
                // The log on disk holds whatever the command printed.
                log_tail: factory.redaction.redact(&log_tail(&task.log)),
                id: task.task,
                agent_id: task.agent,
                command: task.command,
                started: task.started,
                ended: task.ended.unwrap_or_default(),
                running: task.ended.is_none(),
                status: task.status.unwrap_or_default(),
                log_path: task.log,
            })
            .collect();
        Some(v1::ExtensionView {
            extension_id: ID.to_owned(),
            agent_id: agent.to_owned(),
            title: MANIFEST.name.to_owned(),
            body: Some(v1::extension_view::Body::Tasks(v1::Tasks { tasks })),
        })
    }
}

/// The last lines of a task's log; empty when it can't be read.
fn log_tail(path: &str) -> String {
    let read = || -> std::io::Result<Vec<u8>> {
        let mut file = std::fs::File::open(path)?;
        let length = file.seek(SeekFrom::End(0))?;
        let from = length.saturating_sub(TAIL_BYTES);
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        // A cut first line is dropped, unless it's all there is.
        if from > 0
            && let Some(newline) = bytes.iter().position(|byte| *byte == b'\n')
            && newline + 1 < bytes.len()
        {
            bytes.drain(..=newline);
        }
        Ok(bytes)
    };
    let Ok(bytes) = read() else { return String::new() };
    let text = String::from_utf8_lossy(&bytes);
    let text = text.trim_end_matches('\n');
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n")
}

/// Shows the open Tasks panel as it is now, if one is open: tasks started or
/// ended, and running ones' output grew. Sent only when it changed.
pub(crate) fn refresh_view(factory: &Factory) {
    let mut inner = factory.lock();
    if inner.view.as_ref().is_none_or(|view| view.extension_id != ID) {
        return;
    }
    let view = factory.current_view(&inner);
    factory.set_view(&mut inner, view);
}

const LIST: &str = "Background tasks, one line each: your running ones; all: recently finished ones too. agent: another agent's id, or \"*\" for everyone's, if you can see all tasks.";
const STOP: &str = "Stop a background task (its whole process group), e.g. task: \"task-3\". agent: whose, if not yours and you can see all tasks. You get the usual event when it ends.";

#[derive(Deserialize)]
struct ListArgs {
    #[serde(default)]
    agent: Option<String>,
    #[serde(default)]
    all: Option<bool>,
}

fn run_list(factory: &Factory, asker: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: ListArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let whose = whose(factory, asker, args.agent.as_deref())?;
    let all = args.all.unwrap_or(false);
    let now = now_ms();
    let tasks = listed(factory, whose.as_deref(), all, now)?;
    let shown: Vec<_> = tasks
        .iter()
        .map(|task| {
            let mut shown =
                json!({"task": task.task, "agent": task.agent, "command": short(&task.command), "running": task.ended.is_none()});
            if let Some(status) = &task.status {
                shown["status"] = json!(status);
            }
            shown
        })
        .collect();
    let details = json!({"agent": whose.as_deref().unwrap_or("*"), "tasks": shown});
    if tasks.is_empty() {
        let what = if all { "No background tasks ran lately" } else { "No background tasks are running" };
        let said = match whose.as_deref() {
            Some(agent) if agent != asker => format!("{what} for {agent}."),
            _ => format!("{what}."),
        };
        return Ok(ToolResult::text(said, details));
    }
    Ok(ToolResult::text(tasks.iter().map(|task| line(task, now)).collect::<Vec<_>>().join("\n"), details))
}

/// Whose tasks `asker` asks about: Some(agent), or None for everyone's.
fn whose(factory: &Factory, asker: &str, agent: Option<&str>) -> Result<Option<String>, String> {
    let inner = factory.lock();
    let me = super::caller(&inner.config, asker)?;
    let agent = agent.map(str::trim).filter(|agent| !agent.is_empty() && *agent != asker);
    match agent {
        None => Ok(Some(asker.to_owned())),
        Some(_) if !sees_all(me) => Err("You can see only your own tasks.".to_owned()),
        Some("*") => Ok(None),
        Some(agent) => find_agent(&inner.config, agent).map(|found| Some(found.id.clone())),
    }
}

fn find_agent<'a>(config: &'a Config, wanted: &str) -> Result<&'a AgentConfig, String> {
    config
        .agent(wanted)
        .or_else(|| config.agents.iter().find(|agent| agent.name.eq_ignore_ascii_case(wanted)))
        .ok_or_else(|| format!("No agent has the id \"{wanted}\"."))
}

/// Running tasks, newest first, then (with `all`) recently finished ones.
fn listed(factory: &Factory, agent: Option<&str>, all: bool, now: i64) -> Result<Vec<TaskRow>, String> {
    let rows = factory.store.tasks(agent, now - RECENT_MS).map_err(|error| format!("Couldn't read tasks: {error}"))?;
    let (running, finished): (Vec<TaskRow>, Vec<TaskRow>) = rows.into_iter().partition(|task| task.ended.is_none());
    let mut tasks = running;
    if all {
        tasks.extend(finished.into_iter().take(RECENT_SHOWN));
    }
    Ok(tasks)
}

/// "task-3 builder: npm run dev — running 26h", or "— exit 0 after 4m".
fn line(task: &TaskRow, now: i64) -> String {
    let state = match (&task.status, task.ended) {
        (Some(status), Some(ended)) => format!("{status} after {}", span(ended - task.started)),
        _ => format!("running {}", span(now - task.started)),
    };
    format!("{} {}: {} — {state}", task.task, task.agent, short(&task.command))
}

/// The command's first line, cut to 80 characters.
fn short(command: &str) -> String {
    let first = command.lines().next().unwrap_or_default().trim();
    let cut: String = first.chars().take(80).collect();
    if cut.len() < command.trim().len() { format!("{cut}…") } else { cut }
}

/// "<1m", "4m", "26h", "3d".
fn span(ms: i64) -> String {
    let minutes = ms.max(0) / 60_000;
    match minutes {
        0 => "<1m".to_owned(),
        1..60 => format!("{minutes}m"),
        60..2880 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

#[derive(Deserialize)]
struct StopArgs {
    task: String,
    #[serde(default)]
    agent: Option<String>,
}

fn run_stop(factory: &Factory, asker: &str, args: serde_json::Value) -> Result<ToolResult, String> {
    let args: StopArgs = serde_json::from_value(args).map_err(|error| error.to_string())?;
    let owner = match whose(factory, asker, args.agent.as_deref())? {
        Some(owner) => owner,
        None => return Err("Say whose task: agent is an agent's id.".to_owned()),
    };
    let by = factory.agent_name(asker);
    let task = args.task.trim();
    stop(factory, &owner, task, &by)?;
    let said = if owner == asker {
        format!("Stopping {task}; you'll get an event when it ends.")
    } else {
        format!("Stopping {owner}'s {task}; {owner} gets an event when it ends.")
    };
    Ok(ToolResult::text(said, json!({"task": task, "agent": owner})))
}

/// Stops the agent's running task, noting who did; Err says why it can't.
pub(crate) fn stop(factory: &Factory, agent: &str, task: &str, by: &str) -> Result<(), String> {
    let running = factory.store.tasks(Some(agent), i64::MAX).map_err(|error| format!("Couldn't read tasks: {error}"))?;
    let Some(found) = running.into_iter().find(|row| row.task == task) else {
        return Err(format!("{agent} has no running task \"{task}\"."));
    };
    factory.lock().stopping.insert((agent.to_owned(), task.to_owned()), by.to_owned());
    factory_agent::tools::bash::stop_group(found.pid);
    Ok(())
}

/// Tells admin agents of tasks that have run long, once each, for as long
/// as Factory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    let mut checked = tokio::time::Instant::now();
    loop {
        let Some(every) = factory.upgrade().map(|factory| factory.task_check_every()) else { return };
        // Short naps, so a change of `every` applies soon, and an open
        // panel follows its tasks' output.
        tokio::time::sleep(every.min(std::time::Duration::from_secs(1))).await;
        let Some(strong) = factory.upgrade() else { return };
        refresh_view(&strong);
        drop(strong);
        if checked.elapsed() < every {
            continue;
        }
        checked = tokio::time::Instant::now();
        let Some(factory) = factory.upgrade() else { return };
        if let Err(error) = alert(&factory) {
            tracing::error!(%error, "could not check for long-running tasks");
        }
    }
}

fn alert(factory: &Arc<Factory>) -> rusqlite::Result<()> {
    let hours = factory.lock().config.task_alert_hours;
    let hours_ms = i64::try_from(hours).unwrap_or(i64::MAX / 3_600_000).saturating_mul(3_600_000);
    let now = now_ms();
    for task in factory.store.long_tasks(now.saturating_sub(hours_ms))? {
        let (owner, admins) = {
            let inner = factory.lock();
            let owner = inner.config.agent(&task.agent).map_or_else(|| task.agent.clone(), |agent| agent.name.clone());
            let admins: Vec<String> = inner.config.agents.iter().filter(|agent| is_admin(agent)).map(|agent| agent.id.clone()).collect();
            (owner, admins)
        };
        let text = format!(
            "{owner}'s {} `{}` has been running {} (log: {}).",
            task.task,
            short(&task.command),
            span(now - task.started),
            task.log
        );
        for admin in admins {
            let event = host_event(String::new());
            factory.deliver_event(&admin, text.clone(), event);
        }
        factory.store.task_alerted(&task.agent, &task.task)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_task_is_one_terse_line() {
        let task = TaskRow {
            agent: "builder".into(),
            task: "task-3".into(),
            command: "npm run dev\nmore".into(),
            pid: 1,
            log: "/l".into(),
            started: 0,
            ended: None,
            status: None,
        };
        assert_eq!(line(&task, 26 * 3_600_000), "task-3 builder: npm run dev… — running 26h");
        let done = TaskRow { ended: Some(4 * 60_000), status: Some("exit 0".into()), command: "cargo test".into(), ..task };
        assert_eq!(line(&done, 0), "task-3 builder: cargo test — exit 0 after 4m");
    }
}
