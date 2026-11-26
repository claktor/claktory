//! Newer models (#404): when a provider brings out a newer version of a
//! model agents run on ("everything runs on GPT 6.1, but 6.2 is out"),
//! Rivet is told, once, and asks the person before anything changes.
//! Looked at as Claktory starts, every time it looks whether a model list
//! is due, and after each list is read
//! (`factory_agent::providers::listing::successor` says what's newer).
//!
//! What's news is a model that comes out: one a provider's list has that
//! Claktory hasn't seen in it before. Every model seen is kept in
//! factory.db, so a restart or a list read again doesn't repeat it, and a
//! model the person declined isn't raised again; a later one can be. The
//! first time Claktory sees a provider's models (a new Claktory, a provider
//! just signed in to) they're only noted: a model chosen while a newer one
//! was already out isn't news. Except once: this version's first start on a
//! Claktory whose agents have worked announces the newer models already
//! out for their models, all in one event, so Rivet asks with one card.
//! With no admin agent that can take it, Claktory tells the person itself.

use std::collections::{BTreeMap, HashSet};
use std::fmt::Write as _;
use std::sync::Arc;

use factory_agent::providers::listing;
use factory_protocol::v1;

use crate::config::Config;
use crate::extensions::models as models_ext;
use crate::factory::{Factory, host_event};

pub(crate) const SCHEMA: &str = "
-- The models Claktory has seen in its providers' lists (#404), by their
-- full id (\"<provider>/<model id>\"): one not seen before has come out, and
-- is announced when it's newer than a model agents run on (announced 1).
CREATE TABLE models_seen (
    model TEXT PRIMARY KEY,
    seen_ms INTEGER NOT NULL,
    announced INTEGER NOT NULL DEFAULT 0
);
";

pub(crate) const CATCH_UP: &str = "
-- This version's first start on a Claktory whose agents have worked: the
-- newer models already out are announced once, together, when it first
-- sees its providers' models (never on a new Claktory).
CREATE TABLE models_catch_up (due INTEGER NOT NULL);
INSERT INTO models_catch_up (due) SELECT 1 WHERE EXISTS (SELECT 1 FROM runs) OR EXISTS (SELECT 1 FROM messages);
";

/// An agent on an older version of a newer model.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OnOlder {
    id: String,
    name: String,
    /// Its model's full id and name.
    model: String,
    model_name: String,
}

/// A newer model and the agents on older versions of it.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Newer {
    model: String,
    name: String,
    provider: String,
    agents: Vec<OnOlder>,
}

/// The newer models of those agents run on, from the models Claktory can
/// run now: per provider, the successor of each agent's model.
fn newer(config: &Config, models: &[v1::Model], providers: &[v1::Provider]) -> Vec<Newer> {
    let name_of = |id: &str| models.iter().find(|model| model.id == id).map_or_else(|| id.to_owned(), |model| model.name.clone());
    let mut found: BTreeMap<String, Newer> = BTreeMap::new();
    for agent in config.agents.iter().filter(|agent| !config.archived(agent)) {
        let Some((provider, id)) = agent.model.split_once('/') else { continue };
        let prefix = format!("{provider}/");
        let theirs = models.iter().filter_map(|model| model.id.strip_prefix(&prefix));
        let Some(next) = listing::successor(id, theirs) else { continue };
        let model = format!("{prefix}{next}");
        let provider_name = providers
            .iter()
            .find(|known| known.id == provider)
            .map_or_else(|| listing::name(provider).to_owned(), |known| known.name.clone());
        let entry = found.entry(model.clone()).or_insert_with(|| Newer {
            name: name_of(&model),
            model,
            provider: provider_name,
            agents: Vec::new(),
        });
        entry.agents.push(OnOlder {
            id: agent.id.clone(),
            name: agent.name.clone(),
            model: agent.model.clone(),
            model_name: name_of(&agent.model),
        });
    }
    found.into_values().collect()
}

/// Who hears of it: Rivet (the admin agent proposals go to) when its
/// model can run, and whether it can change models itself.
fn rivet(config: &Config, models: &[v1::Model]) -> Option<(String, bool)> {
    let rivet = crate::proposals::rivet(config)?;
    if config.archived(rivet) || !models.iter().any(|model| model.id == rivet.model) {
        return None;
    }
    let can_set =
        crate::extensions::enabled(rivet, models_ext::ID) && crate::extensions::switch(rivet, models_ext::ID, models_ext::CAN_SET);
    Some((rivet.id.clone(), can_set))
}

/// "Mike, Ana and Lena".
fn names(agents: &[OnOlder]) -> String {
    let names: Vec<&str> = agents.iter().map(|agent| agent.name.as_str()).collect();
    match names.split_last() {
        Some((last, [])) => (*last).to_owned(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    }
}

/// What Rivet is told: it stands alone.
fn event_text(newer: &Newer, person: &str, can_set: bool) -> String {
    let Newer { model, name, provider, agents } = newer;
    let mut text = format!(
        "{provider} has a newer version of a model your agents run on: {name} ({model}).\n\nThese agents run on an older version:\n"
    );
    for agent in agents {
        let _ = writeln!(text, "- {} ({}), on {} ({})", agent.name, agent.id, agent.model_name, agent.model);
    }
    let _ = write!(
        text,
        "\nAsk {person} before anything changes: one ask_person card for this model, naming these agents, asking whether to move them to {name}. "
    );
    if can_set {
        let _ = write!(
            text,
            "If {person} says yes, move each one with models_set, giving only the model ({model}), so each keeps its effort where {name} has it. If {person} dismisses the card or says no, change nothing."
        );
    } else {
        let _ = write!(
            text,
            "You can't change agents' models here (your Models extension's Can change models is off), so the card says how {person} can do it in the app: in each agent's settings, under Model, pick {name}. Change nothing yourself."
        );
    }
    let _ = write!(text, " Never move an agent without {person}'s yes. Claktory tells you about each newer model once.");
    text
}

/// What the person is told when no admin agent can take it.
fn person_text(newer: &Newer) -> String {
    let Newer { name, provider, agents, .. } = newer;
    let (run, them) = if agents.len() == 1 { ("runs", "it") } else { ("run", "them") };
    format!(
        "{provider} has a newer version of a model your agents use: {name}. {} {run} on an older version. To move {them}, open each agent's settings and pick {name} under Model.",
        names(agents)
    )
}

/// What Rivet is told once, at this version's first start: every newer
/// model already out, for one card.
fn catch_up_text(newer: &[Newer], person: &str, can_set: bool) -> String {
    let mut text = "Newer versions of models your agents run on are out, from before Claktory looked for them:\n".to_owned();
    for one in newer {
        let _ = writeln!(text, "\n{} ({}), from {}. These agents run on an older version:", one.name, one.model, one.provider);
        for agent in &one.agents {
            let _ = writeln!(text, "- {} ({}), on {} ({})", agent.name, agent.id, agent.model_name, agent.model);
        }
    }
    let _ = write!(
        text,
        "\nAsk {person} before anything changes, on one ask_person card for them all: each newer model with the agents on it, and which to move. "
    );
    if can_set {
        let _ = write!(
            text,
            "For each model {person} says yes to, move its agents with models_set, giving only the model, so each keeps its effort where the new model has it. Leave the rest, and change nothing if {person} dismisses the card."
        );
    } else {
        let _ = write!(
            text,
            "You can't change agents' models here (your Models extension's Can change models is off), so the card says how {person} can do it in the app: in each agent's settings, under Model, pick the newer model. Change nothing yourself."
        );
    }
    let _ = write!(
        text,
        " Never move an agent without {person}'s yes. From now on, Claktory tells you about each newer model once, as it comes out."
    );
    text
}

/// What the person is told once, at this version's first start, when no
/// admin agent can take it.
fn catch_up_person_text(newer: &[Newer]) -> String {
    let each: Vec<String> = newer
        .iter()
        .map(|one| {
            let run = if one.agents.len() == 1 { "runs" } else { "run" };
            format!("{} from {} ({} {run} on an older version)", one.name, one.provider, names(&one.agents))
        })
        .collect();
    format!(
        "Newer versions of models your agents use are out: {}. To move an agent, open its settings and pick the newer model under Model.",
        each.join("; ")
    )
}

/// The models listed now, as factory.db knows them.
#[derive(Default)]
struct Noted {
    /// Those that came out since the last look: not seen before, of a
    /// provider whose models were seen.
    fresh: HashSet<String>,
    /// At this version's first start on a Claktory whose agents have
    /// worked, the models seen for the first time, newer ones among them
    /// already out: announced once, together.
    catch_up: HashSet<String>,
}

/// Notes the models listed now: what came out since the last look, and
/// what to catch up on. Nothing when factory.db can't say, rather than
/// announcing again.
fn came_out(factory: &Factory, listed: &[String]) -> Noted {
    let provider = |model: &str| model.split_once('/').map_or_else(String::new, |(provider, _)| provider.to_owned());
    let mut db = factory.store.db();
    let noted = (|| -> rusqlite::Result<Noted> {
        let transaction = db.transaction()?;
        let seen: HashSet<String> = {
            let mut statement = transaction.prepare("SELECT model FROM models_seen")?;
            statement.query_map([], |row| row.get(0))?.collect::<Result<_, _>>()?
        };
        // Once there are models to look at.
        let catch_up = !listed.is_empty() && transaction.execute("DELETE FROM models_catch_up", [])? > 0;
        let known_providers: HashSet<String> = seen.iter().map(|model| provider(model)).collect();
        let now = factory_agent::clock::now_ms() as i64;
        let mut noted = Noted::default();
        for model in listed.iter().filter(|model| !seen.contains(*model)) {
            transaction.execute("INSERT OR IGNORE INTO models_seen (model, seen_ms) VALUES (?1, ?2)", rusqlite::params![model, now])?;
            if known_providers.contains(&provider(model)) {
                noted.fresh.insert(model.clone());
            } else if catch_up {
                noted.catch_up.insert(model.clone());
            }
        }
        transaction.commit()?;
        Ok(noted)
    })();
    noted.unwrap_or_else(|error| {
        tracing::warn!(%error, "could not note the models listed");
        Noted::default()
    })
}

/// Notes the models as announced.
fn announced(factory: &Factory, newer: &[Newer]) {
    let db = factory.store.db();
    for one in newer {
        if let Err(error) = db.execute("UPDATE models_seen SET announced = 1 WHERE model = ?1", rusqlite::params![one.model]) {
            tracing::warn!(%error, model = one.model, "could not note a newer model as announced");
        }
    }
}

/// Looks for newer versions of the models agents run on, and announces each
/// new one once: to Rivet, or to the person when no admin agent can take it.
pub(crate) fn check(factory: &Arc<Factory>) {
    let (found, rivet, config, listed) = {
        let inner = factory.lock();
        let found = newer(&inner.config, &inner.models, &inner.providers);
        let listed: Vec<String> = inner.models.iter().map(|model| model.id.clone()).collect();
        (found, rivet(&inner.config, &inner.models), inner.config.clone(), listed)
    };
    let noted = came_out(factory, &listed);
    let (caught_up, found): (Vec<Newer>, Vec<Newer>) = found.into_iter().partition(|one| noted.catch_up.contains(&one.model));
    if !caught_up.is_empty() {
        announced(factory, &caught_up);
        tracing::info!(models = caught_up.len(), "newer models already out, told once");
        match &rivet {
            Some((rivet, can_set)) => {
                let count = caught_up.len();
                let summary = match count {
                    1 => format!("{} is out, newer than a model your agents run on", caught_up[0].name),
                    _ => format!("{count} newer models are out for your agents' models"),
                };
                factory.deliver_event(rivet, catch_up_text(&caught_up, config.person(), *can_set), host_event(summary));
            }
            None => tell_person(factory, &config, catch_up_person_text(&caught_up)),
        }
    }
    for newer in found.into_iter().filter(|one| noted.fresh.contains(&one.model)) {
        announced(factory, std::slice::from_ref(&newer));
        tracing::info!(model = newer.model, agents = newer.agents.len(), "a newer model is out");
        match &rivet {
            Some((rivet, can_set)) => {
                let count = newer.agents.len();
                let summary = format!("{} is out: {count} agent{} on an older version", newer.name, if count == 1 { "" } else { "s" });
                let text = event_text(&newer, config.person(), *can_set);
                factory.deliver_event(rivet, text, host_event(summary));
            }
            None => tell_person(factory, &config, person_text(&newer)),
        }
    }
}

/// No admin agent can take it: Claktory notifies the person itself.
fn tell_person(factory: &Factory, config: &Config, what: String) {
    let delivered = crate::notify::host_alert(factory, config, &what);
    tracing::info!(?delivered, "told the person about a newer model itself: no admin agent can take it");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, name: &str) -> v1::Model {
        v1::Model { id: id.to_owned(), name: name.to_owned(), ..Default::default() }
    }

    fn agent(id: &str, name: &str, model: &str) -> crate::config::AgentConfig {
        serde_json::from_value(serde_json::json!({"id": id, "name": name, "model": model, "effort": "high"})).unwrap()
    }

    #[test]
    fn agents_on_older_versions_are_grouped_by_the_newer_model() {
        let config = Config {
            agents: vec![
                agent("mike", "Mike", "openai-codex/gpt-6.1-sol"),
                agent("ana", "Ana", "openai-codex/gpt-6.0-sol"),
                agent("lena", "Lena", "openai-codex/gpt-6.2-sol"),
                agent("omar", "Omar", "openai-codex/gpt-6.1-luna"),
                agent("ida", "Ida", "anthropic/claude-sonnet-5"),
            ],
            ..Config::default()
        };
        let models = [
            model("openai-codex/gpt-6.0-sol", "GPT-6.0 Sol"),
            model("openai-codex/gpt-6.1-sol", "GPT-6.1 Sol"),
            model("openai-codex/gpt-6.2-sol", "GPT-6.2 Sol"),
            model("openai-codex/gpt-6.1-luna", "GPT-6.1 Luna"),
            model("anthropic/claude-sonnet-5", "Claude Sonnet 5"),
            // Another provider's newer one isn't this one's.
            model("openrouter/anthropic/claude-sonnet-6", "Claude Sonnet 6"),
        ];
        let providers = [v1::Provider { id: "openai-codex".into(), name: "OpenAI Codex".into(), ..Default::default() }];
        let found = newer(&config, &models, &providers);
        assert_eq!(found.len(), 1, "{found:?}");
        let sol = &found[0];
        assert_eq!(
            (sol.model.as_str(), sol.name.as_str(), sol.provider.as_str()),
            ("openai-codex/gpt-6.2-sol", "GPT-6.2 Sol", "OpenAI Codex")
        );
        let agents: Vec<&str> = sol.agents.iter().map(|agent| agent.id.as_str()).collect();
        assert_eq!(agents, ["mike", "ana"], "not Lena (on it), not Omar (luna is a sibling)");

        let text = event_text(sol, "Matt", true);
        assert_eq!(
            text,
            "OpenAI Codex has a newer version of a model your agents run on: GPT-6.2 Sol (openai-codex/gpt-6.2-sol).\n\n\
             These agents run on an older version:\n\
             - Mike (mike), on GPT-6.1 Sol (openai-codex/gpt-6.1-sol)\n\
             - Ana (ana), on GPT-6.0 Sol (openai-codex/gpt-6.0-sol)\n\n\
             Ask Matt before anything changes: one ask_person card for this model, naming these agents, asking whether to move them to GPT-6.2 Sol. \
             If Matt says yes, move each one with models_set, giving only the model (openai-codex/gpt-6.2-sol), so each keeps its effort where GPT-6.2 Sol has it. \
             If Matt dismisses the card or says no, change nothing. Never move an agent without Matt's yes. Claktory tells you about each newer model once."
        );
        let text = event_text(sol, "Matt", false);
        assert!(
            text.contains("in each agent's settings, under Model, pick GPT-6.2 Sol. Change nothing yourself.")
                && !text.contains("models_set")
        );
        let catch_up = catch_up_text(std::slice::from_ref(sol), "Matt", true);
        assert!(
            catch_up.starts_with(
                "Newer versions of models your agents run on are out, from before Claktory looked for them:\n\n\
                 GPT-6.2 Sol (openai-codex/gpt-6.2-sol), from OpenAI Codex. These agents run on an older version:\n\
                 - Mike (mike), on GPT-6.1 Sol (openai-codex/gpt-6.1-sol)\n- Ana (ana), on GPT-6.0 Sol (openai-codex/gpt-6.0-sol)\n\n\
                 Ask Matt before anything changes, on one ask_person card for them all"
            ),
            "{catch_up}"
        );
        assert_eq!(
            catch_up_person_text(std::slice::from_ref(sol)),
            "Newer versions of models your agents use are out: GPT-6.2 Sol from OpenAI Codex (Mike and Ana run on an older version). To move an agent, open its settings and pick the newer model under Model."
        );
        assert_eq!(
            person_text(sol),
            "OpenAI Codex has a newer version of a model your agents use: GPT-6.2 Sol. Mike and Ana run on an older version. To move them, open each agent's settings and pick GPT-6.2 Sol under Model."
        );
    }
}
