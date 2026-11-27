//! The agents whose browser has a profile, in
//! `<home>/browser-profiles.json` (this user's only, written whole): an
//! agent is listed before its browser first starts and taken off once its
//! profile is deleted, so a profile is deleted however its agent went (its
//! Browser extension turned off, the agent deleted, or either while
//! Claktory was stopped): `super::sweep` deletes the profile of each one
//! listed that doesn't browse any more.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const FILE: &str = "browser-profiles.json";

#[derive(Default, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    agents: Vec<String>,
}

fn path(home: &Path) -> PathBuf {
    home.join(FILE)
}

/// The agents listed: none when there's no file or it doesn't read.
pub(crate) fn list(home: &Path) -> Vec<String> {
    let Ok(bytes) = std::fs::read(path(home)) else { return Vec::new() };
    match serde_json::from_slice::<Saved>(&bytes) {
        Ok(saved) => saved.agents,
        Err(error) => {
            tracing::warn!(%error, "the list of agents' browsers didn't read");
            Vec::new()
        }
    }
}

/// Lists `agent`, if it isn't yet.
pub(crate) fn add(home: &Path, agent: &str) -> std::io::Result<()> {
    let mut agents = list(home);
    if agents.iter().any(|one| one == agent) {
        return Ok(());
    }
    agents.push(agent.to_owned());
    agents.sort();
    save(home, agents)
}

/// Takes `agent` off the list.
pub(crate) fn remove(home: &Path, agent: &str) -> std::io::Result<()> {
    let mut agents = list(home);
    let before = agents.len();
    agents.retain(|one| one != agent);
    if agents.len() == before {
        return Ok(());
    }
    save(home, agents)
}

fn save(home: &Path, agents: Vec<String>) -> std::io::Result<()> {
    if agents.is_empty() {
        return match std::fs::remove_file(path(home)) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let text = serde_json::to_vec_pretty(&Saved { agents }).unwrap_or_default();
    crate::remote::write_private(&path(home), &text)
}

#[cfg(test)]
mod tests {
    use super::{add, list, path, remove};

    #[test]
    fn an_agent_is_listed_once_until_its_profile_goes() {
        let home = std::env::temp_dir().join(format!("claktory-browser-profiles-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        assert!(list(&home).is_empty());
        add(&home, "yukako").unwrap();
        add(&home, "koichi").unwrap();
        add(&home, "yukako").unwrap();
        assert_eq!(list(&home), ["koichi", "yukako"]);
        remove(&home, "yukako").unwrap();
        assert_eq!(list(&home), ["koichi"]);
        remove(&home, "koichi").unwrap();
        assert!(!path(&home).exists(), "no file once none is left");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
