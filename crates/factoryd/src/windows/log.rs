//! The supervisor's log on Windows. On a Mac launchd hands the supervisor
//! `factoryd.log` as its output, and on Linux systemd does; the sign-in
//! task has nothing like it, and starts the supervisor in a hidden console
//! no one reads. So there the supervisor opens the log itself and makes it
//! its standard output and error, which the backends it starts inherit
//! (`factory_agent::windows::spawn::spawn_helper`).

use std::os::windows::io::{AsRawHandle, IntoRawHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE, SetStdHandle};

/// Where the supervisor's output goes as it starts.
#[derive(Debug, PartialEq, Eq)]
pub enum Output {
    /// Whoever started it chose a file or a pipe (a test, a script's `>>`): left as it is.
    Chosen,
    /// None, a console or NUL: from now on `log`.
    Log,
}

/// Makes `log`, opened to append, the standard output and error of this
/// process from now on, unless they already go to a file or a pipe.
pub fn to_log(log: &Path) -> std::io::Result<Output> {
    // SAFETY: only reads this process's standard handle.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if chosen(output) {
        return Ok(Output::Chosen);
    }
    // Appending only (FILE_APPEND_DATA without FILE_WRITE_DATA), shared, so
    // `log_file::rotate` can copy it aside and empty it as on a Mac.
    let file = std::fs::OpenOptions::new().append(true).create(true).open(log)?;
    let handle = file.as_raw_handle();
    for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: an open handle of ours, kept open below for the life of the process.
        if unsafe { SetStdHandle(which, handle) } == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    // The process's output now: it stays open until the process ends.
    let _ = file.into_raw_handle();
    Ok(Output::Log)
}

fn chosen(handle: HANDLE) -> bool {
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return false;
    }
    // SAFETY: only asks what kind of handle an open one is.
    let kind = unsafe { GetFileType(handle) };
    kind == FILE_TYPE_DISK || kind == FILE_TYPE_PIPE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    const CHILD: &str = "CLAKTORY_LOG_TEST_CHILD";

    /// Run by the tests below as a process of its own: what the supervisor does as it starts.
    #[test]
    fn child_takes_its_log() {
        let Some(log) = std::env::var_os(CHILD) else { return };
        let output = to_log(Path::new(&log)).expect("to the log");
        let appends = crate::windows::fs::appends_only(&std::io::stdout()).expect("its output");
        println!("OUTPUT {output:?} APPENDS {appends}");
        eprintln!("AND ITS ERRORS");
        let started = Command::new("cmd").args(["/C", "echo", "FROM A CHILD"]).status().expect("a child");
        assert!(started.success());
    }

    fn run(log: &Path, stdout: Stdio) -> std::process::Output {
        let exe = std::env::current_exe().expect("test binary");
        Command::new(exe)
            .args(["windows::log::tests::child_takes_its_log", "--exact", "--nocapture", "--test-threads=1"])
            .env(CHILD, log)
            .stdout(stdout)
            .stderr(Stdio::null())
            .output()
            .expect("the child")
    }

    fn folder(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("claktory-log-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        dir
    }

    #[test]
    fn started_with_no_output_it_writes_to_the_log_and_so_do_its_children() {
        let dir = folder("none");
        let log = dir.join("factoryd.log");
        std::fs::write(&log, "an earlier run\n").expect("an earlier log");
        let child = run(&log, Stdio::null());
        assert!(child.status.success());
        let written = std::fs::read_to_string(&log).expect("the log");
        assert!(written.starts_with("an earlier run\n"), "appended, not replaced: {written}");
        assert!(written.contains("OUTPUT Log APPENDS true"), "{written}");
        assert!(written.contains("AND ITS ERRORS"), "{written}");
        assert!(written.contains("FROM A CHILD"), "{written}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn output_its_starter_chose_is_kept() {
        let dir = folder("chosen");
        let log = dir.join("factoryd.log");
        let child = run(&log, Stdio::piped());
        assert!(child.status.success());
        let said = String::from_utf8_lossy(&child.stdout);
        assert!(said.contains("OUTPUT Chosen"), "{said}");
        assert!(said.contains("FROM A CHILD"), "{said}");
        assert!(!log.exists(), "no log made");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

