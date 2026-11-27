//! The backend on Windows (windows.md Part I, §§1–2): where Claktory lives
//! (`dirs`), what the backend sets up there before agents' commands run,
//! and the token the person's app reaches it with (`app_token`). The
//! commands themselves, their jobs and the private files are
//! `factory_agent::windows`.

pub mod app_token;
pub mod dirs;
pub mod listener;

use std::path::Path;

use factory_agent::windows::{job, private};

/// The file the commands' jobs are recorded in, inside the home.
pub fn jobs_file(home: &Path) -> std::path::PathBuf {
    dirs::windows_records(home).join("jobs.json")
}

/// The home itself, private before anything is written into it, as it's
/// 0700 on a Mac (Fugo 45257, condition 1): only this user and SYSTEM, and
/// everything made inside inherits that, so a LocalAppData moved somewhere
/// others can read (another drive, a share) doesn't expose the vault,
/// factory.db or the app's token. A home that's a link elsewhere, or
/// another account's, is refused. Quick when it's private already.
pub fn prepare_home(home: &Path) -> std::io::Result<()> {
    private::create_dir_all(home)
}

/// Before the backend runs any command: the private home, its private
/// records folder, and the jobs an earlier backend left, so their commands
/// are found again (`tools::bash::group_alive` and `stop_group` on Windows)
/// rather than lost.
pub fn start(home: &Path) -> std::io::Result<()> {
    prepare_home(home)?;
    private::create_dir_all(&dirs::windows_records(home))?;
    job::keep_records(jobs_file(home))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_makes_the_records_and_reads_them_again() {
        let home = std::env::temp_dir().join(format!("claktory-start-{}", std::process::id()));
        start(&home).expect("starts");
        assert!(factory_agent::windows::system::is_private_path(&home).unwrap(), "the home is private");
        assert!(dirs::windows_records(&home).is_dir());
        assert!(jobs_file(&home).is_file(), "the records are saved even when there are none");
        start(&home).expect("starts again over its own records");
        std::fs::remove_dir_all(&home).unwrap();
    }
}
