//! Where a Factory keeps its files and which port it serves.
//!
//! Everything lives in the Factory's home: settings, agents' conversations, and
//! the workspace folder that holds the projects. A dev Factory (`--dev`) has its
//! own home and port, so agents can build and test Factory while the real one
//! keeps running.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::{Path, PathBuf};

const PORT: u16 = 7420;
const DEV_PORT: u16 = 7421;

#[derive(Clone, Debug)]
pub struct Home {
    root: PathBuf,
}

impl Home {
    pub fn new(root: PathBuf) -> Home {
        Home { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn config_path(&self) -> PathBuf {
        self.root.join("config.json")
    }

    /// Factory's records; see `store`.
    pub fn db_path(&self) -> PathBuf {
        self.root.join("factory.db")
    }

    /// Where the app's state was kept before the store.
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }

    /// Where working agents were kept before the store.
    pub fn runs_path(&self) -> PathBuf {
        self.root.join("runs.json")
    }

    pub fn auth_path(&self) -> PathBuf {
        self.root.join("auth.json")
    }

    pub fn agent_dir(&self, agent_id: &str) -> PathBuf {
        self.root.join("agents").join(agent_id)
    }

    /// The installed backend, which updates replace.
    pub fn backend(&self) -> PathBuf {
        self.root.join("bin").join(program("factoryd"))
    }

    /// The backend before the last update, to go back to if it fails.
    pub fn previous_backend(&self) -> PathBuf {
        self.root.join("bin").join("factoryd.previous")
    }

    /// An update on trial; see `update`.
    pub fn trial_path(&self) -> PathBuf {
        self.root.join("bin").join("trial.json")
    }

    /// The last update that failed and was undone.
    pub fn update_failed_path(&self) -> PathBuf {
        self.root.join("update-failed.json")
    }

    /// Published builds: `<version>/` folders, and `latest` naming the newest.
    pub fn releases(&self) -> PathBuf {
        self.root.join("releases")
    }

    /// Agents outside any project work here; each project is a folder inside.
    pub fn workspace(&self) -> PathBuf {
        self.root.join("workspace")
    }
}

pub struct Instance {
    pub home: Home,
    pub addr: SocketAddr,
}

/// The real Factory lives in `~/.factory`, the dev one in `~/.factory-dev`.
pub fn instance(dev: bool, user_home: &Path) -> Instance {
    instance_in(dev, user_home.join(if dev { ".factory-dev" } else { ".factory" }))
}

/// The file a program of Claktory's (`factoryd`, a helper) is in: its name,
/// with `.exe` on Windows, where Windows, Defender and the firewall know a
/// program by it; as it is elsewhere.
pub fn program(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Where this computer's Claktory keeps its files, the dev one's with
/// `dev`: `~/.factory` (`~/.factory-dev`), and on Windows
/// `%LOCALAPPDATA%\Claktory\factory` (`factory-dev`). None when there's no
/// home folder to put it in. FACTORY_HOME, where it's honoured, goes over
/// it.
pub fn instance_root(dev: bool) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        crate::windows::dirs::factory_home(dev).ok()
    }
    #[cfg(not(windows))]
    {
        Some(std::env::home_dir()?.join(if dev { ".factory-dev" } else { ".factory" }))
    }
}

/// The Factory whose files are at `root`, on the real or the dev port.
pub fn instance_in(dev: bool, root: PathBuf) -> Instance {
    // Local-only for now; remote access comes later.
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), if dev { DEV_PORT } else { PORT });
    Instance { home: Home::new(root), addr }
}

#[cfg(test)]
mod tests {
    #[cfg_attr(windows, allow(unused_imports, reason = "its one test isn't Windows's"))]
    use super::*;

    #[cfg(not(windows))]
    #[test]
    fn the_instances_live_in_the_home_folder_on_their_own_ports() {
        let user_home = std::env::home_dir().expect("a home folder");
        for (dev, name, port) in [(false, ".factory", PORT), (true, ".factory-dev", DEV_PORT)] {
            let root = instance_root(dev).expect("a root");
            assert_eq!(root, user_home.join(name));
            let found = instance_in(dev, root.clone());
            assert_eq!((found.home.root(), found.addr.port()), (root.as_path(), port));
            let old = instance(dev, &user_home);
            assert_eq!((old.home.root(), old.addr), (root.as_path(), found.addr));
        }
    }
}
