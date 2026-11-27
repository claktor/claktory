//! Handing the open vault's key from a backend to the next across an
//! update, so an open vault stays open (a vault protected by the account
//! would otherwise ask claktory.com after every update): only a crash or a
//! reboot locks it.
//!
//! Under the supervisor, each backend gets one end of a socket pair, its fd
//! named in `FACTORY_VAULT_FD` (`Holder` keeps the other). Before exiting
//! for an update, a backend writes its key there (`keep`); the supervisor
//! holds it, in a locked page, and writes it into the next backend's pair
//! before starting it, which reads it at once (`receive`). While the update
//! is on trial it keeps it for the backend a rollback brings back; once the
//! update passes (`release`), or without a trial, it forgets it. A backend
//! that replaces itself in place (no supervisor) hands it through a pair it
//! makes, inherited across exec (`for_exec`).
//!
//! Only processes Claktory starts this way get the fd: the backend makes it
//! close-on-exec at once, so agents' commands never inherit it. A key that
//! arrives is checked against the vault before it's used (`Vault::take`).
//!
//! On Windows the same messages go through two pipes whose backend's ends
//! only the backend the supervisor starts inherits (`windows::handoff`).
//! Windows has no `exec`, so a backend with no
//! supervisor hands nothing over: the vault opens again by itself, or asks.

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(unix)]
use std::sync::Mutex;

use zeroize::Zeroizing;

use super::memory::Locked;

/// The environment variable naming the fd of the backend's end.
pub const FD_ENV: &str = "FACTORY_VAULT_FD";

/// The backend's end, once taken at start.
#[cfg(unix)]
static CHANNEL: Mutex<Option<UnixStream>> = Mutex::new(None);

/// A key handed over: the vault it opens, and the key.
pub struct Handed {
    pub(crate) vault_id: String,
    pub(crate) vmk: Locked,
}

fn keep_message(vault_id: &str, vmk: &[u8; 32]) -> Option<Zeroizing<Vec<u8>>> {
    let id = vault_id.as_bytes();
    let length = u8::try_from(id.len()).ok()?;
    let mut message = Zeroizing::new(Vec::with_capacity(2 + id.len() + 32));
    message.push(b'K');
    message.push(length);
    message.extend_from_slice(id);
    message.extend_from_slice(vmk);
    Some(message)
}

/// Reads the messages in `bytes`, from the front, into `held`: a key kept
/// ('K'), or forgotten ('D'). Returns how many bytes it used.
fn parse(bytes: &[u8], held: &mut Option<Handed>) -> usize {
    let mut at = 0;
    loop {
        match bytes.get(at) {
            Some(b'D') => {
                *held = None;
                at += 1;
            }
            Some(b'K') => {
                let Some(&length) = bytes.get(at + 1) else { return at };
                let end = at + 2 + usize::from(length) + 32;
                if bytes.len() < end {
                    return at;
                }
                let id = &bytes[at + 2..at + 2 + usize::from(length)];
                let Ok(key) = <&[u8; 32]>::try_from(&bytes[end - 32..end]) else { return bytes.len() };
                *held = match (std::str::from_utf8(id), Locked::new(key)) {
                    (Ok(vault_id), Some(vmk)) => Some(Handed { vault_id: vault_id.to_owned(), vmk }),
                    _ => None,
                };
                at = end;
            }
            // What no Claktory writes: ignore the rest.
            Some(_) => return bytes.len(),
            None => return at,
        }
    }
}

#[cfg(unix)]
fn set_cloexec(fd: RawFd, on: bool) {
    // SAFETY: fcntl on an fd this process holds.
    unsafe {
        let flags = libc::fcntl(fd, libc::F_GETFD);
        if flags >= 0 {
            let flags = if on { flags | libc::FD_CLOEXEC } else { flags & !libc::FD_CLOEXEC };
            libc::fcntl(fd, libc::F_SETFD, flags);
        }
    }
}

/// At the backend's start, before it starts anything: takes its end of the
/// pair, keeps it from every process it starts, and returns the key handed
/// to it, if any.
#[cfg(unix)]
pub fn receive() -> Option<Handed> {
    let fd: RawFd = std::env::var(FD_ENV).ok()?.parse().ok()?;
    // SAFETY: only a socket counts; anything else the variable names is left alone.
    let is_socket = unsafe {
        let mut stat: libc::stat = std::mem::zeroed();
        libc::fstat(fd, &mut stat) == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFSOCK
    };
    if !is_socket {
        return None;
    }
    set_cloexec(fd, true);
    // SAFETY: the fd is a socket handed to this process, owned from here on.
    let mut stream = unsafe { UnixStream::from_raw_fd(fd) };
    let mut bytes = Zeroizing::new(Vec::new());
    if stream.set_nonblocking(true).is_ok() {
        let mut buffer = Zeroizing::new([0u8; 512]);
        loop {
            match stream.read(buffer.as_mut_slice()) {
                Ok(0) | Err(_) => break,
                Ok(read) => bytes.extend_from_slice(&buffer[..read]),
            }
        }
    }
    let _ = stream.set_nonblocking(false);
    let mut held = None;
    parse(&bytes, &mut held);
    *CHANNEL.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stream);
    held
}

/// On Windows: the key the supervisor wrote before starting this backend,
/// through the end `windows::handoff::seal` took first thing in `main`.
#[cfg(windows)]
pub fn receive() -> Option<Handed> {
    let bytes = crate::windows::handoff::take_waiting();
    let mut held = None;
    parse(&bytes, &mut held);
    held
}

/// Before exiting for an update: hands the key to whoever starts the next
/// backend. Whether there was someone to hand it to.
#[cfg(unix)]
pub(crate) fn keep(vault_id: &str, vmk: &[u8; 32]) -> bool {
    let Some(message) = keep_message(vault_id, vmk) else { return false };
    let mut channel = CHANNEL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    channel.as_mut().is_some_and(|stream| stream.write_all(&message).is_ok())
}

/// On Windows, through the backend's pipe up to the supervisor.
#[cfg(windows)]
pub(crate) fn keep(vault_id: &str, vmk: &[u8; 32]) -> bool {
    keep_message(vault_id, vmk).is_some_and(|message| crate::windows::handoff::write(&message))
}

/// The update passed its trial: the supervisor forgets the key.
#[cfg(unix)]
pub(crate) fn release() {
    let mut channel = CHANNEL.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(stream) = channel.as_mut() {
        let _ = stream.write_all(b"D");
    }
}

/// For a backend that becomes the new one in place: the end to leave open
/// across exec (its fd for `FD_ENV`), already holding the key. The other
/// end closes with the exec.
#[cfg(unix)]
pub(crate) fn for_exec(vault_id: &str, vmk: &[u8; 32]) -> std::io::Result<OwnedFd> {
    let (mut ours, theirs) = UnixStream::pair()?;
    let message = keep_message(vault_id, vmk).ok_or_else(|| std::io::Error::other("the vault's id is too long"))?;
    ours.write_all(&message)?;
    // Open until the exec closes it (close-on-exec), so the bytes stay readable.
    std::mem::forget(ours);
    set_cloexec(theirs.as_raw_fd(), false);
    Ok(theirs.into())
}

/// On Windows, the same through the backend's pipe.
#[cfg(windows)]
pub(crate) fn release() {
    let _ = crate::windows::handoff::write(b"D");
}

/// The supervisor's side: the key a backend kept, until the next takes it.
#[derive(Default)]
pub struct Holder {
    held: Option<Handed>,
    unread: Zeroizing<Vec<u8>>,
}

impl Holder {
    /// A pair for the next backend: the supervisor's end, and the backend's
    /// (to leave open in it, its fd in `FD_ENV`), already holding the key if
    /// there is one. `trial`: the backend is an update on trial, or the one
    /// before it coming back, so the key is kept for a rollback.
    #[cfg(unix)]
    pub fn channel(&mut self, trial: bool) -> std::io::Result<(UnixStream, OwnedFd)> {
        let (mut ours, theirs) = UnixStream::pair()?;
        if let Some(handed) = &self.held
            && let Some(message) = keep_message(&handed.vault_id, &handed.vmk)
        {
            ours.write_all(&message)?;
        }
        if !trial {
            self.held = None;
        }
        set_cloexec(theirs.as_raw_fd(), false);
        self.unread.clear();
        Ok((ours, theirs.into()))
    }

    /// On Windows: what to write into the next backend's pipe before it
    /// starts (`windows::handoff::spawn`), the key if there is one; `trial`
    /// as for `channel`.
    #[cfg(windows)]
    pub fn for_next(&mut self, trial: bool) -> Option<Zeroizing<Vec<u8>>> {
        let message = self.held.as_ref().and_then(|handed| keep_message(&handed.vault_id, &handed.vmk));
        if !trial {
            self.held = None;
        }
        self.unread.clear();
        message
    }

    /// What the backend wrote.
    pub fn read(&mut self, bytes: &[u8]) {
        self.unread.extend_from_slice(bytes);
        let used = parse(&self.unread, &mut self.held);
        self.unread.drain(..used);
    }

    /// Whether it holds a key.
    pub fn holds(&self) -> bool {
        self.held.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kept_key_is_held_until_released() {
        let mut holder = Holder::default();
        let message = keep_message("v1234", &[9; 32]).expect("message");
        // In pieces, as a socket may deliver it.
        holder.read(&message[..5]);
        assert!(!holder.holds());
        holder.read(&message[5..]);
        let held = holder.held.as_ref().expect("held");
        assert_eq!((held.vault_id.as_str(), *held.vmk), ("v1234", [9; 32]));
        holder.read(b"D");
        assert!(!holder.holds());
    }

    #[cfg(windows)]
    #[test]
    fn the_next_backend_gets_the_key_once_unless_on_trial() {
        let mut holder = Holder::default();
        assert!(holder.for_next(false).is_none());
        let message = keep_message("v1", &[3; 32]).expect("message");
        holder.read(&message);
        assert_eq!(holder.for_next(true).as_deref(), Some(&*message));
        assert!(holder.holds(), "kept for a rollback");
        assert_eq!(holder.for_next(false).as_deref(), Some(&*message));
        assert!(!holder.holds() && holder.for_next(false).is_none());
    }

    #[cfg(unix)]
    #[test]
    fn the_next_backend_reads_the_key_at_once() {
        let mut holder = Holder::default();
        holder.read(&keep_message("v1", &[3; 32]).expect("message"));
        // On trial it's kept for a rollback; otherwise handed over once.
        let (_ours, theirs) = holder.channel(true).expect("pair");
        assert!(holder.holds());
        let mut stream = UnixStream::from(theirs);
        stream.set_nonblocking(true).expect("nonblocking");
        let mut bytes = vec![0u8; 64];
        let read = stream.read(&mut bytes).expect("read");
        let mut held = None;
        parse(&bytes[..read], &mut held);
        assert_eq!(held.map(|held| *held.vmk), Some([3; 32]));
        let (_ours, _theirs) = holder.channel(false).expect("pair");
        assert!(!holder.holds());
    }
}
