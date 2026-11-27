//! Agents' commands never keep a program that asks who you are (Bruno
//! 36035, Fugo 36039 and 36040): sudo and the like are stopped as soon as
//! they're seen, in the foreground, through a log or in the background,
//! and the agent is told. Copies of /bin/sleep (and /bin/bash, for
//! osascript's arguments) under their names; and on a Mac the real sudo,
//! set-uid, which only ever waits there for a password it never gets.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use factory_agent::tools::asking::{Guard, Process, asking, told};
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{ToolResult, ToolResultPart};
use parking_lot::Mutex;
use serde_json::json;
use tokio_util::sync::CancellationToken;

const HANG: Duration = Duration::from_secs(60);

fn text(result: &ToolResult) -> String {
    result
        .content
        .iter()
        .map(|part| match part {
            ToolResultPart::Text { text } => text.clone(),
            ToolResultPart::Image { .. } => String::new(),
        })
        .collect()
}

/// A copy of `program` named `name` in `dir`. macOS kills a copy of a
/// system binary that keeps Apple's signature, so it's signed again, ad hoc.
fn copy_named(dir: &Path, program: &str, name: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::copy(program, &path).expect("copy");
    if cfg!(target_os = "macos") {
        // codesign fails now and then on a busy Mac: tried 3 times.
        let mut signed = None;
        for _ in 0..3 {
            let output = std::process::Command::new("codesign").args(["-f", "-s", "-"]).arg(&path).output().expect("codesign");
            let done = output.status.success();
            signed = Some(output);
            if done {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        let signed = signed.expect("tried");
        assert!(signed.status.success(), "{signed:?}");
    }
    path
}

fn tool(dir: &Path, background: Option<factory_agent::Background>) -> Tool {
    coding_tools(dir, &[], background).into_iter().find(|tool| tool.spec.name == "bash").expect("bash")
}

async fn run(bash: &Tool, command: &str) -> (String, Duration) {
    let began = Instant::now();
    let result = tokio::time::timeout(HANG, bash.execute(&json!({"command": command}), &CancellationToken::new(), &|_| {}))
        .await
        .expect("it ended")
        .expect("ran");
    (text(&result), began.elapsed())
}

#[test]
fn what_asks_who_you_are_is_known_by_its_name_or_its_file() {
    let process = |name: &str, path: &str| Process { pid: 0, name: name.to_owned(), path: path.to_owned() };
    assert_eq!(asking(&process("sudo", "")), Some("sudo"));
    // Run as root, its name can't be read; its file can.
    assert_eq!(asking(&process("", "/usr/bin/sudo")), Some("sudo"));
    assert_eq!(asking(&process("", "/usr/libexec/authopen")), Some("authopen"));
    assert_eq!(asking(&process("pkexec", "/usr/bin/pkexec")), Some("pkexec"));
    assert_eq!(asking(&process("su", "/usr/bin/su")), Some("su"));
    assert_eq!(asking(&process("run0", "/usr/bin/run0")), Some("run0"));
    assert_eq!(asking(&process("sudoku", "/usr/local/bin/sudoku")), None);
    assert_eq!(asking(&process("bash", "/bin/bash")), None);
    assert_eq!(told("sudo"), "sudo isn't for agents: it was stopped. Ask the person to run it themselves.");
}

#[tokio::test(flavor = "multi_thread")]
async fn sudo_and_the_like_are_stopped_in_agents_commands_and_the_agent_is_told() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bin = dir.path().join("bin");
    std::fs::create_dir_all(&bin).expect("bin");
    for name in ["sudo", "doas", "su", "pkexec", "run0", "authopen"] {
        copy_named(&bin, "/bin/sleep", name);
    }
    copy_named(&bin, "/bin/bash", "osascript");

    // In the foreground, through pipes.
    let bash = tool(dir.path(), None);
    for name in ["sudo", "doas", "su", "pkexec", "run0", "authopen"] {
        let (said, took) = run(&bash, &format!("'{}' 30", bin.join(name).display())).await;
        assert!(said.contains(&told(name)), "{name}: {said}");
        assert!(took < Duration::from_secs(25), "{name} was stopped, not waited for: {took:?}");
    }
    // osascript only with administrator privileges.
    let osascript = bin.join("osascript");
    let (said, _) = run(&bash, &format!("'{}' -c 'sleep 1; echo plain; true' x 'display dialog'", osascript.display())).await;
    assert!(said.contains("plain") && !said.contains("isn't for agents"), "{said}");
    let (said, _) =
        run(&bash, &format!("'{}' -c 'sleep 2; true' x 'do shell script \"x\" with Administrator Privileges'", osascript.display())).await;
    assert!(said.contains(&told("osascript with administrator privileges")), "{said}");
    // Or in the script file it runs (Fugo 49010): AppleScript's words, JXA's
    // option, compiled AppleScript's parameter, by a path from its working
    // folder too. The copy of bash runs each as a script.
    let scripts = dir.path().join("scripts");
    std::fs::create_dir_all(&scripts).expect("scripts");
    for (file, text) in [
        ("admin.applescript", "# do shell script \"x\" with Administrator Privileges\nsleep 2\n"),
        ("admin.js", "# app.doShellScript('x', {administratorPrivileges: true})\nsleep 2\n"),
        ("admin.scpt", "FasdUAS=1; # sysoexec badm\nsleep 2\n"),
        ("plain.applescript", "# display dialog \"hi\"\nsleep 1; echo plain\n"),
    ] {
        std::fs::write(scripts.join(file), text).expect("script");
    }
    for command in [
        format!("'{}' '{}'", osascript.display(), scripts.join("admin.applescript").display()),
        format!("cd '{}' && '{}' admin.applescript", scripts.display(), osascript.display()),
        format!("'{}' '{}'", osascript.display(), scripts.join("admin.js").display()),
        format!("'{}' '{}'", osascript.display(), scripts.join("admin.scpt").display()),
    ] {
        let (said, _) = run(&bash, &command).await;
        assert!(said.contains(&told("osascript with administrator privileges")), "{command}: {said}");
    }
    let (said, _) = run(&bash, &format!("'{}' '{}'", osascript.display(), scripts.join("plain.applescript").display())).await;
    assert!(said.contains("plain") && !said.contains("isn't for agents"), "{said}");

    // Through a log, in the foreground and in the background.
    let (sender, mut finished) = tokio::sync::mpsc::unbounded_channel();
    let started: Arc<Mutex<Vec<factory_agent::Task>>> = Arc::default();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        environment: vec![("FACTORY_AGENT_ID".to_owned(), "lea".to_owned())],
        started: {
            let started = started.clone();
            Arc::new(move |task: &factory_agent::Task| started.lock().push(task.clone()))
        },
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    let bash = tool(dir.path(), Some(background));
    let (said, took) = run(&bash, &format!("echo before; '{}' 30; echo after", bin.join("sudo").display())).await;
    assert!(said.starts_with("before\n") && said.contains(&told("sudo")), "{said}");
    assert!(took < Duration::from_secs(25), "{took:?}");
    let args = json!({"command": format!("'{}' 30", bin.join("doas").display()), "run_in_background": true});
    bash.execute(&args, &CancellationToken::new(), &|_| {}).await.expect("started");
    let done = tokio::time::timeout(HANG, finished.recv()).await.expect("finished in time").expect("finished");
    assert!(done.tail.contains(&told("doas")), "{}", done.tail);

    // Anything else runs as it would.
    let (said, _) = run(&bash, "sleep 0.3; echo fine").await;
    assert!(said.starts_with("fine\n") && !said.contains("isn't for agents"), "{said}");
}

/// The processes running the file at `path`, found by the file they run.
fn running(path: &Path) -> Vec<i32> {
    let wanted = path.canonicalize().unwrap_or_else(|_| path.to_owned());
    #[cfg(target_os = "macos")]
    {
        let mut pids = vec![0 as libc::pid_t; 8192];
        let size = i32::try_from(std::mem::size_of_val(pids.as_slice())).unwrap_or(i32::MAX);
        // SAFETY: pids holds `size` bytes.
        unsafe { libc::proc_listallpids(pids.as_mut_ptr().cast(), size) };
        pids.into_iter()
            .filter(|pid| *pid > 0)
            .filter(|pid| {
                let mut file = vec![0_u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
                // SAFETY: file holds PROC_PIDPATHINFO_MAXSIZE bytes.
                let length = unsafe { libc::proc_pidpath(*pid, file.as_mut_ptr().cast(), libc::PROC_PIDPATHINFO_MAXSIZE as u32) };
                usize::try_from(length).is_ok_and(|length| length > 0 && Path::new(&*String::from_utf8_lossy(&file[..length])) == wanted)
            })
            .collect()
    }
    #[cfg(not(target_os = "macos"))]
    {
        std::fs::read_dir("/proc")
            .map(|entries| {
                entries
                    .flatten()
                    .filter_map(|entry| {
                        let pid: i32 = entry.file_name().to_str()?.parse().ok()?;
                        (std::fs::read_link(entry.path().join("exe")).ok()? == wanted).then_some(pid)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

/// A fake sudo (a copy of sleep) in a folder of its own, so each case
/// finds only its own.
fn fake_sudo(dir: &Path, case: &str) -> PathBuf {
    let bin = dir.join(case);
    std::fs::create_dir_all(&bin).expect("bin");
    copy_named(&bin, "/bin/sleep", "sudo")
}

/// Runs `script` with sh in a group of its own, with `$S` the fake sudo,
/// guarded as an agent's command is.
fn guarded(script: &str, sudo: &Path) -> (std::process::Child, Guard) {
    use std::os::unix::process::CommandExt;
    let child = std::process::Command::new("/bin/sh")
        .args(["-c", script])
        .env("S", sudo)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .process_group(0)
        .spawn()
        .expect("spawn");
    let guard = Guard::watch(child.id(), "lea", None);
    (child, guard)
}

/// How long the fake sudo at `path` ran, from when it was first seen to
/// when it was gone; zero if it was stopped before it was ever seen.
fn ran_for(path: &Path, guard: &Guard) -> Duration {
    let began = Instant::now();
    let mut seen: Option<Instant> = None;
    loop {
        let now = Instant::now();
        let alive = !running(path).is_empty();
        match (alive, seen) {
            (true, None) => seen = Some(now),
            (false, Some(at)) => return now.duration_since(at),
            (false, None) if guard.said().contains(&told("sudo")) => return Duration::ZERO,
            _ => {}
        }
        assert!(began.elapsed() < Duration::from_secs(10), "the fake sudo at {} was never stopped", path.display());
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Whether perl is there, which the cases that leave the group start
/// their session with (`setsid`, which a Mac lacks as a command).
fn perl() -> bool {
    std::process::Command::new("perl").args(["-e", "1"]).status().is_ok_and(|status| status.success())
}

/// Starts its arguments in a session (and group) of its own.
const SETSID: &str = "perl -e 'use POSIX; POSIX::setsid(); exec @ARGV or die'";

/// Fugo 46005: however a command starts sudo, it's stopped within 250 ms:
/// plain, forked and exec'd at once, exec'd in place, in a session of its
/// own, and after a double fork whose middle process lives long enough to
/// be seen.
#[test]
fn sudo_is_stopped_within_250_ms_however_a_command_starts_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut cases = vec![
        ("plain", "\"$S\" 30; echo after".to_owned()),
        ("forked", "\"$S\" 30 & wait".to_owned()),
        ("exec", "exec \"$S\" 30".to_owned()),
    ];
    if perl() {
        cases.push(("setsid", format!("{SETSID} \"$S\" 30")));
        cases.push(("double-fork", format!("( {SETSID} \"$S\" 30 & sleep 1 ); sleep 3")));
    }
    for (case, script) in cases {
        let sudo = fake_sudo(dir.path(), case);
        let (mut child, guard) = guarded(&script, &sudo);
        let took = ran_for(&sudo, &guard);
        assert!(took < Duration::from_millis(250), "{case}: sudo ran for {took:?}");
        assert_eq!(guard.said(), told("sudo"), "{case}");
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// One of 16 commands running at once is stopped as fast, and the other 15
/// are left alone.
#[test]
fn among_16_guarded_commands_the_one_that_starts_sudo_is_stopped_within_250_ms() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sudo = fake_sudo(dir.path(), "sixteen");
    let mut others: Vec<(std::process::Child, Guard)> = (0..15).map(|_| guarded("sleep 30", &sudo)).collect();
    std::thread::sleep(Duration::from_millis(300));
    let (mut child, guard) = guarded("\"$S\" 30 & wait", &sudo);
    let took = ran_for(&sudo, &guard);
    assert!(took < Duration::from_millis(250), "sudo ran for {took:?}");
    for (other, guard) in &mut others {
        assert!(other.try_wait().expect("try_wait").is_none() && guard.said().is_empty());
        let _ = other.kill();
        let _ = other.wait();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Fugo 46319: what can't be traced to a guarded command is never stopped,
/// such as the person's own command from a card, run as Claktory's child in
/// a session of its own, while agents' commands are guarded.
#[test]
fn sudo_outside_every_guarded_command_is_left_alone() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let sudo = fake_sudo(dir.path(), "card");
    let (mut agents, _guard) = guarded("sleep 30", &sudo);
    let mut card = std::process::Command::new("/bin/sh").args(["-c", "\"$S\" 1"]).env("S", &sudo).process_group(0).spawn().expect("spawn");
    let ended = card.wait().expect("wait");
    assert!(ended.success(), "the request's sudo was stopped: {ended:?}");
    let _ = agents.kill();
    let _ = agents.wait();
}

/// This process's CPU time so far, user and system.
fn cpu() -> Duration {
    // SAFETY: zeroed is a valid rusage, which getrusage fills.
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    // SAFETY: usage is a valid rusage to write.
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &raw mut usage) };
    let time = |value: libc::timeval| Duration::from_secs(value.tv_sec as u64) + Duration::from_micros(value.tv_usec as u64);
    time(usage.ru_utime) + time(usage.ru_stime)
}

/// Polpo's measurement (sudo-guard.md): what guarding 16 commands costs
/// among ~280 processes, against today's way of one look at the whole
/// machine per command every 100 ms. Run alone, by hand:
/// `cargo test -p factory-agent --test all asking::what_guarding -- --ignored --nocapture`
#[test]
#[ignore = "a measurement, run by hand"]
fn what_guarding_16_commands_costs() {
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().expect("tempdir");
    let sudo = fake_sudo(dir.path(), "cost");
    let mut crowd: Vec<std::process::Child> =
        (0..264).map(|_| std::process::Command::new("sleep").arg("120").process_group(0).spawn().expect("spawn")).collect();
    let mut guarded: Vec<(std::process::Child, Guard)> = (0..16).map(|_| guarded("sleep 120", &sudo)).collect();
    std::thread::sleep(Duration::from_secs(1));
    let window = Duration::from_secs(10);
    let before = cpu();
    std::thread::sleep(window);
    let now = cpu() - before;
    // The way before: each command's group found by its own look at every
    // process, every 100 ms.
    let groups: Vec<i32> = guarded.iter().map(|(child, _)| child.id() as i32).collect();
    let before = cpu();
    let began = Instant::now();
    while began.elapsed() < window {
        for group in &groups {
            for process in factory_agent::tools::asking::processes(*group) {
                std::hint::black_box(asking(&process));
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    // The guard runs on meanwhile: what it took in as long is taken off.
    let then = (cpu() - before).saturating_sub(now);
    let share = |spent: Duration| spent.as_secs_f64() / window.as_secs_f64() * 100.0;
    println!("16 guarded commands among {} processes: now {:.2}% of a core, before {:.2}%", 280, share(now), share(then));
    for (child, _) in &mut guarded {
        let _ = child.kill();
        let _ = child.wait();
    }
    for child in &mut crowd {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Abbacchio 47134: the real sudo, set-uid, runs as root at once, and the
/// guard must stop it however early it starts, though proc_pidinfo can't
/// read it then. It waits for a password from an askpass that only sleeps
/// (its output still sudo's, so sudo reads nothing until it's stopped):
/// none is ever tried, and -k leaves any cached one alone.
#[cfg(target_os = "macos")]
#[tokio::test(flavor = "multi_thread")]
async fn the_real_sudo_is_stopped_even_as_a_commands_first_program() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let sudo = Path::new("/usr/bin/sudo");
    let setuid = std::fs::metadata(sudo).is_ok_and(|meta| meta.uid() == 0 && meta.mode() & 0o4000 != 0);
    // Without a password to ask for, sudo wouldn't wait to be seen.
    let asks = std::process::Command::new(sudo).args(["-k", "-n", "true"]).output().is_ok_and(|output| !output.status.success());
    if !setuid || !asks {
        eprintln!("skipped: no set-uid sudo that asks for a password here");
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    let askpass = dir.path().join("askpass");
    std::fs::write(&askpass, "#!/bin/sh\nexec sleep 10 2>/dev/null\n").expect("askpass");
    std::fs::set_permissions(&askpass, std::fs::Permissions::from_mode(0o700)).expect("mode");
    // `export` is the shell's own, so sudo is still the first program.
    let with = format!("export SUDO_ASKPASS={};", askpass.display());
    let ask = "/usr/bin/sudo -k -A true";
    let bash = tool(dir.path(), None);
    for command in [ask.to_owned(), format!("exec {ask}"), format!("sleep 0.3; {ask}"), format!("echo first; {ask}; echo after")] {
        let command = format!("{with} {command}");
        let (said, took) = run(&bash, &command).await;
        assert!(said.contains(&told("sudo")), "{command}: {said}");
        assert!(took < Duration::from_secs(3), "{command}: {took:?}");
    }
    // In the background, through its log.
    let (sender, mut finished) = tokio::sync::mpsc::unbounded_channel();
    let background = factory_agent::Background {
        dir: dir.path().join("tasks"),
        environment: Vec::new(),
        started: Arc::new(|_| {}),
        finished: Arc::new(move |done| {
            let _ = sender.send(done);
        }),
    };
    let bash = tool(dir.path(), Some(background));
    let result = bash.execute(&json!({"command": format!("{with} {ask}"), "run_in_background": true}), &CancellationToken::new(), &|_| {}).await.expect("started");
    assert!(text(&result).starts_with("Started task-"), "{}", text(&result));
    let done = tokio::time::timeout(Duration::from_secs(5), finished.recv()).await.expect("it ended").expect("finished");
    assert!(done.tail.contains(&told("sudo")), "{}", done.tail);
}
