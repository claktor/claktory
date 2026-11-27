//! Slash commands: what the person can run for an agent by typing "/" in its
//! message box. Built in for now; extensions can add their own later.

use std::path::Path;
use std::sync::Arc;

use factory_protocol::v1;

use crate::config::AgentConfig;
use crate::factory::{Factory, RequestError, transcript_of};
use crate::store::Outcome;
use crate::transcript::{Transcript, now_ms};

struct Command {
    name: &'static str,
    description: &'static str,
    args_hint: &'static str,
}

const BUILT_IN: [Command; 2] = [
    Command { name: "clear", description: "Start a new conversation; this one is archived. Checklist and memory stay.", args_hint: "" },
    Command { name: "compact", description: "Summarize the conversation now and carry on from the summary.", args_hint: "" },
];

/// The commands the person can run for `agent`, in the order to offer them.
pub(crate) fn of_agent(_agent: &AgentConfig) -> Vec<v1::SlashCommand> {
    BUILT_IN
        .iter()
        .map(|command| v1::SlashCommand {
            name: command.name.to_owned(),
            description: command.description.to_owned(),
            args_hint: command.args_hint.to_owned(),
        })
        .collect()
}

impl Factory {
    /// Runs one of the agent's commands for the person.
    pub async fn run_command(self: &Arc<Self>, id: &str, name: &str, args: &str) -> Result<(), RequestError> {
        let Some(agent) = self.lock().config.agent(id).cloned() else { return Err(RequestError::UnknownAgent(id.to_owned())) };
        let name = name.trim().trim_start_matches('/');
        let Some(command) = BUILT_IN.iter().find(|command| command.name == name) else {
            let names: Vec<String> = of_agent(&agent).into_iter().map(|command| format!("/{}", command.name)).collect();
            return Err(RequestError::Command(format!("There's no command /{name}. Commands: {}.", names.join(", "))));
        };
        if command.args_hint.is_empty() && !args.trim().is_empty() {
            return Err(RequestError::Command(format!("/{} takes nothing after it.", command.name)));
        }
        match command.name {
            "clear" => self.clear_conversation(id).await,
            "compact" => self.compact_now(id, &agent.name, false).await.map(|_| ()),
            _ => unreachable!("every built-in command is handled"),
        }
    }

    /// /clear: stops the agent as Stop does, moves its conversation to
    /// `archive/<ms>/` in its folder, and starts it on an empty one. Its
    /// extensions' data (checklist, memory) is the agent's, so it stays.
    pub(crate) async fn clear_conversation(&self, id: &str) -> Result<(), RequestError> {
        let writes = self.lock().agents.entry(id.to_owned()).or_default().writes.clone();
        // No write of the old conversation can land after this.
        let _writing = writes.lock().await;
        {
            let mut inner = self.lock();
            let runtime = inner.agents.entry(id.to_owned()).or_default();
            let dropped = std::mem::take(&mut runtime.queue);
            self.settle(&dropped, Outcome::Dropped);
            self.stop_run(runtime);
            // What the stopped run still reports belongs to the old conversation.
            runtime.epoch += 1;
            runtime.compact = None;
            runtime.doing = None;
            // A fresh start: idle from now, whatever its last run did.
            self.reset_presence(id);
            // It has read no pipe yet.
            crate::pipes::agents::clear_taint(self, id);
            // Nor anything from the web (errand_open's check).
            crate::errands::clear_outside_read(self, id);
            // Its questions went with the old conversation.
            if let Err(error) = crate::extensions::ask::cleared(&self.store, id) {
                tracing::warn!(agent = %id, %error, "could not dismiss its questions");
            }
            // So did Rivet's proposals of app access.
            if let Err(error) = crate::user_apps::cleared(&self.store, id) {
                tracing::warn!(agent = %id, %error, "could not decline its proposals of app access");
            }
            if let Err(error) = crate::apps::access::cleared(&self.store, id) {
                tracing::warn!(agent = %id, %error, "could not decline its proposals of what apps read");
            }
            self.broadcast_workspace(&inner);
        }
        // And the add-secret cards it opened, and what it was finding for
        // the person: the requests it held are theirs again.
        crate::vault::requests::forget_admin(self, id);
        // Its browser sessions were the old conversation's.
        self.browser_forget(id, "agent_changed");
        // The files its images went as go with it.
        let forgotten: Vec<String> = {
            let inner = self.lock();
            let runtime = inner.agents.get(id).and_then(|runtime| runtime.transcript.as_ref());
            runtime.map(|transcript| crate::models::image_digests(transcript.messages()).into_iter().collect()).unwrap_or_default()
        };
        if !forgotten.is_empty() {
            let models = self.models.clone();
            tokio::spawn(async move { models.forget_images(forgotten).await });
        }
        let dir = self.home.agent_dir(id);
        archive(&dir).await?;
        let (transcript, persist) = Transcript::cleared(dir, id.to_owned(), self.old_segments.clone());
        persist.write().await?;
        let mut inner = self.lock();
        if let Some(runtime) = inner.agents.get_mut(id) {
            runtime.transcript = Some(transcript);
        }
        if inner.state.selected_agent.as_deref() == Some(id)
            && let Some(transcript) = transcript_of(&inner, id)
        {
            self.send_transcript(&inner, transcript);
        }
        self.broadcast_workspace(&inner);
        Ok(())
    }

    /// /compact: summarizes an idle agent's conversation now. With `later`
    /// (an admin agent's agents_compact), a working agent's is summarized as
    /// soon as its current work ends, before it reads anything new. True
    /// when it starts now.
    pub(crate) async fn compact_now(self: &Arc<Self>, id: &str, name: &str, later: bool) -> Result<bool, RequestError> {
        self.ensure_loaded(id).await?;
        let mut inner = self.lock();
        let runtime = inner.agents.entry(id.to_owned()).or_default();
        if runtime.transcript.as_ref().is_none_or(|transcript| transcript.messages().is_empty()) && !runtime.running {
            return Err(RequestError::Command("Nothing to compact yet.".to_owned()));
        }
        if runtime.running {
            if !later {
                return Err(RequestError::Command(format!("{name} is working; stop it or wait, then /compact.")));
            }
            // Its run takes it up when what it's doing now ends (`next_job`).
            runtime.compact = Some(factory_agent::compaction::Reason::Requested);
            return Ok(false);
        }
        runtime.compact = Some(factory_agent::compaction::Reason::Requested);
        runtime.stopped = false;
        runtime.running = true;
        tokio::spawn(self.clone().run(id.to_owned()));
        self.broadcast_workspace(&inner);
        Ok(true)
    }
}

/// Moves the conversation's files into a new `archive/<ms>/` folder, with
/// its other versions (`crate::fork`).
async fn archive(dir: &Path) -> std::io::Result<()> {
    let mut files = match tokio::fs::read_dir(dir).await {
        Ok(files) => files,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error),
    };
    let into = dir.join("archive").join(now_ms().to_string());
    let mut made = false;
    while let Some(file) = files.next_entry().await? {
        let name = file.file_name();
        if crate::transcript::log::number(&name).is_some() {
            if !made {
                tokio::fs::create_dir_all(&into).await?;
                made = true;
            }
            tokio::fs::rename(file.path(), into.join(&name)).await?;
        }
    }
    let branches = dir.join(crate::fork::BRANCHES);
    if tokio::fs::try_exists(&branches).await? {
        tokio::fs::create_dir_all(&into).await?;
        tokio::fs::rename(&branches, into.join(crate::fork::BRANCHES)).await?;
    }
    Ok(())
}
