//! Starting a command's shell on Windows: CreateProcessW with an attribute
//! list, because neither std's nor tokio's Command can say either thing that
//! matters here:
//!
//! - PROC_THREAD_ATTRIBUTE_JOB_LIST: the process is in its job as it's
//!   created, so nothing it runs is ever outside it, even for an instant.
//! - PROC_THREAD_ATTRIBUTE_HANDLE_LIST: it inherits only the handles named
//!   here, its stdin, stdout and stderr, never one of the backend's (its
//!   database, the vault's handoff, the tunnel's sockets), whatever else
//!   happens to be inheritable in this process at that moment.
//!
//! Each command gets a console of its own, hidden (CREATE_NEW_CONSOLE with
//! SW_HIDE), so the console programs it runs share it instead of each
//! opening a window of its own on the person's screen.

use std::ffi::{OsStr, OsString};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GetHandleInformation, HANDLE, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    SetHandleInformation,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NEW_CONSOLE, CREATE_UNICODE_ENVIRONMENT, CreateProcessW, DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT,
    GetCurrentProcess, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
    PROC_THREAD_ATTRIBUTE_JOB_LIST, PROCESS_INFORMATION, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    UpdateProcThreadAttribute,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

use super::child::Child;
use super::job::{self, Job};
use super::system::creation_time;
use super::wide::{check, owned, wide};

/// Where a stream of the command goes.
pub enum Output {
    /// To a pipe the backend reads (`Child::stdout` / `stderr`).
    Pipe,
    /// To this file (a task's log, opened to append).
    File(std::fs::File),
}

/// What to start.
pub struct Command<'a> {
    /// The program, by its absolute path: never looked up on PATH.
    pub program: &'a Path,
    /// Its arguments, after its own name.
    pub args: Vec<OsString>,
    /// Its whole environment (`shell::environment` makes it).
    pub environment: Vec<(OsString, OsString)>,
    pub cwd: &'a Path,
    pub stdout: Output,
    /// None: the same as stdout.
    pub stderr: Option<Output>,
}

/// Starts `command` in a new job of its own, remembered under its pid
/// (`job::alive`, `job::stop`). Its stdin is empty (NUL).
pub fn spawn(command: Command<'_>) -> std::io::Result<(Child, u32)> {
    let job = Job::create()?;
    // The child's ends, inheritable; ours, not.
    let stdin = inheritable(open_nul()?)?;
    let (stdout, out_reader) = end(command.stdout)?;
    let (stderr, err_reader) = match command.stderr {
        Some(output) => {
            let (handle, reader) = end(output)?;
            (Some(handle), reader)
        }
        None => (None, None),
    };
    let stdout_raw = stdout.as_raw_handle();
    let stderr_raw = stderr.as_ref().map_or(stdout_raw, AsRawHandle::as_raw_handle);
    let mut handles: Vec<HANDLE> = vec![stdin.as_raw_handle(), stdout_raw];
    if stderr_raw != stdout_raw {
        handles.push(stderr_raw);
    }
    let job_handle = job.raw();
    let attributes = Attributes::new(&handles, Some(&job_handle))?;

    let application = wide(command.program)?;
    let mut line = wide(command_line(command.program.as_os_str(), &command.args)?)?;
    let block = environment_block(&command.environment)?;
    let cwd = wide(command.cwd)?;

    // SAFETY: an all-zero STARTUPINFOEXW is valid to fill in.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
    startup.StartupInfo.wShowWindow = SW_HIDE as u16;
    startup.StartupInfo.hStdInput = stdin.as_raw_handle();
    startup.StartupInfo.hStdOutput = stdout_raw;
    startup.StartupInfo.hStdError = stderr_raw;
    startup.lpAttributeList = attributes.list;
    // SAFETY: an all-zero PROCESS_INFORMATION is valid to write over.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every pointer is to a live, NUL-terminated buffer or structure above;
    // bInheritHandles is on only with the attribute list naming exactly what's inherited.
    check(unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT | CREATE_NEW_CONSOLE,
            block.as_ptr().cast(),
            cwd.as_ptr(),
            &raw const startup.StartupInfo,
            &raw mut info,
        )
    })?;
    drop(attributes);
    // The child has its own copies now.
    drop((stdin, stdout, stderr));
    // SAFETY: fresh handles CreateProcessW gave us.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: as above; the thread's handle isn't needed.
    drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
    let pid = info.dwProcessId;
    let created = creation_time(&process)?;
    job::remember(job, pid, created);
    let child = Child::new(process, pid, out_reader.map(reader), err_reader.map(reader))?;
    Ok((child, pid))
}

/// Starts one of Claktory's own processes (the supervisor's backend) the way
/// it starts commands, without the job: it inherits exactly `inherit` (which
/// the caller made inheritable) and its stdin (NUL), stdout and stderr (this
/// process's, or NUL), never whatever else is inheritable here. It shares
/// this process's console and folder; `environment` is its whole environment.
///
/// The handles the list names are inheritable until the caller closes them:
/// it holds its own spawn lock across this call (`factoryd::windows::backend`),
/// so no other spawn in the process picks them up meanwhile.
pub fn spawn_helper(program: &Path, args: &[OsString], environment: &[(OsString, OsString)], inherit: &[HANDLE]) -> std::io::Result<Child> {
    let stdin = inheritable(open_nul()?)?;
    let stdout = standard(STD_OUTPUT_HANDLE)?;
    let stderr = standard(STD_ERROR_HANDLE)?;
    let mut handles: Vec<HANDLE> = vec![stdin.as_raw_handle(), stdout.as_raw_handle(), stderr.as_raw_handle()];
    handles.extend_from_slice(inherit);
    let attributes = Attributes::new(&handles, None)?;

    let application = wide(program)?;
    let mut line = wide(command_line(program.as_os_str(), args)?)?;
    let block = environment_block(environment)?;

    // SAFETY: an all-zero STARTUPINFOEXW is valid to fill in.
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin.as_raw_handle();
    startup.StartupInfo.hStdOutput = stdout.as_raw_handle();
    startup.StartupInfo.hStdError = stderr.as_raw_handle();
    startup.lpAttributeList = attributes.list;
    // SAFETY: an all-zero PROCESS_INFORMATION is valid to write over.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every pointer is to a live, NUL-terminated buffer or structure above;
    // bInheritHandles is on only with the attribute list naming exactly what's inherited.
    check(unsafe {
        CreateProcessW(
            application.as_ptr(),
            line.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            1,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            block.as_ptr().cast(),
            std::ptr::null(),
            &raw const startup.StartupInfo,
            &raw mut info,
        )
    })?;
    drop(attributes);
    drop((stdin, stdout, stderr));
    // SAFETY: fresh handles CreateProcessW gave us.
    let process = unsafe { OwnedHandle::from_raw_handle(info.hProcess) };
    // SAFETY: as above; the thread's handle isn't needed.
    drop(unsafe { OwnedHandle::from_raw_handle(info.hThread) });
    Child::new(process, info.dwProcessId, None, None)
}

/// An inheritable copy of this process's standard handle, or NUL when it
/// has none (a process started without a console or redirection).
fn standard(which: STD_HANDLE) -> std::io::Result<OwnedHandle> {
    // SAFETY: only reads this process's standard handle; it isn't ours to close.
    let handle = unsafe { GetStdHandle(which) };
    if handle.is_null() || handle == INVALID_HANDLE_VALUE {
        return inheritable(open_nul()?);
    }
    let mut copy: HANDLE = std::ptr::null_mut();
    // SAFETY: duplicating an open handle of this process into itself.
    let duplicated =
        unsafe { DuplicateHandle(GetCurrentProcess(), handle, GetCurrentProcess(), &raw mut copy, 0, 1, DUPLICATE_SAME_ACCESS) };
    if duplicated == 0 {
        return inheritable(open_nul()?);
    }
    // SAFETY: the fresh, inheritable copy, ours alone.
    unsafe { owned(copy) }
}

fn reader(handle: OwnedHandle) -> tokio::fs::File {
    tokio::fs::File::from_std(std::fs::File::from(handle))
}

/// The child's end of an output (inheritable), and ours to read for a pipe.
fn end(output: Output) -> std::io::Result<(OwnedHandle, Option<OwnedHandle>)> {
    match output {
        Output::File(file) => Ok((inheritable(OwnedHandle::from(file))?, None)),
        Output::Pipe => {
            let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
            let attributes = SECURITY_ATTRIBUTES {
                nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                lpSecurityDescriptor: std::ptr::null_mut(),
                bInheritHandle: 0,
            };
            // SAFETY: two handles to write; neither inheritable yet.
            check(unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) })?;
            // SAFETY: the pipe's two fresh handles, ours alone.
            let (read, write) = unsafe { (owned(read)?, owned(write)?) };
            Ok((inheritable(write)?, Some(read)))
        }
    }
}

/// Runs `spawn` (std's Command, which a child inherits every inheritable
/// handle from) with this process's standard input, output and error kept
/// from it, then puts them back as they were. A child that outlives this
/// process then doesn't hold its output open, so whatever reads that (a
/// script's pipe, an ssh session) ends when this process does. For a
/// program that starts others to run on after it (claktory-qa's
/// sandboxes); a spawn on another thread meanwhile would lose them too.
pub fn keeping_standard_handles<T>(spawn: impl FnOnce() -> T) -> T {
    let mut kept: Vec<HANDLE> = Vec::new();
    for which in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
        // SAFETY: only reads this process's standard handle; it isn't ours to close.
        let handle = unsafe { GetStdHandle(which) };
        if handle.is_null() || handle == INVALID_HANDLE_VALUE || kept.contains(&handle) {
            continue;
        }
        let mut flags = 0;
        // SAFETY: an open handle and a flags word to write.
        let inheritable = unsafe { GetHandleInformation(handle, &raw mut flags) } != 0 && flags & HANDLE_FLAG_INHERIT != 0;
        // SAFETY: as above; only its inherit flag changes.
        if inheritable && unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) } != 0 {
            kept.push(handle);
        }
    }
    let spawned = spawn();
    for handle in kept {
        // SAFETY: the same standard handle, made inheritable again as it was.
        unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) };
    }
    spawned
}

/// `handle`, made inheritable, for the attribute list to name. Only what's
/// in the list is inherited, so another spawn elsewhere in the backend
/// (std's, which inherits everything inheritable) is the only thing that
/// could pick it up meanwhile; it's closed as soon as the child has it.
fn inheritable(handle: OwnedHandle) -> std::io::Result<OwnedHandle> {
    let mut copy: HANDLE = std::ptr::null_mut();
    // SAFETY: duplicating our own open handle within this process.
    check(unsafe {
        DuplicateHandle(GetCurrentProcess(), handle.as_raw_handle(), GetCurrentProcess(), &raw mut copy, 0, 1, DUPLICATE_SAME_ACCESS)
    })?;
    // SAFETY: the fresh copy, ours alone.
    let copy = unsafe { owned(copy) }?;
    // SAFETY: an open handle; the flag is already set by the duplicate, this only makes sure.
    check(unsafe { SetHandleInformation(copy.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) })?;
    Ok(copy)
}

fn open_nul() -> std::io::Result<OwnedHandle> {
    let name = wide("NUL")?;
    // SAFETY: a NUL-terminated name; the handle is checked and owned.
    unsafe {
        owned(CreateFileW(
            name.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            std::ptr::null_mut(),
        ))
    }
}

/// The process attribute list: the handles to inherit and the job.
struct Attributes {
    list: LPPROC_THREAD_ATTRIBUTE_LIST,
    _memory: Vec<u64>,
}

impl Attributes {
    /// `handles` and `job` must outlive it (the list points at them).
    fn new(handles: &[HANDLE], job: Option<&HANDLE>) -> std::io::Result<Attributes> {
        let count = if job.is_some() { 2 } else { 1 };
        let mut size = 0usize;
        // SAFETY: asks only for the size; it "fails" with ERROR_INSUFFICIENT_BUFFER.
        unsafe { InitializeProcThreadAttributeList(std::ptr::null_mut(), count, 0, &raw mut size) };
        let mut memory = vec![0u64; size.div_ceil(8)];
        let list: LPPROC_THREAD_ATTRIBUTE_LIST = memory.as_mut_ptr().cast();
        // SAFETY: `size` bytes of memory for the list.
        check(unsafe { InitializeProcThreadAttributeList(list, count, 0, &raw mut size) })?;
        let attributes = Attributes { list, _memory: memory };
        // SAFETY: the handles' array and its size in bytes, alive as long as the list.
        check(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
                handles.as_ptr().cast(),
                std::mem::size_of_val(handles),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        })?;
        let Some(job) = job else { return Ok(attributes) };
        // SAFETY: one job handle, alive as long as the list.
        check(unsafe {
            UpdateProcThreadAttribute(
                list,
                0,
                PROC_THREAD_ATTRIBUTE_JOB_LIST as usize,
                (job as *const HANDLE).cast(),
                std::mem::size_of::<HANDLE>(),
                std::ptr::null_mut(),
                std::ptr::null(),
            )
        })?;
        Ok(attributes)
    }
}

impl Drop for Attributes {
    fn drop(&mut self) {
        // SAFETY: initialized in new, deleted once.
        unsafe { DeleteProcThreadAttributeList(self.list) };
    }
}

/// A command line from a program and its arguments, quoted the way the C
/// runtime (and Cygwin's, which Git Bash is built on) reads it back: an
/// argument with a space, a quote or anything Cygwin would expand (a
/// wildcard, a brace, a tilde, a single quote) goes in double quotes, so
/// neither splits nor expands it, with backslashes doubled only before a
/// quote; a plain one goes as it is.
pub fn command_line(program: &OsStr, args: &[OsString]) -> std::io::Result<OsString> {
    let mut line = String::new();
    for (index, arg) in std::iter::once(program).chain(args.iter().map(OsString::as_os_str)).enumerate() {
        let arg = arg.to_str().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "an argument isn't Unicode"))?;
        if arg.contains('\0') {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "an argument holds a NUL"));
        }
        if index > 0 {
            line.push(' ');
        }
        let plain = !arg.is_empty() && !arg.chars().any(|character| character.is_whitespace() || "\"*?[]{}'~".contains(character));
        if plain {
            line.push_str(arg);
            continue;
        }
        line.push('"');
        let mut backslashes = 0;
        for character in arg.chars() {
            match character {
                '\\' => backslashes += 1,
                '"' => {
                    line.extend(std::iter::repeat_n('\\', backslashes * 2 + 1));
                    line.push('"');
                    backslashes = 0;
                }
                other => {
                    line.extend(std::iter::repeat_n('\\', backslashes));
                    line.push(other);
                    backslashes = 0;
                }
            }
        }
        line.extend(std::iter::repeat_n('\\', backslashes * 2));
        line.push('"');
    }
    Ok(line.into())
}

/// An environment block: NAME=value pairs, each NUL-terminated, sorted as
/// Windows expects (case-insensitively), then one more NUL. A name with
/// `=` after its first character, or a NUL anywhere, is refused.
pub fn environment_block(environment: &[(OsString, OsString)]) -> std::io::Result<Vec<u16>> {
    let mut pairs: Vec<&(OsString, OsString)> = environment.iter().collect();
    pairs.sort_by_cached_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in pairs {
        let name_text = name.to_string_lossy();
        if name_text.is_empty() || name_text[1..].contains('=') {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{name_text:?} can't be an environment variable's name"),
            ));
        }
        let mut pair = name.clone();
        pair.push("=");
        pair.push(value);
        let mut units = wide(&pair)?;
        block.append(&mut units);
    }
    if block.is_empty() {
        block.push(0);
    }
    block.push(0);
    Ok(block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(args: &[&str]) -> String {
        command_line(OsStr::new("C:\\Program Files\\Git\\usr\\bin\\bash.exe"), &args.iter().map(OsString::from).collect::<Vec<_>>())
            .unwrap()
            .into_string()
            .unwrap()
    }

    const OUTLIVING: &str = "CLAKTORY_SPAWN_TEST_OUTLIVING";

    /// Run by the test below as a process of its own: starts a program that
    /// outlives it (std's Command, its output going elsewhere), says so, and ends.
    #[test]
    fn child_starts_one_that_outlives_it() {
        let Some(kept) = std::env::var_os(OUTLIVING) else { return };
        let mut ping = std::process::Command::new("ping");
        ping.args(["-n", "8", "127.0.0.1"]).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null());
        let started = if kept == "1" { keeping_standard_handles(|| ping.spawn()) } else { ping.spawn() };
        println!("STARTED {}", started.expect("ping").id());
    }

    /// How long reading the child's output to its end takes, and the
    /// program it started.
    fn read_to_end(kept: bool) -> (std::time::Duration, u32) {
        use std::io::Read as _;
        let exe = std::env::current_exe().expect("test binary");
        let mut child = std::process::Command::new(exe)
            .args(["windows::spawn::tests::child_starts_one_that_outlives_it", "--exact", "--nocapture", "--test-threads=1"])
            .env(OUTLIVING, if kept { "1" } else { "0" })
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("child");
        let started = std::time::Instant::now();
        let mut output = String::new();
        child.stdout.take().expect("its output").read_to_string(&mut output).expect("read");
        let took = started.elapsed();
        let _ = child.wait();
        let pid = output.split("STARTED ").nth(1).and_then(|rest| rest.split_whitespace().next()?.parse().ok()).expect("its pid");
        (took, pid)
    }

    #[test]
    fn a_program_started_keeping_the_standard_handles_doesnt_hold_its_starters_output_open() {
        let (took, pid) = read_to_end(true);
        super::super::system::terminate_process(pid);
        assert!(took < std::time::Duration::from_secs(4), "the output ended only after {took:?}");
        // The control: started plainly, it holds the output until it ends,
        // which is what the test guards against.
        let (took, pid) = read_to_end(false);
        super::super::system::terminate_process(pid);
        assert!(took >= std::time::Duration::from_secs(5), "the output ended after {took:?} anyway, so the test proves nothing");
    }

    #[test]
    fn arguments_are_quoted_as_the_c_runtime_reads_them() {
        assert_eq!(line(&[]), r#""C:\Program Files\Git\usr\bin\bash.exe""#);
        assert_eq!(line(&["-c", "echo *"]), r#""C:\Program Files\Git\usr\bin\bash.exe" -c "echo *""#);
        assert_eq!(line(&["*.rs", "~", "{a,b}", "it's"]), r#""C:\Program Files\Git\usr\bin\bash.exe" "*.rs" "~" "{a,b}" "it's""#);
        assert_eq!(line(&[r"C:\plain\path"]), r#""C:\Program Files\Git\usr\bin\bash.exe" C:\plain\path"#);
        assert_eq!(line(&[r#"say "hi""#]), r#""C:\Program Files\Git\usr\bin\bash.exe" "say \"hi\"""#);
        assert_eq!(line(&[r"C:\dir\"]), r#""C:\Program Files\Git\usr\bin\bash.exe" C:\dir\"#);
        assert_eq!(line(&[r"C:\my dir\"]), r#""C:\Program Files\Git\usr\bin\bash.exe" "C:\my dir\\""#);
        assert_eq!(line(&[r#"a\"b"#]), r#""C:\Program Files\Git\usr\bin\bash.exe" "a\\\"b""#);
        assert_eq!(line(&[""]), r#""C:\Program Files\Git\usr\bin\bash.exe" """#);
    }

    #[test]
    fn the_environment_block_is_sorted_and_ends_twice() {
        let block = environment_block(&[("b".into(), "2".into()), ("A".into(), "1".into()), ("=C:".into(), "C:\\".into())]).unwrap();
        let text = String::from_utf16(&block).unwrap();
        assert_eq!(text, "=C:=C:\\\0A=1\0b=2\0\0");
        assert!(environment_block(&[("A=B".into(), "1".into())]).is_err());
        assert_eq!(environment_block(&[]).unwrap(), vec![0, 0]);
    }

    #[test]
    fn a_command_runs_in_its_job_with_its_output_piped() {
        let system = std::env::var_os("SystemRoot").expect("SystemRoot");
        let cmd = Path::new(&system).join("System32").join("cmd.exe");
        let environment: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        runtime.block_on(async {
            use tokio::io::AsyncReadExt;
            let (mut child, pid) = spawn(Command {
                program: &cmd,
                args: vec!["/d".into(), "/c".into(), "echo hello& exit 3".into()],
                environment,
                cwd: &std::env::temp_dir(),
                stdout: Output::Pipe,
                stderr: None,
            })
            .expect("spawns");
            let mut out = String::new();
            child.stdout.take().unwrap().read_to_string(&mut out).await.unwrap();
            assert_eq!(out.trim(), "hello");
            assert_eq!(child.wait().await.unwrap().code(), Some(3));
            // Its console host may still be in the job a moment longer, but
            // nothing of the command runs.
            assert!(!job::alive(pid));
            assert!(job::processes(pid).is_empty());
        });
    }
}
