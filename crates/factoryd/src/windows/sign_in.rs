//! The sign-in task on Windows: what a Mac's LaunchAgent is. A task in
//! Task Scheduler, this user's own, that starts the supervisor
//! (`bin\factoryd-supervisor.exe supervise`, `supervisor_copy`) when they
//! sign in to Windows, and that `factoryd install --start` runs to start it
//! now. Only `factoryd install` writes it (`windows::install`); nothing else
//! knows its shape.
//!
//! - Its name has the user's SID in it, `\Claktory\Claktory <SID>`
//!   (`\Claktory\Claktory Dev <SID>` for the dev Claktory): Task
//!   Scheduler's names are the whole computer's, and each user of it has
//!   their own Claktory.
//! - It runs as this user, only while they're signed in (an interactive
//!   token: no password kept), at normal priority (a task's default is
//!   below normal, which would slow every agent's build), with no time
//!   limit, on battery too, one at a time, and again a minute after a
//!   failure, three times.
//! - It runs the supervisor through `conhost.exe --headless`, so the
//!   supervisor has a console no one sees: no window at sign-in, and
//!   Windows Terminal, when it's the default terminal, isn't opened for it.
//! - Its command and arguments name the home through `%LOCALAPPDATA%` and
//!   `%SystemRoot%`, which Task Scheduler expands as it runs them, so the
//!   task's definition is plain ASCII: `schtasks /Query /XML` answers in
//!   the console's code page, which can't say every user's folder name,
//!   and what it says has to match what was written.
//! - Whether it starts at sign-in is the task's own Enabled. A disabled
//!   task can't be run either: `run` enables it for the moment.
//!
//! It's driven with Windows' own `schtasks.exe`, which a user may use on
//! their own tasks without an administrator.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::home::Home;

/// The folder its tasks go in, in Task Scheduler.
const FOLDER: &str = r"\Claktory";

/// The task for this user's Claktory, or the dev one's.
pub fn name(dev: bool, sid: &str) -> String {
    format!(r"{FOLDER}\{} {sid}", if dev { "Claktory Dev" } else { "Claktory" })
}

/// What a task runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub command: String,
    pub arguments: String,
    pub working_directory: String,
}

impl Action {
    /// Runs `supervisor` (as `dev` or not) headless, in `home`; each path
    /// written as `written` says.
    pub fn supervising(supervisor: &str, home: &str, dev: bool) -> Action {
        Action {
            command: r"%SystemRoot%\System32\conhost.exe".to_owned(),
            arguments: format!("--headless \"{supervisor}\"{} supervise", if dev { " --dev" } else { "" }),
            working_directory: home.to_owned(),
        }
    }
}

/// A path as the task names it: through `%LOCALAPPDATA%` when it's in this
/// user's LocalAppData and that variable says where that is, as it does
/// in the task's environment; as it is otherwise (a test's home).
pub fn written(path: &Path) -> String {
    let local = super::dirs::local_app_data().ok();
    let variable = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    let same = |a: &Path, b: &Path| a.as_os_str().eq_ignore_ascii_case(b.as_os_str());
    if let (Some(local), Some(variable)) = (local, variable)
        && same(&local, &variable)
        && let Some(rest) = strip_prefix_ignoring_case(path, &local)
    {
        return format!(r"%LOCALAPPDATA%\{}", rest.display());
    }
    path.display().to_string()
}

fn strip_prefix_ignoring_case<'a>(path: &'a Path, prefix: &Path) -> Option<&'a Path> {
    let mut components = path.components();
    for want in prefix.components() {
        let have = components.next()?;
        if !have.as_os_str().eq_ignore_ascii_case(want.as_os_str()) {
            return None;
        }
    }
    let rest = components.as_path();
    (!rest.as_os_str().is_empty()).then_some(rest)
}

/// A sign-in task: its name, and what it should run.
#[derive(Clone, Debug)]
pub struct Task {
    pub name: String,
    pub action: Action,
    schtasks: PathBuf,
}

/// A task as Task Scheduler has it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registered {
    /// Whether it starts at sign-in (and can be run).
    pub enabled: bool,
    /// What it runs, if it runs one program.
    pub runs: Option<Action>,
}

impl Task {
    /// The task that runs `home`'s supervisor, named for this user.
    pub fn for_home(home: &Home, dev: bool) -> std::io::Result<Task> {
        let sid = factory_agent::windows::system::user_sid()?;
        let supervisor = written(&super::supervisor_copy::installed(home));
        Ok(Task::new(name(dev, &sid), Action::supervising(&supervisor, &written(home.root()), dev)))
    }

    pub fn new(name: String, action: Action) -> Task {
        let system = factory_agent::windows::system::system_folder().unwrap_or_else(|| PathBuf::from(r"C:\Windows\System32"));
        Task { name, action, schtasks: system.join("schtasks.exe") }
    }

    /// Its definition, for `schtasks /Create /XML`.
    pub fn xml(&self, sid: &str, enabled: bool) -> String {
        let Action { command, arguments, working_directory } = &self.action;
        format!(
            r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Author>Claktory</Author>
    <Description>Starts Claktory, which runs your agents, when you sign in to Windows. Turn it on or off in Claktory, in Settings, General.</Description>
    <URI>{name}</URI>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{sid}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{sid}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>{enabled}</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>5</Priority>
    <RestartOnFailure>
      <Interval>PT1M</Interval>
      <Count>3</Count>
    </RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>{command}</Command>
      <Arguments>{arguments}</Arguments>
      <WorkingDirectory>{working_directory}</WorkingDirectory>
    </Exec>
  </Actions>
</Task>
"#,
            name = escape(&self.name),
            sid = escape(sid),
            command = escape(command),
            arguments = escape(arguments),
            working_directory = escape(working_directory),
        )
    }

    /// Writes the task, over one of its name that's there (`/F`), through
    /// a file in `scratch` (a private folder), which it removes after.
    pub fn register(&self, enabled: bool, scratch: &Path) -> Result<(), String> {
        let sid = factory_agent::windows::system::user_sid().map_err(|error| format!("Can't read this user's SID: {error}"))?;
        let file = scratch.join("sign-in-task.xml");
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(self.xml(&sid, enabled).encode_utf16().flat_map(u16::to_le_bytes));
        factory_agent::windows::private::write(&file, &bytes).map_err(|error| format!("Can't write {}: {error}", file.display()))?;
        let done =
            self.schtasks(&["/Create".as_ref(), "/TN".as_ref(), self.name.as_ref(), "/XML".as_ref(), file.as_os_str(), "/F".as_ref()]);
        let _ = std::fs::remove_file(&file);
        done.map(drop)
    }

    /// The task as Task Scheduler has it; None when there's no such task
    /// (or it can't be read, which repairing it puts right).
    pub fn query(&self) -> Option<Registered> {
        let said = self.schtasks(&["/Query".as_ref(), "/TN".as_ref(), self.name.as_ref(), "/XML".as_ref()]).ok()?;
        parse(&String::from_utf8_lossy(&said))
    }

    /// Whether it starts at sign-in, and can be run.
    pub fn set_enabled(&self, enabled: bool) -> Result<(), String> {
        let change = if enabled { "/ENABLE" } else { "/DISABLE" };
        self.schtasks(&["/Change".as_ref(), "/TN".as_ref(), self.name.as_ref(), change.as_ref()]).map(drop)
    }

    /// Asks Task Scheduler to run it now, enabling it for the moment when
    /// it's disabled (a disabled task can't be run), then leaving it as it
    /// was. Task Scheduler runs it only while this user is signed in to
    /// Windows; otherwise it says it did, and nothing runs.
    pub fn run(&self) -> Result<(), String> {
        let disabled = self.query().is_some_and(|task| !task.enabled);
        if disabled {
            self.set_enabled(true)?;
        }
        let ran = self.schtasks(&["/Run".as_ref(), "/TN".as_ref(), self.name.as_ref()]).map(drop);
        if disabled && let Err(error) = self.set_enabled(false) {
            return ran.and(Err(error));
        }
        ran
    }

    /// Deletes it, if it's there.
    pub fn delete(&self) -> Result<(), String> {
        if self.query().is_none() {
            return Ok(());
        }
        self.schtasks(&["/Delete".as_ref(), "/TN".as_ref(), self.name.as_ref(), "/F".as_ref()]).map(drop)
    }

    /// Runs schtasks with `args`: what it printed, or Err with what it
    /// said when it fails.
    fn schtasks(&self, args: &[&std::ffi::OsStr]) -> Result<Vec<u8>, String> {
        use std::os::windows::process::CommandExt;
        /// CREATE_NO_WINDOW: no console flashes up from the app.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let shown = || args.iter().map(|arg| arg.to_string_lossy()).collect::<Vec<_>>().join(" ");
        let output = Command::new(&self.schtasks)
            .args(args)
            .stdin(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .output()
            .map_err(|error| format!("Can't run schtasks: {error}"))?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(format!("schtasks {}: {}", shown(), String::from_utf8_lossy(&output.stderr).trim()))
        }
    }
}

/// Text as XML has it in an element.
fn escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '"' => escaped.push_str("&quot;"),
            '\'' => escaped.push_str("&apos;"),
            c => escaped.push(c),
        }
    }
    escaped
}

fn unescape(text: &str) -> String {
    text.replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&")
}

/// The text of the first `<tag>` element in `xml` (with attributes or
/// without), unescaped; "" for one closed in place.
fn element(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let mut from = 0;
    while let Some(at) = xml[from..].find(&open) {
        let rest = &xml[from + at + open.len()..];
        if rest.starts_with(|c: char| c == '>' || c == '/' || c.is_whitespace()) {
            let close = rest.find('>')?;
            if rest[..close].ends_with('/') {
                return Some(String::new());
            }
            let body = &rest[close + 1..];
            let end = body.find(&format!("</{tag}>"))?;
            return Some(unescape(body[..end].trim()));
        }
        from += at + open.len();
    }
    None
}

/// A task's definition, as `schtasks /Query /XML` gives it.
pub fn parse(xml: &str) -> Option<Registered> {
    let settings = element(xml, "Settings")?;
    // Settings without an Enabled are a task that's on.
    let enabled = element(&settings, "Enabled").is_none_or(|enabled| enabled != "false");
    let actions = element(xml, "Actions")?;
    let runs = (actions.matches("<Exec>").count() == 1).then(|| Action {
        command: element(&actions, "Command").unwrap_or_default(),
        arguments: element(&actions, "Arguments").unwrap_or_default(),
        working_directory: element(&actions, "WorkingDirectory").unwrap_or_default(),
    });
    Some(Registered { enabled, runs })
}

/// The processes of this user's that run `program`, but this one.
pub fn running(program: &Path) -> Vec<u32> {
    use factory_agent::windows::system::{image_path, open_process};
    use factory_agent::windows::wide::owned;
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
    };
    let Some(name) = program.file_name() else { return Vec::new() };
    let wanted = program.as_os_str();
    let mut found = Vec::new();
    // SAFETY: a snapshot of the system's processes, checked and owned.
    let Ok(snapshot) = (unsafe { owned(CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)) }) else { return found };
    // SAFETY: an all-zero PROCESSENTRY32W is valid once its size is set.
    let mut entry: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    entry.dwSize = std::mem::size_of::<PROCESSENTRY32W>() as u32;
    // SAFETY: the snapshot and an entry with its size set.
    let mut more = unsafe { Process32FirstW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    while more {
        let length = entry.szExeFile.iter().position(|&unit| unit == 0).unwrap_or(entry.szExeFile.len());
        let exe = String::from_utf16_lossy(&entry.szExeFile[..length]);
        if entry.th32ProcessID != std::process::id()
            && std::ffi::OsStr::new(&exe).eq_ignore_ascii_case(name)
            && let Ok(process) = open_process(entry.th32ProcessID)
            && std::ffi::OsStr::new(&image_path(&process)).eq_ignore_ascii_case(wanted)
        {
            found.push(entry.th32ProcessID);
        }
        // SAFETY: as above.
        more = unsafe { Process32NextW(snapshot.as_raw_handle(), &raw mut entry) } != 0;
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &str) -> Task {
        let supervisor = r#"C:\Users\Zoë & "co"\AppData\Local\Claktory\factory\bin\factoryd-supervisor.exe"#;
        Task::new(name.to_owned(), Action::supervising(supervisor, r"C:\Users\Zoë\AppData\Local\Claktory\factory", true))
    }

    #[test]
    fn a_definition_reads_back_as_it_was_written() {
        let task = task(r"\Claktory\Claktory Dev S-1-5-21-1");
        for enabled in [true, false] {
            let xml = task.xml("S-1-5-21-1", enabled);
            assert!(xml.contains("&quot;C:\\Users\\Zoë &amp; &quot;co&quot;"), "{xml}");
            assert_eq!(parse(&xml), Some(Registered { enabled, runs: Some(task.action.clone()) }));
        }
        assert!(task.action.arguments.ends_with(" --dev supervise"));
    }

    /// As schtasks gives it: an enabled task's Settings leave Enabled out,
    /// quotes unescaped, and empty elements closed in place.
    #[test]
    fn a_definition_as_schtasks_answers_reads_too() {
        let said = r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <Triggers><LogonTrigger><Enabled>false</Enabled></LogonTrigger></Triggers>
  <Settings>
    <IdleSettings><StopOnIdleEnd>false</StopOnIdleEnd></IdleSettings>
    <Priority>5</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>%SystemRoot%\System32\conhost.exe</Command>
      <Arguments>--headless "%LOCALAPPDATA%\Claktory\factory\bin\factoryd-supervisor.exe" supervise</Arguments>
      <WorkingDirectory />
    </Exec>
  </Actions>
</Task>"#;
        let read = parse(said).expect("a task");
        assert!(read.enabled, "the trigger's Enabled isn't the task's");
        let runs = read.runs.expect("one program");
        assert_eq!(runs.arguments, r#"--headless "%LOCALAPPDATA%\Claktory\factory\bin\factoryd-supervisor.exe" supervise"#);
        assert_eq!(runs.working_directory, "");
        let disabled = said.replace("<Priority>5</Priority>", "<Priority>5</Priority><Enabled>false</Enabled>");
        assert!(!parse(&disabled).expect("a task").enabled);
        let two = said.replace("</Exec>", "</Exec><Exec><Command>x</Command></Exec>");
        assert_eq!(parse(&two).expect("a task").runs, None, "two programs aren't the supervisor alone");
        assert_eq!(parse("ERROR: The system cannot find the file specified."), None);
    }

    #[test]
    fn a_home_in_local_app_data_is_named_through_its_variable() {
        let local = super::super::dirs::local_app_data().expect("LocalAppData");
        let home = local.join("Claktory").join("factory");
        assert_eq!(written(&home), r"%LOCALAPPDATA%\Claktory\factory");
        let elsewhere = std::env::temp_dir().join("claktory-sign-in");
        if !elsewhere.starts_with(&local) {
            assert_eq!(written(&elsewhere), elsewhere.display().to_string());
        }
        assert_eq!(written(&local), local.display().to_string(), "LocalAppData itself isn't a home");
    }

    /// With Task Scheduler itself: a task this user may write, read, turn
    /// off and on, and delete, without an administrator.
    #[test]
    fn a_task_is_written_read_turned_off_and_on_and_deleted() {
        let sid = factory_agent::windows::system::user_sid().expect("SID");
        let scratch = std::env::temp_dir().join(format!("claktory-sign-in-{}", std::process::id()));
        factory_agent::windows::private::create_dir_all(&scratch).expect("scratch");
        let task = task(&format!(r"\Claktory\Claktory test {} {sid}", std::process::id()));
        let _ = task.delete();
        assert_eq!(task.query(), None);
        task.register(false, &scratch).expect("registered");
        assert!(std::fs::read_dir(&scratch).expect("scratch").next().is_none(), "its file is gone");
        assert!(!task.query().expect("there").enabled);
        task.set_enabled(true).expect("on");
        assert!(task.query().expect("there").enabled);
        task.register(false, &scratch).expect("written again over itself");
        assert!(!task.query().expect("there").enabled);
        task.delete().expect("deleted");
        assert_eq!(task.query(), None);
        task.delete().expect("nothing to delete");
        let _ = std::fs::remove_dir_all(&scratch);

        let ascii = Task::new(task.name.clone(), Action::supervising(r"%LOCALAPPDATA%\Claktory\x.exe", r"%LOCALAPPDATA%\Claktory", false));
        ascii.register(true, &std::env::temp_dir()).expect("registered");
        assert_eq!(ascii.query(), Some(Registered { enabled: true, runs: Some(ascii.action.clone()) }));
        ascii.delete().expect("deleted");
    }

    #[test]
    fn a_program_is_found_running_by_its_file_and_never_as_this_process() {
        let ping = factory_agent::windows::system::system_folder().expect("System32").join("ping.exe");
        let mut child = std::process::Command::new(&ping).args(["-n", "30", "127.0.0.1"]).stdout(Stdio::null()).spawn().expect("ping");
        let found = running(&ping);
        let elsewhere = running(&std::env::temp_dir().join("ping.exe"));
        let _ = child.kill();
        let _ = child.wait();
        assert!(found.contains(&child.id()), "{found:?}");
        assert!(!elsewhere.contains(&child.id()), "the same name somewhere else isn't the program");
        let me = std::env::current_exe().expect("exe");
        assert!(!running(&me).contains(&std::process::id()));
    }
}
