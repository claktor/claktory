//! A pipe's bound agent (#250): the liaison made for it lives in the pipe
//! rather than beside it. The app shows it in the pipe's row and view, with
//! no row of its own (Agent.pipe_id), and it goes with the pipe when the
//! pipe is deleted, its conversation kept in the pipe's folder.

use std::path::Path;
use std::sync::Arc;

use super::{Record, change, folder, line};
use crate::config::Config;
use crate::factory::{Factory, host_event};

/// The names a new liaison gets, in turn (Trish's: none of this team's).
const NAMES: &[&str] = &[
    "Kai", "Mira", "Theo", "Nadia", "Lucia", "Jonas", "Amara", "Ravi", "Elena", "Ines", "Malik", "Sofia", "Leo", "Zara", "Mateo", "Aiko",
    "Nils", "Dario", "Leila", "Kofi", "Freya", "Hugo", "Asha", "Emil", "Noor", "Pablo", "Selin", "Idris", "Oskar", "Talia", "Ren", "Esme",
    "Ayo", "Clara", "Joon", "Vera", "Samir", "Lotte", "Ilan", "Rosa", "Tariq", "Anouk", "Dev", "Otto", "Benji", "Lior", "Wren", "Pia",
    "Bao", "Tove",
];

/// What Claktory's liaison prompt starts with (`make_liaison`): how an
/// agent made for a pipe before binding is told apart from one the person
/// only named so.
pub(crate) const LIAISON_PROMPT: &str = "You are the liaison for the pipe \"";

/// The name a new liaison of `record` would get: one of `NAMES` no agent of
/// its project has (the same for the pipe until one is taken, so it doesn't
/// change under the person), else the first with a number ("Kai 2").
pub(crate) fn suggest(config: &Config, record: &Record) -> String {
    let taken = |name: &str| {
        config.agents.iter().any(|agent| agent.project.as_deref() == Some(record.project.as_str()) && agent.name.eq_ignore_ascii_case(name))
    };
    let start = record.id.bytes().fold(0usize, |sum, byte| sum.wrapping_mul(31).wrapping_add(usize::from(byte))) % NAMES.len();
    let order = || NAMES.iter().cycle().skip(start).take(NAMES.len());
    if let Some(name) = order().find(|name| !taken(name)) {
        return (*name).to_owned();
    }
    (2..).flat_map(|number| order().map(move |name| format!("{name} {number}"))).find(|name| !taken(name)).expect("a free name")
}

impl super::Pipes {
    /// The pipe `agent` is bound to, by id; empty for every other agent.
    pub(crate) fn bound_pipe(&self, agent: &str) -> String {
        self.data()
            .pipes
            .values()
            .find(|record| !agent.is_empty() && record.bound == agent)
            .map(|record| record.id.clone())
            .unwrap_or_default()
    }
}

/// At start, once per pipe: binds the liaison Claktory made for a pipe from
/// before binding (its prompt is Claktory's, it's a member, the lead first),
/// and gives one still named "Liaison" a name, telling it.
pub(crate) fn upgrade(factory: &Arc<Factory>) {
    let config = factory.lock().config.clone();
    let mut renames = Vec::new();
    for record in factory.pipes.all() {
        if record.bind_set {
            continue;
        }
        let made_for_it = |id: &String| {
            config.agent(id).is_some_and(|agent| agent.prompt.starts_with(LIAISON_PROMPT)) && factory.pipes.bound_pipe(id).is_empty()
        };
        let bound = if made_for_it(&record.lead) && record.members.contains(&record.lead) {
            Some(record.lead.clone())
        } else {
            record.members.iter().find(|id| made_for_it(id)).cloned()
        };
        change(factory, &record.id, |record| {
            record.bind_set = true;
            if let Some(bound) = &bound {
                record.bound.clone_from(bound);
            }
        });
        if let Some(bound) = bound
            && config.agent(&bound).is_some_and(|agent| agent.name.trim() == "Liaison")
        {
            renames.push((bound, suggest(&config, &record)));
        }
    }
    if renames.is_empty() {
        return;
    }
    let factory = factory.clone();
    tokio::spawn(async move {
        for (id, name) in renames {
            rename(&factory, &id, &name).await;
        }
    });
}

/// Gives the agent its name, and tells it.
async fn rename(factory: &Arc<Factory>, id: &str, name: &str) {
    let renamed = factory
        .edit_config(|config| {
            if let Some(agent) = config.agents.iter_mut().find(|agent| agent.id == id) {
                agent.name = name.to_owned();
            }
            Ok(())
        })
        .await;
    match renamed {
        Ok(()) => factory.deliver_event(id, format!("Your name is now {name}."), host_event(format!("Your name is now {name}"))),
        Err(error) => tracing::warn!(agent = %id, %error, "could not name a pipe's liaison"),
    }
}

/// The pipe is deleted: its bound agent goes with it, its folder (its
/// conversation) moved first into the pipe's folder, `conversation`, where
/// the pipe's inbox and outbox stay too.
pub(crate) fn delete_with(factory: &Arc<Factory>, record: &Record) {
    if record.bound.is_empty() || factory.lock().config.agent(&record.bound).is_none() {
        return;
    }
    let (id, from) = (record.bound.clone(), factory.home.agent_dir(&record.bound));
    let to = folder(factory, record).map(|folder| folder.join("conversation"));
    let factory = factory.clone();
    tokio::spawn(async move {
        if let Some(to) = to
            && let Err(error) = keep(&from, &to)
        {
            tracing::warn!(agent = %id, %error, "could not keep a pipe's liaison's conversation");
        }
        if let Err(error) = factory.delete_agent(&id).await {
            tracing::error!(agent = %id, %error, "could not delete a pipe's liaison");
        }
    });
}

/// Copies `from` into `to`, files and folders, never through a link.
fn keep(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            keep(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), &target)?;
        }
    }
    Ok(())
}

/// Its bound agent left the pipe's members (`before`: the pipe as it was):
/// it isn't the pipe's any more, has its own place in the sidebar again,
/// and is told, with what it has back if it was limited to the pipe.
pub(crate) fn unbind_if_gone(factory: &Arc<Factory>, before: &Record) {
    let Some(record) = factory.pipes.get(&before.id) else { return };
    if record.bound.is_empty() || record.members.contains(&record.bound) {
        return;
    }
    let (name, person) = {
        let inner = factory.lock();
        (inner.config.agent(&record.bound).map(|agent| agent.name.clone()), inner.config.person_capitalized().to_owned())
    };
    let agent = record.bound.clone();
    change(factory, &record.id, |record| record.bound.clear());
    let Some(name) = name else { return };
    line(factory, &record.id, format!("{name} isn't this pipe's liaison any more: it's in the sidebar with your other agents."));
    let summary = format!("{person} took you out of the pipe \"{}\": you're no longer its liaison, and its tools are gone", record.name);
    let mut text = format!("{summary}.");
    if before.quarantined.contains(&agent) {
        text.push_str(&format!(
            " Your usual tools and folders are back from your next step. What you read from the pipe stays in your conversation, so using a secret still needs {person}'s approval each time until it's cleared."
        ));
    }
    factory.deliver_event(&agent, text, host_event(summary));
}
