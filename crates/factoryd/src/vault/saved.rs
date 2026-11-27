//! Secrets agents saved from a command's output (bash's `save_as`, Fugo's
//! save-as-secret.md, Bruno 45874): for Matt's rule of one broad
//! credential, a key made with it for the job, the broad one revoked and
//! given up. The output goes straight into the vault, granted only to the
//! agent that saved it; no card asks, since the key is narrower than what
//! the person already gave, but the person sees it in History
//! (NEEDS_YOU_RECORD_KIND_SECRET_SAVED: "Pericolo saved Play deploy key, a
//! new secret made with Google Cloud owner") and in
//! Settings → Secrets as saved by that agent, where they can delete it.
//!
//! What's checked before the command runs, so a key made at a service is
//! never lost to a refusal after: the vault is open, the name is free, the
//! agent reads nothing from outside (not contained, not tainted by a pipe),
//! it's within 5 saves an hour and 20 a day, and none of the secrets the
//! command runs with is bound to sites or given to it only to fill. After
//! it ran, the value is never another secret's (equal to one, holding one
//! of 8 characters or more, or held in one), and the new secret asks each
//! time when any of those secrets does: a copy never sheds a limit.

use std::collections::HashMap;

use factory_agent::secrets::SaveAs;
use rusqlite::{OptionalExtension as _, params};
use zeroize::Zeroizing;

use super::{Value, Vault, VaultError, check_name, valid_variable};
use crate::factory::Factory;
use crate::transcript::now_ms;

/// The table of secrets agents saved.
pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS secrets_saved (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    agent TEXT NOT NULL,
    -- The secret, by id, and its name then.
    secret TEXT NOT NULL,
    name TEXT NOT NULL,
    -- Its description, in the agent's words.
    description TEXT NOT NULL DEFAULT '',
    -- The secrets the command ran with, by their names then, as JSON.
    parents TEXT NOT NULL DEFAULT '[]',
    at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS secrets_saved_agent ON secrets_saved (agent, at);
CREATE INDEX IF NOT EXISTS secrets_saved_at ON secrets_saved (at);
";

/// Saves an agent may make in an hour, and in a day.
pub(crate) const PER_HOUR: i64 = 5;
pub(crate) const PER_DAY: i64 = 20;

/// The most a saved secret holds.
pub(crate) const MOST: usize = factory_agent::secrets::SAVE_MOST;

/// The tool, and the key of its result's details naming the record.
pub(crate) const TOOL: &str = "bash";
pub(crate) const DETAIL: &str = "secret_saved";

/// What's said when the value is another secret's.
const ANOTHER: &str = "That's the value of another secret; ask the person instead. Nothing was saved.";

/// What the secrets a command runs with pass on to one saved from it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Inherited {
    /// One of them asks the person each time it's used.
    pub ask: bool,
}

impl Vault {
    /// Whether no secret is named `name` (any case, as names are matched).
    fn name_free(&self, name: &str) -> Result<bool, VaultError> {
        let inner = self.lock();
        let db = Self::db(&inner)?;
        let taken: i64 = db.query_row("SELECT COUNT(*) FROM secrets WHERE name = ?1 COLLATE NOCASE", [name], |row| row.get(0))?;
        Ok(taken == 0)
    }

    /// What `agent`'s secrets named `parents` pass on to a secret saved
    /// from a command they run in, or why nothing may be saved from it:
    /// one bound to sites, or given to it only to fill. A name it doesn't
    /// hold is left to the command's own check, which says so.
    pub(crate) fn inherited(&self, agent: &str, parents: &[String]) -> Result<Inherited, String> {
        let inner = self.lock();
        let (Ok(key), Ok(db)) = (Self::grant_key(&inner), Self::db(&inner)) else { return Err(locked()) };
        let mut inherited = Inherited::default();
        for asked in parents {
            let found: Option<(String, String, bool, String)> = db
                .query_row("SELECT id, name, ask, sites FROM secrets WHERE name = ?1 COLLATE NOCASE", [asked.trim()], crate::store::row4)
                .optional()
                .map_err(|error| error.to_string())?;
            let Some((id, name, ask, sites)) = found else { continue };
            let Ok(Some(grant)) = Self::grant_of(&inner, &key, &id, agent) else { continue };
            if grant.fill_only || !sites.is_empty() {
                return Err(format!(
                    "{name} is for filling into its sites in Claktory's browser, so nothing made with it can be saved: ask the person instead."
                ));
            }
            inherited.ask |= ask;
        }
        Ok(inherited)
    }

    /// Whether `value` is another secret's: equal to one, holding one of 8
    /// characters or more, or held in one.
    fn holds_another(value: &[u8], others: &[factory_agent::SecretVariable]) -> bool {
        let contains = |haystack: &[u8], needle: &[u8]| needle.len() <= haystack.len() && haystack.windows(needle.len()).any(|window| window == needle);
        others.iter().any(|other| {
            let other = other.bytes();
            !other.is_empty()
                && (other.as_slice() == value
                    || (other.len() >= factory_agent::secrets::MIN_REDACTED && contains(value, &other))
                    || (value.len() >= factory_agent::secrets::MIN_REDACTED && contains(&other, value)))
        })
    }
}

fn locked() -> String {
    "The vault is locked, so nothing can be saved: try again once it's open (Settings → Secrets). The command didn't run.".to_owned()
}

/// Whether `agent` may save `save` from a command run with `parents`,
/// before the command runs; why not, in words for it.
pub(crate) fn check(factory: &Factory, agent: &str, save: &SaveAs, parents: &[String]) -> Result<Inherited, String> {
    let not_run = "The command didn't run.";
    if factory.lock().config.agent(agent).is_none() {
        return Err("You're no longer in this Claktory.".to_owned());
    }
    if factory.pipes.contained(agent) || !factory.pipes.tainted(agent).is_empty() {
        return Err(format!(
            "You've read what came from outside this Claktory, so you can't save a command's output as a secret: what you'd save might be a stranger's key. Ask the person instead. {not_run}"
        ));
    }
    if !factory.vault.is_open() {
        return Err(locked());
    }
    let name = check_name(&save.name).map_err(|error| format!("{error} {not_run}"))?;
    match factory.vault.name_free(&name) {
        Ok(true) => {}
        Ok(false) => return Err(format!("There's a secret named {name} already: choose another name. {not_run}")),
        Err(error) => return Err(format!("{error} {not_run}")),
    }
    if !valid_variable(save.variable.trim()) {
        return Err(format!("\"{}\" isn't a variable name: letters, digits and _, not starting with a digit. {not_run}", save.variable.trim()));
    }
    if save.description.trim().is_empty() {
        return Err(format!("Describe the secret (description): what it is, what it's for and what made it. {not_run}"));
    }
    let now = now_ms();
    let (hour, day) = {
        let db = factory.store.db();
        let count = |since: i64| -> i64 {
            db.query_row("SELECT COUNT(*) FROM secrets_saved WHERE agent = ?1 AND at >= ?2", params![agent, since], |row| row.get(0))
                .unwrap_or(i64::MAX)
        };
        (count(now - 3_600_000), count(now - 86_400_000))
    };
    if hour >= PER_HOUR || day >= PER_DAY {
        return Err(format!(
            "You've saved {} secrets in the last {}, the most there can be: try again later, or ask the person. {not_run}",
            if hour >= PER_HOUR { PER_HOUR } else { PER_DAY },
            if hour >= PER_HOUR { "hour" } else { "day" }
        ));
    }
    factory.vault.inherited(agent, parents).map_err(|why| format!("{why} {not_run}"))
}

/// Saves `value`, a command's whole output, as `save`, granted to `agent`
/// alone: the record's id, which History shows it by; or why not, and
/// nothing is kept.
pub(crate) fn store(factory: &Factory, agent: &str, save: &SaveAs, parents: &[String], value: &[u8]) -> Result<String, String> {
    let nothing = "Nothing was saved: if the command made a key at a service, revoke it there, then run it again.";
    let inherited = check(factory, agent, save, parents).map_err(|why| format!("{} {nothing}", why.trim_end_matches(" The command didn't run.")))?;
    if value.len() > MOST {
        return Err(format!("The output is {} bytes, more than a secret holds ({MOST}). {nothing}", value.len()));
    }
    if value.iter().all(u8::is_ascii_whitespace) {
        return Err("The command printed nothing to save, so nothing was saved.".to_owned());
    }
    let mut others = factory.vault.every_variable();
    others.extend(factory.broker.hidden(&factory.vault));
    if Vault::holds_another(value, &others) {
        return Err(ANOTHER.to_owned());
    }
    let variable = save.variable.trim().to_owned();
    let value = match std::str::from_utf8(value) {
        // A variable holds the line printed, without its line ending; a
        // file holds exactly what was printed.
        Ok(text) if !save.file => Value { variable, value: Zeroizing::new(text.trim_end_matches(['\n', '\r']).to_owned()), file: false, binary: false },
        Ok(text) => Value { variable, value: Zeroizing::new(text.to_owned()), file: true, binary: false },
        Err(_) if save.file => {
            use base64::Engine as _;
            let encoded = base64::engine::general_purpose::STANDARD.encode(value);
            Value { variable, value: Zeroizing::new(encoded), file: true, binary: true }
        }
        Err(_) => return Err(format!("The output isn't text, so it can only be saved as a file: set file to true. {nothing}")),
    };
    let name = save.name.trim();
    let description = save.description.trim();
    let id = factory.vault.create_secret(name, description, std::slice::from_ref(&value), agent).map_err(|error| format!("{error} {nothing}"))?;
    let finish = || -> Result<(), VaultError> {
        factory.vault.grant(&id, agent, agent)?;
        if inherited.ask {
            factory.vault.set_ask_each_time(&id, true)?;
        }
        Ok(())
    };
    if let Err(error) = finish() {
        let _ = factory.vault.delete_secret(&id);
        factory.vault_changed();
        return Err(format!("{error} {nothing}"));
    }
    let record = record(factory, agent, &id, name, description, parents);
    factory.vault_changed();
    tracing::info!(agent, secret = %id, "an agent saved a command's output as a secret");
    Ok(record.map(|id| id.to_string()).unwrap_or_default())
}

/// Keeps that `agent` saved the secret, for History and the limits: its
/// id; None when it couldn't be kept (logged).
fn record(factory: &Factory, agent: &str, secret: &str, name: &str, description: &str, parents: &[String]) -> Option<i64> {
    let db = factory.store.db();
    let kept = db.execute(
        "INSERT INTO secrets_saved (agent, secret, name, description, parents, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![agent, secret, name, description, serde_json::to_string(parents).unwrap_or_default(), now_ms()],
    );
    match kept {
        Ok(_) => Some(db.last_insert_rowid()),
        Err(error) => {
            tracing::error!(%error, "couldn't keep a saved secret for the history");
            None
        }
    }
}

/// A secret an agent saved, as kept.
#[derive(Clone, Debug)]
pub(crate) struct Saved {
    pub id: i64,
    pub agent: String,
    pub name: String,
    pub description: String,
    pub parents: Vec<String>,
    pub at: i64,
}

impl Saved {
    /// Claktory's sentence for it, with the agent's name now, what was
    /// saved first (Trish 50375): "Pericolo saved Play deploy key, a new
    /// secret made with Google Cloud owner".
    pub(crate) fn title(&self, names: &HashMap<String, String>) -> String {
        let agent = names.get(&self.agent).cloned().unwrap_or_else(|| self.agent.clone());
        if self.parents.is_empty() {
            format!("{agent} saved {}, a new secret", self.name)
        } else {
            format!("{agent} saved {}, a new secret made with {}", self.name, factory_agent::text::list_and(&self.parents))
        }
    }
}

/// The secrets saved from `since` to before `before`, newest first, at
/// most `limit`.
pub(crate) fn between(db: &rusqlite::Connection, since: i64, before: i64, limit: i64) -> rusqlite::Result<Vec<Saved>> {
    let mut statement = db.prepare(
        "SELECT id, agent, name, description, parents, at FROM secrets_saved
         WHERE at >= ?1 AND at < ?2 ORDER BY at DESC, id DESC LIMIT ?3",
    )?;
    statement
        .query_map(params![since, before, limit], |row| {
            let parents: String = row.get(4)?;
            Ok(Saved {
                id: row.get(0)?,
                agent: row.get(1)?,
                name: row.get(2)?,
                description: row.get(3)?,
                parents: serde_json::from_str(&parents).unwrap_or_default(),
                at: row.get(5)?,
            })
        })?
        .collect()
}

/// The record a bash call with save_as made, from its entry.
pub(crate) fn saved_of(call: &factory_protocol::v1::ToolCall) -> Option<String> {
    if call.name != TOOL {
        return None;
    }
    let details: serde_json::Value = serde_json::from_str(&call.details).ok()?;
    details.get(DETAIL)?.as_str().map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use super::super::tests::{temp, value, vault};
    use super::*;

    fn variable(value: &str) -> factory_agent::SecretVariable {
        factory_agent::SecretVariable {
            secret: "Other".to_owned(),
            variable: "OTHER".to_owned(),
            value: Zeroizing::new(value.to_owned()),
            file: false,
            public: false,
            binary: false,
        }
    }

    #[test]
    fn a_saved_secret_asks_each_time_when_one_it_was_made_with_does_and_none_comes_from_a_site_secret() {
        let dir = temp();
        let vault = vault(&dir);
        vault.create(Some("correct horse battery"), false).expect("create");
        let gcloud = vault.create_secret("Google Cloud", "Owner.", &[value("GOOGLE_TOKEN", "gc-owner-token-0123")], "").expect("secret");
        let aws = vault.create_secret("AWS", "Deploys.", &[value("AWS_SECRET", "wJalrXUtnFEMI-K7MDENG")], "").expect("secret");
        let shop = vault.create_secret("Shop", "Sign-in.", &[value("PASSWORD", "hunter2-hunter2")], "").expect("secret");
        let mail = vault.create_secret("Mail", "Sign-in.", &[value("PASSWORD", "mail-password-01")], "").expect("secret");
        for id in [&gcloud, &aws, &shop] {
            vault.grant(id, "bo", "").expect("grant");
        }
        vault.set_ask_each_time(&aws, true).expect("ask");
        vault.set_sites(&shop, &["https://shop.example".to_owned()]).expect("sites");
        vault.grant_fill_only(&mail, "bo", "").expect("fill only");
        let names = |names: &[&str]| names.iter().map(|name| (*name).to_owned()).collect::<Vec<_>>();

        assert_eq!(vault.inherited("bo", &names(&["Google Cloud"])), Ok(Inherited { ask: false }));
        assert_eq!(vault.inherited("bo", &names(&["google cloud", " aws "])), Ok(Inherited { ask: true }), "names in any case");
        assert_eq!(vault.inherited("lea", &names(&["AWS"])), Ok(Inherited { ask: false }), "not hers: the command's own check says so");
        for site in ["Shop", "Mail"] {
            let refused = vault.inherited("bo", &names(&["Google Cloud", site])).expect_err("a site's secret");
            assert!(refused.starts_with(&format!("{site} is for filling into its sites")), "{refused}");
        }
    }

    #[test]
    fn a_value_is_another_secrets_when_equal_to_it_holding_it_or_held_in_it() {
        let others = [variable("gc-owner-token-0123"), variable("short")];
        for value in ["gc-owner-token-0123", "Bearer gc-owner-token-0123\n", "owner-token", "short"] {
            assert!(Vault::holds_another(value.as_bytes(), &others), "{value}");
        }
        for value in ["a-new-narrow-key-99", "token", "shorter", "Bearer short-lived"] {
            assert!(!Vault::holds_another(value.as_bytes(), &others), "{value}");
        }
    }
}
