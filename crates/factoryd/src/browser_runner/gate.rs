//! The browser's gate, on Linux: the one program the person may run as the
//! browser's user through sudo (`sudo -n -u claktory-browser GATE
//! browser-gate run`, setup's only line). Anything running as the person
//! can use that line, agents' commands included, so the gate runs the
//! runner only for factoryd. The gate is a copy of factoryd that setup put
//! in a folder of root's; the runner at /opt/claktory-browser/runner is the
//! person's, so the gate trusts its bytes only once factoryd has signed
//! their hash.
//!
//! Before anything else, and reading no file but `KEY_FILE` and the
//! runner, nor its environment or working folder:
//! 1. its arguments are exactly `browser-gate run`;
//! 2. the key is read from `KEY_FILE`, root's alone;
//! 3. the runner is copied into a sealed memfd (no one can change those
//!    bytes after), and hashed there;
//! 4. 32 random bytes go out on stdout, and exactly 64 come back on stdin
//!    within `WITHIN`: factoryd's signature over `message`, which names
//!    the hash and the runner's arguments. Anything else, and it ends.
//!
//! Then it makes the runner's namespaces (`namespace`, which needs the
//! gate's path for AppArmor's user-namespace rule on Ubuntu) and, as PID 1
//! there, runs the sealed copy as `browser-runner run-gated`.
//!
//! What the gate and factoryd say to each other is `VERSION`: a factoryd
//! that speaks another asks for setup again, never signing for an older
//! gate.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};

/// The gate's protocol: the label, its arguments, its wire and what it
/// does before the runner (2: hides a seed that isn't root's alone). A
/// gate that says another (`browser-gate version`) is set up again.
pub const VERSION: u32 = 2;
/// The public half, hex, one line.
pub(crate) const KEY_FILE: &str = "/etc/claktory-browser/runner.pub";
/// What the signature covers first.
const LABEL: &[u8] = b"claktory browser gate v1\0";
/// The runner's arguments after its binary, as the gate runs it.
pub(crate) const RUNNER_ARGS: [&str; 2] = ["browser-runner", "run-gated"];
/// How long factoryd has to answer.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const WITHIN: std::time::Duration = std::time::Duration::from_secs(10);
/// The largest runner the gate copies.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const MAX_RUNNER: u64 = 512 * 1024 * 1024;

/// What factoryd signs: `LABEL`, the gate's 32 bytes, the runner's
/// SHA-256, then each of `RUNNER_ARGS` with a NUL after it.
pub(crate) fn message(nonce: &[u8; 32], runner: &[u8; 32]) -> Vec<u8> {
    let mut message = LABEL.to_vec();
    message.extend_from_slice(nonce);
    message.extend_from_slice(runner);
    for arg in RUNNER_ARGS {
        message.extend_from_slice(arg.as_bytes());
        message.push(0);
    }
    message
}

/// The public half in `KEY_FILE`'s words: 64 hex digits.
pub(crate) fn key_text(key: &VerifyingKey) -> String {
    key.as_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn parse_key(text: &str) -> Option<VerifyingKey> {
    let text = text.trim();
    if text.len() != 64 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let mut bytes = [0u8; 32];
    for (at, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[at * 2..at * 2 + 2], 16).ok()?;
    }
    VerifyingKey::from_bytes(&bytes).ok()
}

/// Whether `signature` is factoryd's for `nonce` and the runner `runner`.
pub(crate) fn verify(key: &VerifyingKey, nonce: &[u8; 32], runner: &[u8; 32], signature: &[u8; 64]) -> bool {
    key.verify(&message(nonce, runner), &Signature::from_bytes(signature)).is_ok()
}

/// Whether `path` and every folder above it are root's and writable by no
/// one else, none of them a link: what only root could have written.
#[cfg(unix)]
pub(crate) fn roots_alone(path: &std::path::Path) -> Result<(), String> {
    use std::os::unix::fs::MetadataExt;
    for each in path.ancestors() {
        if each.as_os_str().is_empty() {
            continue;
        }
        let meta = std::fs::symlink_metadata(each).map_err(|error| format!("{}: {error}", each.display()))?;
        if meta.file_type().is_symlink() {
            return Err(format!("{} is a link", each.display()));
        }
        if meta.uid() != 0 || meta.mode() & 0o022 != 0 {
            return Err(format!("{} isn't root's alone", each.display()));
        }
    }
    Ok(())
}

#[cfg(unix)]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn key_at(file: &std::path::Path) -> Result<VerifyingKey, String> {
    let setup_again = "run Claktory's browser setup again";
    roots_alone(file).map_err(|why| format!("the browser's key isn't in place ({why}): {setup_again}"))?;
    let text = std::fs::read_to_string(file).map_err(|error| format!("the browser's key can't be read ({error}): {setup_again}"))?;
    parse_key(&text).ok_or_else(|| format!("the browser's key doesn't read: {setup_again}"))
}

/// What the gate was asked to do.
#[derive(Debug, PartialEq)]
enum Asked {
    Run,
    Version,
}

/// Exactly `browser-gate run` or `browser-gate version` after the binary;
/// anything else is refused.
fn asked(args: &[std::ffi::OsString]) -> Option<Asked> {
    match args {
        [_, gate, what] if gate == "browser-gate" && what == "run" => Some(Asked::Run),
        [_, gate, what] if gate == "browser-gate" && what == "version" => Some(Asked::Version),
        _ => None,
    }
}

/// `factoryd browser-gate …`, all of `args` (the binary first): main's
/// first step, before logs, settings or a runtime.
pub fn main(args: &[std::ffi::OsString]) -> std::process::ExitCode {
    match asked(args) {
        Some(Asked::Version) => {
            println!("{VERSION}");
            std::process::ExitCode::SUCCESS
        }
        Some(Asked::Run) => run(),
        None => {
            eprintln!("usage: browser-gate run | version");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn run() -> std::process::ExitCode {
    eprintln!("Claktory's browser gate runs on Linux only.");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "linux")]
fn run() -> std::process::ExitCode {
    match linux::run() {
        Ok(never) => match never {},
        Err(linux::Ended::Before(why)) => {
            eprintln!("Claktory's browser gate ends: {why}");
            std::process::ExitCode::FAILURE
        }
        Err(linux::Ended::After(why)) => {
            super::fatal_before_start(&why);
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use std::convert::Infallible;
    use std::ffi::CString;
    use std::io::{Read, Write};
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::fs::{FileExt, OpenOptionsExt};
    use std::path::Path;

    use sha2::Digest;

    use super::{KEY_FILE, MAX_RUNNER, RUNNER_ARGS, WITHIN, key_at, verify};

    /// Why the gate ended: before factoryd's answer checked (said on
    /// stderr: factoryd reads no frame yet), or after (said as the wire
    /// does, as the runner would).
    pub(super) enum Ended {
        Before(String),
        After(String),
    }

    pub(super) fn run() -> Result<Infallible, Ended> {
        let key = key_at(Path::new(KEY_FILE)).map_err(Ended::Before)?;
        let runner = Sealed::copy(Path::new(crate::browser_host::setup::RUNNER)).map_err(Ended::Before)?;
        let mut nonce = [0u8; 32];
        getrandom::fill(&mut nonce).map_err(|error| Ended::Before(format!("no random challenge: {error}")))?;
        {
            let mut out = std::io::stdout().lock();
            out.write_all(&nonce).and_then(|()| out.flush()).map_err(|error| Ended::Before(format!("couldn't send the challenge: {error}")))?;
        }
        let signature = read_exactly::<64>(WITHIN).map_err(Ended::Before)?;
        if !verify(&key, &nonce, &runner.sha256, &signature) {
            return Err(Ended::Before("the answer to the gate's challenge isn't factoryd's for this runner".to_owned()));
        }
        // Checked: the rest is the runner's start, said on the wire.
        super::super::namespace::enter_for_gate().map_err(Ended::After)?;
        Err(Ended::After(runner.exec()))
    }

    /// The runner's bytes, in a memfd no one can change any more.
    struct Sealed {
        fd: OwnedFd,
        sha256: [u8; 32],
    }

    impl Sealed {
        fn copy(path: &Path) -> Result<Sealed, String> {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(path)
                .map_err(|error| format!("the runner can't be opened ({error}): Claktory puts it there before each start"))?;
            let meta = file.metadata().map_err(|error| format!("the runner can't be read: {error}"))?;
            if !meta.is_file() || meta.len() > MAX_RUNNER {
                return Err("the runner isn't a program Claktory put there".to_owned());
            }
            let fd = memfd()?;
            let mut copy = std::fs::File::from(fd);
            let copied = std::io::copy(&mut (&file).take(MAX_RUNNER + 1), &mut copy).map_err(|error| format!("couldn't copy the runner: {error}"))?;
            if copied > MAX_RUNNER {
                return Err("the runner isn't a program Claktory put there".to_owned());
            }
            let seals = libc::F_SEAL_SEAL | libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_WRITE;
            // SAFETY: fcntl on our own memfd.
            if unsafe { libc::fcntl(copy.as_raw_fd(), libc::F_ADD_SEALS, seals) } != 0 {
                return Err(format!("couldn't seal the runner's copy: {}", std::io::Error::last_os_error()));
            }
            let mut hash = sha2::Sha256::new();
            let mut buffer = vec![0u8; 1 << 20];
            let mut at = 0u64;
            loop {
                let read = copy.read_at(&mut buffer, at).map_err(|error| format!("couldn't read the runner's copy: {error}"))?;
                if read == 0 {
                    break;
                }
                hash.update(&buffer[..read]);
                at += read as u64;
            }
            Ok(Sealed { fd: OwnedFd::from(copy), sha256: hash.finalize().into() })
        }

        /// Runs the sealed copy in place of this process; returns only
        /// why it couldn't.
        fn exec(self) -> String {
            let argv0 = CString::new("claktory-browser-runner").unwrap_or_default();
            let args: Vec<CString> = RUNNER_ARGS.iter().map(|arg| CString::new(*arg).unwrap_or_default()).collect();
            let mut argv: Vec<*const libc::c_char> = vec![argv0.as_ptr()];
            argv.extend(args.iter().map(|arg| arg.as_ptr()));
            argv.push(std::ptr::null());
            // The person who ran it, as sudo says (and can't be told
            // otherwise): the runner keeps their profiles apart by it.
            let sudo_uid = std::env::var("SUDO_UID")
                .ok()
                .filter(|uid| !uid.is_empty() && uid.len() <= 10 && uid.bytes().all(|byte| byte.is_ascii_digit()))
                .and_then(|uid| CString::new(format!("SUDO_UID={uid}")).ok());
            let mut envp: Vec<*const libc::c_char> = sudo_uid.iter().map(|each| each.as_ptr()).collect();
            envp.push(std::ptr::null());
            let empty = CString::default();
            // SAFETY: execveat on our own memfd, with NUL-ended arrays of C
            // strings that live until it returns (only on failure).
            unsafe { libc::syscall(libc::SYS_execveat, self.fd.as_raw_fd(), empty.as_ptr(), argv.as_ptr(), envp.as_ptr(), libc::AT_EMPTY_PATH) };
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::EACCES) {
                return format!("Linux won't run the browser's runner from memory ({error}); vm.memfd_noexec must be 0 or 1");
            }
            format!("couldn't start the browser's runner: {error}")
        }
    }

    /// A memfd that can be sealed and run: MFD_EXEC where the kernel knows
    /// it (6.3 and later), without it before.
    fn memfd() -> Result<OwnedFd, String> {
        let name = CString::new("claktory-browser-runner").unwrap_or_default();
        let base = libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING;
        for flags in [base | libc::MFD_EXEC, base] {
            // SAFETY: memfd_create with a C string of ours.
            let fd = unsafe { libc::memfd_create(name.as_ptr(), flags) };
            if fd >= 0 {
                // SAFETY: a descriptor just made, ours alone.
                return Ok(unsafe { OwnedFd::from_raw_fd(fd) });
            }
            let error = std::io::Error::last_os_error();
            match error.raw_os_error() {
                Some(libc::EINVAL) if flags != base => continue,
                Some(libc::EACCES) => {
                    return Err(format!("Linux won't run programs from memory here ({error}), which Claktory's browser needs: vm.memfd_noexec must be 0 or 1"));
                }
                _ => return Err(format!("couldn't copy the runner: {error}")),
            }
        }
        Err("couldn't copy the runner".to_owned())
    }

    /// Exactly N bytes from stdin before `within` runs out, read straight
    /// from its descriptor (nothing buffered past them).
    fn read_exactly<const N: usize>(within: std::time::Duration) -> Result<[u8; N], String> {
        let until = std::time::Instant::now() + within;
        let mut bytes = [0u8; N];
        let mut got = 0;
        while got < N {
            let left = until.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return Err("no answer to the gate's challenge in time".to_owned());
            }
            let mut poll = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
            let wait = i32::try_from(left.as_millis()).unwrap_or(i32::MAX).max(1);
            // SAFETY: one pollfd of ours, for stdin.
            let ready = unsafe { libc::poll(&mut poll, 1, wait) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(format!("couldn't wait for the answer: {error}"));
            }
            if ready == 0 {
                continue;
            }
            // SAFETY: reads into the rest of our own buffer.
            let read = unsafe { libc::read(0, bytes[got..].as_mut_ptr().cast(), N - got) };
            match read {
                0 => return Err("the answer to the gate's challenge ended early".to_owned()),
                read if read < 0 => {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(format!("couldn't read the answer: {error}"));
                    }
                }
                read => got += read.unsigned_abs(),
            }
        }
        Ok(bytes)
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// The gate runs exactly the bytes it hashed: its copy can't be
        /// written, grown or cut once sealed, whatever happens to the
        /// runner's file after.
        #[test]
        fn the_runners_copy_is_sealed_and_hashed_as_it_runs() {
            let dir = std::env::temp_dir().join(format!("gate-seal-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let runner = dir.join("runner");
            std::fs::write(&runner, b"runner bytes").unwrap();
            let sealed = Sealed::copy(&runner).expect("copied");
            assert_eq!(sealed.sha256, <[u8; 32]>::from(sha2::Sha256::digest(b"runner bytes")));
            std::fs::write(&runner, b"other bytes").unwrap();
            let copy = std::fs::File::from(sealed.fd);
            assert!(copy.write_at(b"x", 0).is_err(), "written");
            assert!(copy.set_len(1).is_err(), "cut");
            assert!(copy.set_len(100).is_err(), "grown");
            let mut back = Vec::new();
            (&copy).read_to_end(&mut back).unwrap();
            assert_eq!(back, b"runner bytes");
            let link = dir.join("link");
            std::os::unix::fs::symlink(&runner, &link).unwrap();
            assert!(Sealed::copy(&link).is_err(), "a link isn't followed");
            assert!(Sealed::copy(&dir).is_err(), "a folder isn't a runner");
            std::fs::remove_dir_all(&dir).unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn only_a_signature_over_this_challenge_and_this_runner_checks() {
        let key = SigningKey::from_bytes(&[5; 32]);
        let public = key.verifying_key();
        let (nonce, runner) = ([9u8; 32], [7u8; 32]);
        let signed = key.sign(&message(&nonce, &runner)).to_bytes();
        assert!(verify(&public, &nonce, &runner, &signed));
        assert!(!verify(&public, &[8u8; 32], &runner, &signed), "a replay, for another challenge");
        assert!(!verify(&public, &nonce, &[6u8; 32], &signed), "another runner");
        let other = SigningKey::from_bytes(&[6; 32]).sign(&message(&nonce, &runner)).to_bytes();
        assert!(!verify(&public, &nonce, &runner, &other), "another key");
        assert!(!verify(&public, &nonce, &runner, &[0; 64]));
        assert_eq!(parse_key(&key_text(&public)), Some(public));
        assert_eq!(parse_key(&format!("{}\n", key_text(&public))), Some(public));
        assert_eq!(parse_key("zz"), None);
    }

    /// The gate's protocol is fixed: a change here is a new `VERSION`, and
    /// every gate set up again.
    #[test]
    fn what_factoryd_signs_is_the_gates_version_2() {
        let signed = message(&[1; 32], &[2; 32]);
        let mut expected = b"claktory browser gate v1\0".to_vec();
        expected.extend([1u8; 32]);
        expected.extend([2u8; 32]);
        expected.extend(b"browser-runner\0run-gated\0");
        assert_eq!(signed, expected);
        assert_eq!(VERSION, 2);
    }

    #[test]
    fn the_gate_takes_exactly_its_two_commands() {
        let args = |list: &[&str]| list.iter().map(std::ffi::OsString::from).collect::<Vec<_>>();
        assert_eq!(asked(&args(&["/usr/local/libexec/claktory-browser/gate", "browser-gate", "run"])), Some(Asked::Run));
        assert_eq!(asked(&args(&["gate", "browser-gate", "version"])), Some(Asked::Version));
        for refused in [
            &["gate", "browser-gate", "run", "--profile"][..],
            &["gate", "browser-gate"],
            &["gate", "browser-gate", "run "],
            &["gate", "browser-runner", "run"],
            &["gate", "browser-gate", "selftest"],
            &["gate"],
        ] {
            assert_eq!(asked(&args(refused)), None, "{refused:?}");
        }
    }

    /// The key must be root's alone; files a test can make are the user's,
    /// so they're refused, and a missing one says to run setup again.
    #[cfg(unix)]
    #[test]
    fn a_key_file_anyone_but_root_could_write_is_refused() {
        let dir = std::env::temp_dir().join(format!("gate-key-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("folder");
        let file = dir.join("runner.pub");
        std::fs::write(&file, key_text(&SigningKey::from_bytes(&[5; 32]).verifying_key())).expect("written");
        let refused = key_at(&file).expect_err("not root's");
        assert!(refused.contains("setup again"), "{refused}");
        let missing = key_at(&dir.join("none.pub")).expect_err("missing");
        assert!(missing.contains("setup again"), "{missing}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
