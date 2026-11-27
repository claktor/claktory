//! The app's token on Windows: how the person's own Claktory app on this PC
//! reaches its backend, in place of the Mac's local socket. Under authority
//! B it's never local person authority: the token alone gives the app
//! LocalUncontained, in Hello.token on the loopback port.
//!
//! `<home>\app-token` holds 64 lowercase hex characters and a newline. A
//! fresh one is written each time the backend starts, never reused, so a
//! token another account took (by listening on the port before the backend
//! did) is dead once the backend runs. It's written whole (a new private
//! file renamed over the old), so the app, which reads it again before
//! every try, sees the old token or the new, never part of one. Private as
//! the home is (this user and SYSTEM only); the backend keeps the value in
//! memory and never reads the file back.

use std::path::{Path, PathBuf};
use std::time::Duration;

use factory_agent::windows::private;
use factory_agent::windows::system::{hex, random_bytes};
use zeroize::Zeroizing;

/// The token's file in the home.
pub fn path(home: &Path) -> PathBuf {
    home.join("app-token")
}

/// Writes a new token for this start of the backend and returns it, for the
/// connection's check of Hello.token. 32 random bytes from the system's
/// generator.
pub fn rotate(home: &Path) -> std::io::Result<Zeroizing<String>> {
    let bytes = Zeroizing::new(random_bytes::<32>()?);
    let token = Zeroizing::new(hex(&bytes[..]));
    let contents = Zeroizing::new(format!("{}\n", token.as_str()));
    let file = path(home);
    // The app may have the file open to read it this very moment, which
    // stops a rename over it; it's only for as long as a read takes.
    let mut tries = 0;
    loop {
        match private::write(&file, contents.as_bytes()) {
            Err(error) if tries < 20 && error.kind() == std::io::ErrorKind::PermissionDenied => {
                tries += 1;
                std::thread::sleep(Duration::from_millis(50));
            }
            done => return done.map(|()| token),
        }
    }
}

/// At a clean stop: the token file goes, while it still holds `token`
/// (this start's), so an app that reads it while no backend runs waits for
/// the next start's instead of trying a dead one first. A
/// crash leaves it; the next start writes over it all the same. Nothing
/// ever accepts an old token.
pub fn forget(home: &Path, token: &str) {
    let file = path(home);
    let ours = std::fs::read_to_string(&file).is_ok_and(|text| text.trim_end() == token);
    if ours && let Err(error) = std::fs::remove_file(&file) {
        tracing::warn!(%error, "could not remove the app's token");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_start_writes_a_new_private_token() {
        let home = std::env::temp_dir().join(format!("claktory-token-{}", std::process::id()));
        super::super::prepare_home(&home).unwrap();
        let first = rotate(&home).expect("written");
        assert_eq!(first.len(), 64);
        assert!(first.chars().all(|character| matches!(character, '0'..='9' | 'a'..='f')));
        assert_eq!(std::fs::read_to_string(path(&home)).unwrap(), format!("{}\n", first.as_str()));
        assert!(factory_agent::windows::system::is_private_path(&path(&home)).unwrap(), "only this user and SYSTEM");
        // Held open to read, as the app might, while the next start writes.
        let reader = std::fs::File::open(path(&home)).unwrap();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(150));
            drop(reader);
        });
        let second = rotate(&home).expect("written again");
        releaser.join().unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read_to_string(path(&home)).unwrap().trim(), second.as_str());
        let leftovers = std::fs::read_dir(&home).unwrap().filter(|entry| entry.as_ref().unwrap().file_name() != "app-token").count();
        assert_eq!(leftovers, 0, "no temporary file stays behind");
        // A clean stop takes only its own token away.
        forget(&home, &first);
        assert!(path(&home).exists(), "another start's token stays");
        forget(&home, &second);
        assert!(!path(&home).exists());
        std::fs::remove_dir_all(&home).unwrap();
    }
}
