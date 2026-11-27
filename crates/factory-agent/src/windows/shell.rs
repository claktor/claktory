//! The bash tool's shell on Windows: Git for Windows' Bash, a private copy
//! Claktory ships beside its backend, so `bash` means Bash, run by its absolute path, never whichever `bash` is on PATH (WSL's
//! `bash.exe` in System32 among them).
//!
//! Layout, in the home's `tools` (beside `bin`, where factoryd.exe is), or
//! where `FACTORY_WINDOWS_TOOLS` says for development and QA:
//!
//! ```text
//! tools\git.txt                  the Git folder in use, such as git-2.51.0;
//!                                `git` when there's no such file
//! tools\<git>\usr\bin\bash.exe   Bash and the Unix tools it comes with
//! tools\<git>\ucrt64\bin         git and its native helpers (mingw64
//!                                before Git for Windows 2.50)
//! tools\bin                      rg.exe, jq.exe
//! ```
//!
//! Each Git goes in a folder of its own, named by its version, so an update
//! puts a new one beside the one agents' commands may still be running
//! from, and only the pointer changes.
//!
//! Commands run as on a Mac: `bash --noprofile --norc -e -o pipefail -c
//! SCRIPT`, so no startup file (the person's or Git's) runs, and the agent's
//! script is passed through the environment rather than the command line,
//! so no Windows quoting or Cygwin wildcard expansion can change it.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use zeroize::Zeroizing;

use super::child::Child;
use super::spawn::{self, Command, Output};

/// Where the tools are when they aren't beside the backend.
pub const TOOLS_VARIABLE: &str = "FACTORY_WINDOWS_TOOLS";
/// The variables the wrappers read their script and status file from, and
/// unset before the script runs.
const SCRIPT: &str = "CLAKTORY_SCRIPT";
const STATUS: &str = "CLAKTORY_STATUS";
const LOG: &str = "CLAKTORY_LOG";

/// Runs the script and records its exit code, as `tools::bash`'s
/// TASK_WRAPPER does elsewhere. Bash opens the log itself, to append:
/// Cygwin can't write to the handle it inherits, which is opened to append
/// only (FILE_APPEND_DATA without FILE_WRITE_DATA, since Cygwin writes at an
/// offset of its own), while with `>>` it moves to the end before each
/// write, so its output still lands after whatever else the log got
/// meanwhile, a cut of its middle included.
const TASK_WRAPPER: &str = concat!(
    r#"s=$CLAKTORY_SCRIPT; f=$CLAKTORY_STATUS; l=$CLAKTORY_LOG; unset CLAKTORY_SCRIPT CLAKTORY_STATUS CLAKTORY_LOG; "#,
    r#"exec >>"$l" 2>&1 || { echo 125 > "$f"; exit 125; }; "#,
    r#""$BASH" --noprofile --norc -e -o pipefail -c "$s"; echo $? > "$f""#
);

/// As TASK_WRAPPER, with the output going through the filter (its program
/// and arguments are the wrapper's own arguments) that hides secrets'
/// values, as FILTERED_TASK_WRAPPER does elsewhere. The filter, a native
/// program, writes the log through the handle it inherits.
const FILTERED_TASK_WRAPPER: &str = concat!(
    r#"s=$CLAKTORY_SCRIPT; f=$CLAKTORY_STATUS; unset CLAKTORY_SCRIPT CLAKTORY_STATUS CLAKTORY_LOG; "#,
    r#"( unset CLAKTORY_REDACT; exec "$BASH" --noprofile --norc -e -o pipefail -c "$s" ) 2>&1 | "$@"; echo "${PIPESTATUS[0]}" > "$f""#
);

/// Runs a monitor's check as a Mac's `bash -o pipefail -c` does: without
/// -e, its stderr and stdout together.
const CHECK_WRAPPER: &str = r#"s=$CLAKTORY_SCRIPT; unset CLAKTORY_SCRIPT; exec 2>&1; exec "$BASH" --noprofile --norc -o pipefail -c "$s""#;

/// Runs the script with its output on pipes.
const PIPED_WRAPPER: &str = r#"s=$CLAKTORY_SCRIPT; unset CLAKTORY_SCRIPT; exec "$BASH" --noprofile --norc -e -o pipefail -c "$s""#;

/// Variables that would make Bash run something before the script, or
/// change how it runs it, whoever set them.
const DROPPED: &[&str] =
    &["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS", "CDPATH", "GLOBIGNORE", "PROMPT_COMMAND", "MSYS", "MSYS2_PATH_TYPE", "CYGWIN"];

/// The private shell and tools.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shell {
    pub bash: PathBuf,
    /// Put first on PATH, in this order.
    pub path: Vec<PathBuf>,
    /// The environment its native tools are built for, as MSYSTEM names
    /// it: UCRT64 in Git for Windows from 2.50, MINGW64 before.
    pub msystem: &'static str,
}

/// The file in `tools` that names the Git folder in use.
pub const GIT_POINTER: &str = "git.txt";

/// Whether `name` may be a Git folder's: `git`, or `git-` and a version,
/// with nothing that could reach another folder.
pub fn is_git_folder(name: &str) -> bool {
    name == "git"
        || name.strip_prefix("git-").is_some_and(|version| {
            !version.is_empty()
                && version.len() <= 64
                && version.bytes().all(|byte| byte.is_ascii_alphanumeric() || b"._-".contains(&byte))
                && !version.starts_with('.')
        })
}

/// The Git folder `tools` uses: the one GIT_POINTER names, or `git`.
pub fn git_folder(tools: &Path) -> String {
    std::fs::read_to_string(tools.join(GIT_POINTER))
        .ok()
        .map(|text| text.trim().to_owned())
        .filter(|name| is_git_folder(name))
        .unwrap_or_else(|| "git".to_owned())
}

impl Shell {
    /// The shell in `tools`, if it's all there.
    pub fn in_folder(tools: &Path) -> std::io::Result<Shell> {
        let git = tools.join(git_folder(tools));
        let bash = git.join("usr").join("bin").join("bash.exe");
        if !bash.is_file() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Claktory's shell isn't installed: {} is missing. Reinstall Claktory to put it back.", bash.display()),
            ));
        }
        let (native, msystem) = if git.join("ucrt64").join("bin").is_dir() { ("ucrt64", "UCRT64") } else { ("mingw64", "MINGW64") };
        let path = vec![git.join("usr").join("bin"), git.join(native).join("bin"), tools.join("bin")];
        Ok(Shell { bash, path, msystem })
    }

    /// The shell this backend uses: in FACTORY_WINDOWS_TOOLS, or in the
    /// home's `tools`, beside the `bin` the backend runs from. Looked for
    /// once.
    pub fn get() -> std::io::Result<&'static Shell> {
        static SHELL: OnceLock<Result<Shell, String>> = OnceLock::new();
        let found = SHELL.get_or_init(|| {
            let tools = match std::env::var_os(TOOLS_VARIABLE) {
                Some(folder) => PathBuf::from(folder),
                None => std::env::current_exe()
                    .map_err(|error| error.to_string())?
                    .parent()
                    .and_then(Path::parent)
                    .map(|home| home.join("tools"))
                    .unwrap_or_default(),
            };
            Shell::in_folder(&tools).map_err(|error| error.to_string())
        });
        found.as_ref().map_err(|message| std::io::Error::new(std::io::ErrorKind::NotFound, message.clone()))
    }

    /// A command's whole environment: `inherited` (the backend's own,
    /// which it has already cleared of anything private), without what
    /// would change how Bash starts; the private tools first on PATH; HOME
    /// as the person's profile folder, as Git Bash has it; then `extra`
    /// (the agent's variables and its secrets), which win, except that the
    /// private tools stay first on PATH.
    pub fn environment(
        &self,
        inherited: impl IntoIterator<Item = (OsString, OsString)>,
        extra: &[(String, Zeroizing<String>)],
    ) -> std::io::Result<Vec<(OsString, OsString)>> {
        let mut environment: Vec<(OsString, OsString)> = Vec::new();
        for (name, value) in inherited {
            let upper = name.to_string_lossy().to_uppercase();
            if DROPPED.contains(&upper.as_str()) || [SCRIPT, STATUS, LOG].contains(&upper.as_str()) {
                continue;
            }
            set(&mut environment, name, value);
        }
        set(&mut environment, "MSYSTEM".into(), self.msystem.into());
        if get(&environment, "HOME").is_none()
            && let Some(profile) = get(&environment, "USERPROFILE")
        {
            set(&mut environment, "HOME".into(), profile);
        }
        if get(&environment, "LANG").is_none() && get(&environment, "LC_ALL").is_none() {
            set(&mut environment, "LANG".into(), "C.UTF-8".into());
        }
        for (name, value) in extra {
            set(&mut environment, name.into(), value.as_str().into());
        }
        // The private tools go first on whichever PATH won, the agent's
        // own too: `bash` in a command must be Git Bash's, never WSL's
        // launcher in System32.
        let mut path: Vec<PathBuf> = self.path.clone();
        if let Some(theirs) = get(&environment, "PATH") {
            path.extend(std::env::split_paths(&theirs).filter(|entry| !self.path.contains(entry)));
        }
        let joined = std::env::join_paths(path).map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidInput, error))?;
        set(&mut environment, "PATH".into(), joined);
        Ok(environment)
    }

    /// Starts `script` as a background task does: output appended to `log`
    /// (`file` is it, open to append), its exit code written to `log` with
    /// the extension `status`. `filter`: the program and arguments its
    /// output goes through.
    pub fn spawn_task(
        &self,
        cwd: &Path,
        script: &str,
        log: &Path,
        file: std::fs::File,
        mut environment: Vec<(OsString, OsString)>,
        filter: Option<&[OsString]>,
    ) -> std::io::Result<(Child, u32)> {
        set(&mut environment, SCRIPT.into(), script.into());
        set(&mut environment, STATUS.into(), unix_style(&log.with_extension("status")));
        set(&mut environment, LOG.into(), unix_style(log));
        let wrapper = if filter.is_some() { FILTERED_TASK_WRAPPER } else { TASK_WRAPPER };
        let mut args: Vec<OsString> = ["--noprofile", "--norc", "-c", wrapper, "factory-task"].map(OsString::from).to_vec();
        args.extend(filter.unwrap_or_default().iter().cloned());
        spawn::spawn(Command { program: &self.bash, args, environment, cwd, stdout: Output::File(file), stderr: None })
    }

    /// Starts `script` with its stdout and stderr on pipes.
    pub fn spawn_piped(&self, cwd: &Path, script: &str, mut environment: Vec<(OsString, OsString)>) -> std::io::Result<(Child, u32)> {
        set(&mut environment, SCRIPT.into(), script.into());
        let args: Vec<OsString> = ["--noprofile", "--norc", "-c", PIPED_WRAPPER, "factory"].map(OsString::from).to_vec();
        spawn::spawn(Command { program: &self.bash, args, environment, cwd, stdout: Output::Pipe, stderr: Some(Output::Pipe) })
    }

    /// Starts a monitor's check, `script`, in a job of its own, its output
    /// and errors together on one pipe (`Child::stdout`).
    pub fn spawn_check(&self, cwd: &Path, script: &str, mut environment: Vec<(OsString, OsString)>) -> std::io::Result<(Child, u32)> {
        set(&mut environment, SCRIPT.into(), script.into());
        let args: Vec<OsString> = ["--noprofile", "--norc", "-c", CHECK_WRAPPER, "factory-check"].map(OsString::from).to_vec();
        spawn::spawn(Command { program: &self.bash, args, environment, cwd, stdout: Output::Pipe, stderr: None })
    }
}

/// A Windows path as Bash takes it best: forward slashes (`C:/Users/…`).
pub fn unix_style(path: &Path) -> OsString {
    path.to_string_lossy().replace('\\', "/").into()
}

/// The variable `name`, matched as Windows does, ignoring case.
fn get(environment: &[(OsString, OsString)], name: &str) -> Option<OsString> {
    environment.iter().find(|(key, _)| same(key, name)).map(|(_, value)| value.clone())
}

/// Sets `name`, replacing it in whatever case it was.
fn set(environment: &mut Vec<(OsString, OsString)>, name: OsString, value: OsString) {
    let text = name.to_string_lossy().into_owned();
    environment.retain(|(key, _)| !same(key, &text));
    environment.push((name, value));
}

fn same(key: &OsStr, name: &str) -> bool {
    key.to_string_lossy().eq_ignore_ascii_case(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shell() -> Shell {
        Shell {
            bash: PathBuf::from(r"C:\T\git\usr\bin\bash.exe"),
            path: vec![PathBuf::from(r"C:\T\git\usr\bin"), PathBuf::from(r"C:\T\bin")],
            msystem: "MINGW64",
        }
    }

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs.iter().map(|(name, value)| (OsString::from(name), OsString::from(value))).collect()
    }

    #[test]
    fn the_environment_puts_the_tools_first_and_drops_what_changes_bash() {
        let inherited =
            vars(&[("Path", r"C:\Windows\System32"), ("BASH_ENV", "evil.sh"), ("USERPROFILE", r"C:\Users\fred"), ("CLAKTORY_SCRIPT", "x")]);
        let extra = [("FACTORY_AGENT_ID".to_owned(), Zeroizing::new("rivet".to_owned()))];
        let environment = shell().environment(inherited.clone(), &extra).unwrap();
        assert_eq!(get(&environment, "PATH").unwrap(), OsString::from(r"C:\T\git\usr\bin;C:\T\bin;C:\Windows\System32"));
        assert!(get(&environment, "BASH_ENV").is_none());
        assert!(get(&environment, "CLAKTORY_SCRIPT").is_none());
        assert_eq!(get(&environment, "HOME").unwrap(), OsString::from(r"C:\Users\fred"));
        assert_eq!(get(&environment, "MSYSTEM").unwrap(), OsString::from("MINGW64"));
        assert_eq!(get(&environment, "factory_agent_id").unwrap(), OsString::from("rivet"));
        assert_eq!(environment.iter().filter(|(name, _)| same(name, "PATH")).count(), 1);
        // The agent's PATH replaces the inherited one, but the tools stay
        // first, so its `bash` is never System32's WSL launcher.
        let extra = [("PATH".to_owned(), Zeroizing::new(r"C:\Windows\System32;C:\T\bin;C:\mine".to_owned()))];
        let environment = shell().environment(inherited, &extra).unwrap();
        assert_eq!(get(&environment, "PATH").unwrap(), OsString::from(r"C:\T\git\usr\bin;C:\T\bin;C:\Windows\System32;C:\mine"));
    }

    #[test]
    fn paths_go_to_bash_with_forward_slashes() {
        assert_eq!(unix_style(Path::new(r"C:\Users\fred\task-1.status")), OsString::from("C:/Users/fred/task-1.status"));
    }

    #[test]
    fn the_git_in_use_is_the_one_its_pointer_names_and_only_a_plain_name() {
        let tools = std::env::temp_dir().join(format!("claktory-git-pointer-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tools);
        std::fs::create_dir_all(&tools).unwrap();
        assert_eq!(git_folder(&tools), "git");
        for (written, used) in [
            ("git-2.51.0.windows.1\r\n", "git-2.51.0.windows.1"),
            ("git", "git"),
            ("..", "git"),
            ("git-..\\x", "git"),
            ("git-../x", "git"),
            ("git-", "git"),
            ("C:\\git", "git"),
            ("other", "git"),
        ] {
            std::fs::write(tools.join(GIT_POINTER), written).unwrap();
            assert_eq!(git_folder(&tools), used, "{written:?}");
        }
        let _ = std::fs::remove_dir_all(&tools);
    }

    #[test]
    fn a_missing_shell_says_so() {
        let error = Shell::in_folder(Path::new(r"C:\nowhere")).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
        assert!(error.to_string().contains("Claktory's shell isn't installed"));
    }

    /// The private shell itself, run as the bash tool runs it. Needs
    /// FACTORY_WINDOWS_TOOLS (Git for Windows in `git`, rg and jq in
    /// `bin`): `cargo test -- --ignored` on Windows with it set.
    #[test]
    #[ignore = "needs Git Bash in FACTORY_WINDOWS_TOOLS"]
    fn the_private_shell_runs_scripts_and_tasks_under_their_job() {
        use super::super::job;
        use tokio::io::AsyncReadExt;
        let shell = Shell::get().expect("FACTORY_WINDOWS_TOOLS names Git Bash");
        let folder = std::env::temp_dir().join(format!("claktory-shell-{}", std::process::id()));
        std::fs::create_dir_all(&folder).unwrap();
        // The agent's PATH puts System32 (and WSL's bash.exe) first; the
        // private tools still come before it.
        let system = std::env::var("SystemRoot").unwrap();
        let extra = [("PATH".to_owned(), Zeroizing::new(format!(r"{system}\System32;{system}")))];
        let environment = shell.environment(std::env::vars_os(), &extra).unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            let script =
                r#"uname -s; bash -c 'uname -s'; command -v rg jq git | sed 's/.*\///'; echo "a b" '*' ~ | wc -w; false; echo unreachable"#;
            let (mut child, _) = shell.spawn_piped(&folder, script, environment.clone()).unwrap();
            let mut out = String::new();
            child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
            let status = child.wait().await.unwrap();
            let lines: Vec<&str> = out.lines().collect();
            // Git Bash's uname says MINGW64_NT under UCRT64 too; WSL's Linux.
            let native = |line: &str| line.starts_with("MINGW64_NT") || line.starts_with("UCRT64_NT");
            assert!(native(lines[0]), "{out}");
            assert!(native(lines[1]), "bash on PATH is Git Bash's, never WSL's: {out}");
            assert_eq!(&lines[2..5], ["rg", "jq", "git"], "{out}");
            // Four words: the quoted ones aren't split or expanded, ~ is.
            assert_eq!(lines[5].trim(), "4", "{out}");
            assert_eq!(lines.len(), 6, "-e stops at false: {out}");
            assert_eq!(status.code(), Some(1));

            // A task: its output to a file, its exit code to the status
            // file, and a child it leaves behind keeps it alive until
            // stopped.
            // Opened to append, as the bash tool opens a task's log.
            let log = folder.join("task.log");
            let status_file = folder.join("task.status");
            std::fs::write(&log, "before\n").unwrap();
            let file = std::fs::OpenOptions::new().create(true).append(true).open(&log).unwrap();
            let script = "echo started; sleep 60 & echo $! > child.pid; exit 4";
            let (mut child, pid) = shell.spawn_task(&folder, script, &log, file, environment, None).unwrap();
            child.wait().await.unwrap();
            let written = std::fs::read_to_string(&log).unwrap_or_default();
            assert_eq!(std::fs::read_to_string(&status_file).unwrap().trim(), "4", "log: {written}");
            assert_eq!(written, "before\nstarted\n");
            assert!(job::alive(pid), "the sleep it left still runs: {:?}", job::processes(pid));
            // Cygwin's fork and exec start sleep.exe a moment after its stub.
            let mut seen = Vec::new();
            for _ in 0..50 {
                seen = job::processes(pid);
                if seen.iter().any(|(_, image)| image.ends_with("sleep.exe")) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            assert!(seen.iter().any(|(_, image)| image.ends_with("sleep.exe")), "{seen:?}");
            assert!(job::stop(pid));
            for _ in 0..50 {
                if !job::alive(pid) {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            }
            assert!(!job::alive(pid), "stopped: {:?}", job::processes(pid));
        });
        let _ = std::fs::remove_dir_all(&folder);
    }
}
