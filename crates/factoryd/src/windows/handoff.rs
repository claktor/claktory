//! The open vault's key from one backend to the next across an update, on
//! Windows: what the socket
//! pair is on a Mac or Linux (`vault::handoff`), with the same messages and
//! the same trial and rollback rules.
//!
//! Two anonymous pipes, made for this user only and not inheritable: `down`
//! (the supervisor writes the held key into it before it starts the
//! backend) and `up` (the backend writes its key before it exits for an
//! update, and `D` when the update passed). The backend's two ends reach
//! only the backend the supervisor starts, through the process's attribute
//! list (PROC_THREAD_ATTRIBUTE_HANDLE_LIST), never by inheriting everything
//! inheritable; their values are in `FACTORY_VAULT_HANDLES`. They're
//! inheritable only while the supervisor holds its spawn lock (`SPAWN`),
//! which every process the supervisor starts takes.
//!
//! The backend takes its ends first thing in `main` (`seal`), before logs
//! or anything else could start a process: no longer inheritable, and the
//! variable gone from its environment. Anything off (no variable, a value
//! that isn't a pipe, a short message, a key that doesn't open the vault)
//! hands nothing over: the vault opens again by itself, or asks.
//!
//! Like the open vault's key in the backend's memory, this keeps nothing
//! from a program of the same user that opens these processes; that waits for contained commands.

use std::ffi::OsString;
use std::fs::File;
use std::io::{Read, Write};
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::Mutex;

use factory_agent::windows::child::Child;
use factory_agent::windows::system::Protected;
use factory_agent::windows::wide::{check, owned};
use windows_sys::Win32::Foundation::{HANDLE, HANDLE_FLAG_INHERIT, SetHandleInformation};
use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_PIPE, GetFileType};
use windows_sys::Win32::System::Pipes::{CreatePipe, PeekNamedPipe};
use zeroize::Zeroizing;

/// The variable naming the backend's two ends: `<down>,<up>`.
pub const HANDLES_ENV: &str = "FACTORY_VAULT_HANDLES";

/// Held by every process start in the supervisor, so none inherits the
/// pipes' ends while they're inheritable for the backend's.
/// Anything the supervisor binary starts with std's or tokio's `Command`,
/// which inherits every inheritable handle, takes it first.
pub static SPAWN: Mutex<()> = Mutex::new(());

/// The most a message is: `K`, a length, an id of up to 255 bytes, a key.
const MOST: usize = 2 + 255 + 32;

/// The backend's ends, once sealed.
static ENDS: Mutex<Ends> = Mutex::new(Ends { down: None, up: None });

struct Ends {
    down: Option<File>,
    up: Option<File>,
}

fn ends() -> std::sync::MutexGuard<'static, Ends> {
    ENDS.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The backend's side, first thing in `main`: takes the ends the supervisor
/// named, if they're pipes, so nothing this process starts can inherit
/// them, and removes the variable.
pub fn seal() {
    let Some(text) = std::env::var_os(HANDLES_ENV) else { return };
    // SAFETY: first thing in main, before any other thread exists.
    unsafe { std::env::remove_var(HANDLES_ENV) };
    let Some((down, up)) = parse_handles(&text.to_string_lossy()) else { return };
    let (down, up) = (take(down), take(up));
    // Both or neither: one alone is no handover.
    if let (Some(down), Some(up)) = (down, up) {
        *ends() = Ends { down: Some(down), up: Some(up) };
    }
}

fn parse_handles(text: &str) -> Option<(usize, usize)> {
    let (down, up) = text.split_once(',')?;
    let (down, up) = (down.parse::<usize>().ok()?, up.parse::<usize>().ok()?);
    (down != 0 && up != 0 && down != up).then_some((down, up))
}

/// The handle `value` names, if it's a pipe: no longer inheritable, and
/// this process's from here on. Anything else is left alone.
fn take(value: usize) -> Option<File> {
    let handle = value as HANDLE;
    // SAFETY: GetFileType only asks; an invalid value answers FILE_TYPE_UNKNOWN.
    if unsafe { GetFileType(handle) } != FILE_TYPE_PIPE {
        return None;
    }
    // SAFETY: an open pipe handle this process inherited.
    let cleared = unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0) };
    // SAFETY: the supervisor handed it to this process alone; owned (and closed) from here on.
    let file = File::from(unsafe { OwnedHandle::from_raw_handle(handle) });
    // One that stays inheritable is closed rather than kept.
    (cleared != 0).then_some(file)
}

/// What the supervisor wrote before starting this backend (the held key,
/// if any), read at once without waiting; `down` is closed after.
pub fn take_waiting() -> Zeroizing<Vec<u8>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let Some(mut down) = ends().down.take() else { return bytes };
    let mut buffer = Zeroizing::new([0u8; 512]);
    while bytes.len() < MOST {
        let mut waiting = 0u32;
        // SAFETY: an open pipe handle; only the count of bytes waiting is asked.
        let peeked = unsafe {
            PeekNamedPipe(down.as_raw_handle(), std::ptr::null_mut(), 0, std::ptr::null_mut(), &raw mut waiting, std::ptr::null_mut())
        };
        if peeked == 0 || waiting == 0 {
            break;
        }
        let want = (waiting as usize).min(buffer.len()).min(MOST - bytes.len());
        match down.read(&mut buffer[..want]) {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&buffer[..read]),
        }
    }
    bytes
}

/// Writes to the supervisor; whether it went.
pub fn write(bytes: &[u8]) -> bool {
    ends().up.as_mut().is_some_and(|up| up.write_all(bytes).and_then(|()| up.flush()).is_ok())
}

/// A backend the supervisor started, and what it writes up.
pub struct Started {
    pub child: Child,
    /// What the backend writes (`vault::handoff::Holder::read`), until it
    /// closes its end; None when it was started without a handover.
    pub written: Option<tokio::sync::mpsc::UnboundedReceiver<Zeroizing<Vec<u8>>>>,
}

/// The supervisor's side: starts the backend at `program` with `args`, this
/// process's environment and `extra`, handing it `held` (the key a backend
/// kept, as `vault::handoff` writes it) through its own pipe. A handover that
/// can't be made is left out, logged: the backend then opens the vault again
/// by itself, or asks. Only starting it can fail.
pub fn spawn(program: &Path, args: &[OsString], extra: &[(&str, &str)], held: Option<&[u8]>) -> std::io::Result<Started> {
    let mut environment: Vec<(OsString, OsString)> =
        std::env::vars_os().filter(|(name, _)| !name.eq_ignore_ascii_case(HANDLES_ENV)).collect();
    environment.extend(extra.iter().map(|(name, value)| (OsString::from(name), OsString::from(value))));
    let pipes =
        Pipes::new(held).inspect_err(|error| tracing::warn!(%error, "could not make the vault's handover; an update locks it")).ok();
    let guard = SPAWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let started = match &pipes {
        Some(pipes) => pipes.inheritable().and_then(|()| {
            let mut environment = environment.clone();
            environment.push((
                HANDLES_ENV.into(),
                format!("{},{}", pipes.down_read.as_raw_handle() as usize, pipes.up_write.as_raw_handle() as usize).into(),
            ));
            factory_agent::windows::spawn::spawn_helper(
                program,
                args,
                &environment,
                &[pipes.down_read.as_raw_handle(), pipes.up_write.as_raw_handle()],
            )
        }),
        None => factory_agent::windows::spawn::spawn_helper(program, args, &environment, &[]),
    };
    // The backend has its own copies now (or none): ours close before
    // anything else can start.
    let reader = pipes.map(|pipes| pipes.up_read);
    drop(guard);
    let mut child = started?;
    child.kill_on_drop(true);
    let written = reader.map(|up| {
        let (sender, receiver) = tokio::sync::mpsc::unbounded_channel();
        // Reading a pipe blocks: a thread of its own, ending with the backend's end.
        std::thread::spawn(move || {
            let mut up = File::from(up);
            let mut buffer = Zeroizing::new([0u8; 512]);
            loop {
                match up.read(buffer.as_mut_slice()) {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if sender.send(Zeroizing::new(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        receiver
    });
    Ok(Started { child, written })
}

/// The two pipes; the backend's ends close when this is dropped, which the
/// caller does as soon as the backend is started.
struct Pipes {
    down_read: OwnedHandle,
    up_write: OwnedHandle,
    up_read: OwnedHandle,
}

impl Pipes {
    fn new(held: Option<&[u8]>) -> std::io::Result<Pipes> {
        let (down_read, down_write) = pipe()?;
        let (up_read, up_write) = pipe()?;
        if let Some(held) = held {
            // Waiting in the pipe for the backend; it fits its buffer.
            let mut down = File::from(down_write);
            down.write_all(held)?;
        } else {
            drop(down_write);
        }
        Ok(Pipes { down_read, up_write, up_read })
    }

    /// The backend's ends, inheritable for the attribute list to name:
    /// only under `SPAWN`, and closed right after the start.
    fn inheritable(&self) -> std::io::Result<()> {
        for handle in [&self.down_read, &self.up_write] {
            // SAFETY: an open handle of ours.
            check(unsafe { SetHandleInformation(handle.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) })?;
        }
        Ok(())
    }
}

/// An anonymous pipe only this user can open, not inheritable: (read, write).
fn pipe() -> std::io::Result<(OwnedHandle, OwnedHandle)> {
    let protected = Protected::for_user()?;
    let attributes = protected.attributes();
    let (mut read, mut write): (HANDLE, HANDLE) = (std::ptr::null_mut(), std::ptr::null_mut());
    // SAFETY: two handles to write, made with a live descriptor; a 4 KiB buffer holds any message.
    check(unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 4096) })?;
    // SAFETY: the pipe's two fresh handles, ours alone.
    Ok(unsafe { (owned(read)?, owned(write)?) })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// Set in a test's child: it lists the pipes it holds and exits.
    const ENUMERATE: &str = "FACTORY_TEST_LIST_PIPES";

    /// The pipe handles this process holds, other than its standard ones:
    /// what a hostile child would find (handle values are multiples of 4).
    fn pipes_held() -> Vec<usize> {
        use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
        // SAFETY: only reads this process's standard handles.
        let standard: Vec<usize> =
            [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].iter().map(|which| unsafe { GetStdHandle(*which) } as usize).collect();
        (1..0x4000usize)
            .map(|index| index * 4)
            .filter(|value| !standard.contains(value))
            // SAFETY: GetFileType only asks; a value that's no handle answers FILE_TYPE_UNKNOWN.
            .filter(|value| unsafe { GetFileType(*value as HANDLE) } == FILE_TYPE_PIPE)
            .collect()
    }

    /// Run as a child by the tests below (it does nothing on its own): prints
    /// the pipes it holds, and what was handed down, and exits.
    #[test]
    fn child_lists_its_pipes() {
        if std::env::var_os(ENUMERATE).is_none() {
            return;
        }
        let named = std::env::var(HANDLES_ENV).unwrap_or_default();
        seal();
        let waiting = take_waiting();
        let held = pipes_held();
        let report = format!("PIPES {named} {held:?} WAITING {}", String::from_utf8_lossy(&waiting));
        // A backend reports up its pipe; a std child on its stdout.
        if !write(report.as_bytes()) {
            println!("{report}");
        }
        std::process::exit(0);
    }

    fn child_command() -> (std::path::PathBuf, Vec<OsString>) {
        let exe = std::env::current_exe().expect("test binary");
        let args = ["windows::handoff::tests::child_lists_its_pipes", "--exact", "--nocapture", "--test-threads=1"];
        (exe, args.iter().map(OsString::from).collect())
    }

    #[tokio::test]
    async fn the_backend_inherits_its_two_ends_and_nothing_else() {
        let (exe, args) = child_command();
        // A pipe of ours that's inheritable: the backend mustn't get it.
        let (stray, _stray_write) = pipe().expect("pipe");
        // SAFETY: an open handle of ours.
        check(unsafe { SetHandleInformation(stray.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }).expect("inheritable");
        let mut started = spawn(&exe, &args, &[(ENUMERATE, "1")], Some(b"KEY")).expect("started");
        let status = started.child.wait().await.expect("waited");
        assert!(status.success(), "{status:?}");
        let mut written = started.written.take().expect("a handover");
        let mut up = Vec::new();
        while let Some(bytes) = written.recv().await {
            up.extend_from_slice(&bytes);
        }
        let report = String::from_utf8(up).expect("text");
        // "PIPES <down>,<up> [<up>] WAITING KEY": down read and closed, only
        // up still held, nothing else, and the key arrived.
        let named = report.strip_prefix("PIPES ").and_then(|rest| rest.split_once(' ')).map(|(named, _)| named.to_owned()).expect(&report);
        let (_, up_value) = parse_handles(&named).expect(&report);
        // The stray, inheritable all along, isn't among them.
        assert_eq!(report, format!("PIPES {named} [{up_value}] WAITING KEY"));
        drop(stray);

        // Without a key: nothing waiting, still a way up.
        let mut started = spawn(&exe, &args, &[(ENUMERATE, "1")], None).expect("started");
        started.child.wait().await.expect("waited");
        let mut written = started.written.take().expect("a handover");
        let mut up = Vec::new();
        while let Some(bytes) = written.recv().await {
            up.extend_from_slice(&bytes);
        }
        assert!(String::from_utf8_lossy(&up).ends_with("WAITING "), "{}", String::from_utf8_lossy(&up));
    }

    #[test]
    fn a_concurrent_std_child_never_gets_the_ends() {
        // In a process of its own, so no other test's pipes are about.
        const ALONE: &str = "FACTORY_TEST_ALONE";
        let (exe, args) = child_command();
        if std::env::var_os(ALONE).is_none() {
            let status = Command::new(&exe)
                .args(["windows::handoff::tests::a_concurrent_std_child_never_gets_the_ends", "--exact", "--test-threads=1"])
                .env(ALONE, "1")
                .status()
                .expect("alone");
            assert!(status.success(), "{status:?}");
            return;
        }
        // What a std child holds: the pipes it found, past its own stdout.
        // This process inherited some of its own (whatever the test runner
        // had inheritable), which every std child inherits again: those are
        // the baseline.
        let std_child = || -> Vec<usize> {
            let child = {
                let _guard = SPAWN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let mut command = Command::new(&exe);
                command.args(&args).env(ENUMERATE, "1").stdin(Stdio::null()).stderr(Stdio::null()).stdout(Stdio::piped());
                command.spawn().expect("child")
            };
            let output = child.wait_with_output().expect("output");
            let text = String::from_utf8_lossy(&output.stdout).into_owned();
            let list = text.split_once("PIPES ").and_then(|(_, rest)| rest.split_once('[')).and_then(|(_, rest)| rest.split_once(']'));
            let (list, _) = list.unwrap_or_else(|| panic!("no list: {text}"));
            list.split(", ").filter(|value| !value.is_empty()).map(|value| value.parse().expect("a handle")).collect()
        };
        let baseline = std_child();
        // The control: an inheritable pipe end is found by a std child, so
        // the listing works.
        let (control, _write) = pipe().expect("pipe");
        // SAFETY: an open handle of ours.
        check(unsafe { SetHandleInformation(control.as_raw_handle(), HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) }).expect("inheritable");
        let seen = std_child();
        assert!(seen.contains(&(control.as_raw_handle() as usize)), "the control wasn't seen: {seen:?}");
        drop(control);
        assert_eq!(std_child(), baseline);

        // Backends started over and over on one thread, std children on
        // another, taking the spawn lock as the supervisor's do: no std
        // child ever holds a pipe beyond its own output's.
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let spawner = {
            let stop = stop.clone();
            let (exe, args) = (exe.clone(), args.clone());
            std::thread::spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("runtime");
                while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                    let mut started = spawn(&exe, &args, &[(ENUMERATE, "1")], Some(b"KEY")).expect("started");
                    runtime.block_on(started.child.wait()).expect("waited");
                }
            })
        };
        for _ in 0..20 {
            assert_eq!(std_child(), baseline, "a std child held a backend's end");
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        spawner.join().expect("spawner");
    }

    #[test]
    fn handles_are_named_as_two_values() {
        assert_eq!(parse_handles("12,16"), Some((12, 16)));
        for bad in ["", "12", "12,", "0,16", "12,12", "a,b", "12,16,20"] {
            assert_eq!(parse_handles(bad), None, "{bad}");
        }
    }
}
