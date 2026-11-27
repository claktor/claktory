//! `factoryd browser-runner selftest [--pid PID] [--secret FILE]…`: what
//! Chrome can't reach, tried from inside the runner's own namespaces with
//! Chrome's fence on, as claktory-browser (agent-browser.md R5; Fugo
//! 36536, 36609). It prints one JSON object, and ends 0 only when every
//! refusal holds. PID is a process of the person's outside (factoryd's,
//! say), FILE a file the browser's user must not read (the vault).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitCode, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

use super::{fence, namespace, xvfb};

pub(super) fn main(args: &[String]) -> ExitCode {
    let mut pid = None;
    let mut secrets = Vec::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        match (arg.as_str(), args.next()) {
            ("--pid", Some(value)) => pid = value.parse::<u32>().ok(),
            ("--secret", Some(value)) => secrets.push(value.clone()),
            _ => {
                eprintln!("usage: factoryd browser-runner selftest [--pid PID] [--secret FILE]…");
                return ExitCode::from(2);
            }
        }
    }
    // Before the namespace, so it can't see what it shouldn't: whether a
    // process of the person's is there to look for.
    let outside = pid.map(|pid| (pid, std::path::Path::new(&format!("/proc/{pid}")).exists()));
    if let Err(why) = namespace::enter() {
        println!("{}", json!({"ok": false, "namespace": why}));
        return ExitCode::FAILURE;
    }
    let mut checks: Vec<Value> = Vec::new();
    let mut ok = true;
    let mut check = |what: &str, refused_expected: bool, result: Result<String, String>| {
        let refused = result.is_err();
        ok &= refused == refused_expected;
        checks.push(json!({"what": what, "expected": if refused_expected { "refused" } else { "allowed" }, "refused": refused, "detail": result.unwrap_or_else(|why| why)}));
    };

    // The namespace (Fugo 36609).
    let own_proc = std::fs::read_link("/proc/self").map(|me| me.display().to_string()).map_err(|e| e.to_string());
    check("/proc/self is 1 (the runner's own /proc)", false, own_proc.and_then(|me| if me == "1" { Ok(me) } else { Err(me) }));
    let cmdline = std::fs::read("/proc/1/cmdline").map(|line| String::from_utf8_lossy(&line).replace('\0', " ")).map_err(|e| e.to_string());
    check("/proc/1 is the runner", false, cmdline.and_then(|line| if line.contains("browser-runner") { Ok(line) } else { Err(line) }));
    let mounted = std::fs::read_to_string("/proc/self/mounts").map_err(|e| e.to_string()).map(|mounts| {
        mounts.lines().rfind(|line| line.split_whitespace().nth(1) == Some("/proc")).unwrap_or("none").to_owned()
    });
    check("/proc's mount (hidepid where the kernel takes it)", false, mounted);
    let sockets = std::fs::read_to_string("/proc/self/mounts").map_err(|e| e.to_string()).and_then(|mounts| {
        mounts.lines().rfind(|line| line.split_whitespace().nth(1) == Some("/tmp/.X11-unix") && line.starts_with("tmpfs ")).map(str::to_owned).ok_or_else(|| "not its own".to_owned())
    });
    check("/tmp/.X11-unix is the runner's own (another runner can't take its screen)", false, sockets);
    check("Chrome's policy is the runner's own (the person's Chrome never reads it)", false, namespace::policy_in_place());
    check("the firewall refuses this computer's own addresses", false, super::firewall_holds().map(|()| "refused at each".to_owned()));
    check("the firewall refuses a public IPv6 address (no IPv6 beyond DNS)", false, super::no_ipv6_out());
    if let Some((pid, existed)) = outside {
        check(&format!("see the person's process {pid} (it exists outside: {existed})"), true, read_some(&format!("/proc/{pid}/cmdline")));
    }

    let root = std::path::Path::new(super::PROFILES).join(format!("u{}-selftest", std::env::var("SUDO_UID").unwrap_or_default()));
    let (tmp, _tmp_lock) = match super::private_folder(&root).map_err(|e| e.to_string()).and_then(|()| super::own_tmp(&std::env::temp_dir())) {
        Ok(made) => made,
        Err(why) => {
            println!("{}", json!({"ok": false, "setup": why}));
            return ExitCode::FAILURE;
        }
    };
    let (rules, abi) = match fence::rules(&fence::read_write(&root, &tmp), &fence::read_only()).and_then(|(rules, abi)| Ok((xvfb::above_stdio(rules)?, abi))) {
        Ok(made) => made,
        Err(why) => {
            println!("{}", json!({"ok": false, "landlock": why}));
            return ExitCode::FAILURE;
        }
    };

    // A program started as Chrome is, fenced: its capabilities.
    let fence_fd = std::os::fd::AsRawFd::as_raw_fd(&rules);
    let mut status = Command::new("/bin/cat");
    status.arg("/proc/self/status").stdin(Stdio::null()).stderr(Stdio::null());
    // SAFETY: only async-signal-safe calls between fork and exec.
    unsafe { status.pre_exec(move || super::fence_self(fence_fd)) };
    let capabilities = status.output().map_err(|e| e.to_string()).and_then(|output| {
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        let effective = text.lines().find(|line| line.starts_with("CapEff:")).unwrap_or("CapEff: ?").to_owned();
        if effective.trim_end().ends_with("0000000000000000") { Ok(effective) } else { Err(effective) }
    });
    check("a program started as Chrome has no capabilities", false, capabilities);

    // The rest from a fenced child of the runner's, as Chrome is.
    let results = fenced(fence_fd, &root, &secrets);
    super::remove_tmp(&tmp);
    let _ = std::fs::remove_dir_all(&root);
    match results {
        Ok(results) => {
            for (what, refused_expected, result) in results {
                check(&what, refused_expected, result);
            }
        }
        Err(why) => check("run the fenced checks", false, Err(why)),
    }
    println!("{}", json!({"ok": ok, "landlock_abi": abi, "checks": checks}));
    if ok { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

type Results = Vec<(String, bool, Result<String, String>)>;

/// Forks, fences the child as Chrome is, runs the checks there and reads
/// their results back.
fn fenced(fence_fd: std::os::fd::RawFd, root: &std::path::Path, secrets: &[String]) -> Result<Results, String> {
    let (read, write) = xvfb::pipe()?;
    // SAFETY: the runner here is single-threaded (no threads started yet).
    let child = unsafe { libc::fork() };
    if child < 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    if child == 0 {
        drop(read);
        let results = match super::fence_self(fence_fd) {
            Ok(()) => checks(root, secrets),
            Err(error) => vec![("apply the fence".to_owned(), false, Err(error.to_string()))],
        };
        let encoded: Vec<Value> = results.into_iter().map(|(what, refused, result)| json!([what, refused, result])).collect();
        let mut file = std::fs::File::from(write);
        let _ = file.write_all(Value::Array(encoded).to_string().as_bytes());
        // SAFETY: ending the forked child without running the parent's exit handlers.
        unsafe { libc::_exit(0) };
    }
    drop(write);
    let mut text = String::new();
    let _ = std::fs::File::from(read).read_to_string(&mut text);
    // SAFETY: our own child.
    unsafe { libc::waitpid(child, std::ptr::null_mut(), 0) };
    let parsed: Vec<(String, bool, Result<String, String>)> = serde_json::from_str::<Vec<(String, bool, Result<String, String>)>>(&text)
        .map_err(|error| format!("the fenced checks said nothing readable: {error}"))?;
    Ok(parsed)
}

/// The checks, inside the fence: (what, refusal expected, what happened).
fn checks(root: &std::path::Path, secrets: &[String]) -> Results {
    let mut results: Results = Vec::new();
    let mut add = |what: &str, refused: bool, result: Result<String, String>| results.push((what.to_owned(), refused, result));
    for secret in secrets {
        add(&format!("read {secret}"), true, read_some(secret));
    }
    add("list /home", true, std::fs::read_dir("/home").map(|entries| format!("{} entries", entries.count())).map_err(|e| e.to_string()));
    add("list /root", true, std::fs::read_dir("/root").map(|entries| format!("{} entries", entries.count())).map_err(|e| e.to_string()));
    add("read /etc/shadow", true, read_some("/etc/shadow"));
    add("write /tmp", true, std::fs::write("/tmp/claktory-browser-selftest", "x").map(|()| "written".to_owned()).map_err(|e| e.to_string()));
    add("write /var/tmp", true, std::fs::write("/var/tmp/claktory-browser-selftest", "x").map(|()| "written".to_owned()).map_err(|e| e.to_string()));
    add("write the profile", false, std::fs::write(root.join("selftest"), "x").map(|()| "written".to_owned()).map_err(|e| e.to_string()));
    add("unmount /proc", true, unmount_proc());
    add("TCP to 127.0.0.1:7420 (Claktory)", true, connect("127.0.0.1:7420"));
    add("TCP to 127.0.0.1:22", true, connect("127.0.0.1:22"));
    add("TCP to 1.1.1.1:8443", true, connect("1.1.1.1:8443"));
    add("TCP to 1.1.1.1:443", false, connect("1.1.1.1:443"));
    add("listen on TCP 127.0.0.1:9999", true, TcpListener::bind("127.0.0.1:9999").map(|_| "listening".to_owned()).map_err(|e| e.to_string()));
    results
}

fn read_some(path: &str) -> Result<String, String> {
    let mut bytes = [0u8; 16];
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let read = file.read(&mut bytes).map_err(|e| e.to_string())?;
    if read == 0 { Err("empty".to_owned()) } else { Ok(format!("read {read} bytes")) }
}

fn connect(address: &str) -> Result<String, String> {
    let address: SocketAddr = address.parse().map_err(|_| "bad address".to_owned())?;
    TcpStream::connect_timeout(&address, Duration::from_secs(5)).map(|_| "connected".to_owned()).map_err(|e| e.to_string())
}

fn unmount_proc() -> Result<String, String> {
    let proc = std::ffi::CString::new("/proc").unwrap_or_default();
    // SAFETY: a umount2 call with our C string; a success is the failure this checks for.
    if unsafe { libc::umount2(proc.as_ptr(), libc::MNT_DETACH) } == 0 {
        Ok("unmounted".to_owned())
    } else {
        Err(std::io::Error::last_os_error().to_string())
    }
}
