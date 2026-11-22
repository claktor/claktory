//! Which Claktory this helper belongs to, fixed when it's built: the real
//! one, or Claktory Dev (the `dev` feature). Each has its own native
//! messaging host name, socket folder, extension and app, so a Dev helper
//! never talks to the real app, nor the real helper to Claktory Dev. Nothing
//! here can be changed at run time.

use std::path::{Path, PathBuf};

use crate::peer::{self, Signature};

pub struct Flavor {
    /// The native messaging host, as Chrome's manifests name it.
    pub host: &'static str,
    /// The app's folder in Application Support.
    pub folder: &'static str,
    /// The extension Chrome starts this helper for; empty until the Chrome
    /// Web Store assigns it, which refuses every caller meanwhile.
    pub extension: &'static str,
    /// The app's bundle identifier.
    pub app: &'static str,
    /// This helper's own signing identifier.
    pub helper: &'static str,
}

#[cfg(not(feature = "dev"))]
pub const FLAVOR: Flavor = Flavor {
    host: "com.claktory.browser",
    folder: "Claktory",
    // Given by the Chrome Web Store on the first upload (Pericolo).
    extension: "",
    app: "dev.factory.app",
    helper: "com.claktory.browser-host",
};

#[cfg(feature = "dev")]
pub const FLAVOR: Flavor = Flavor {
    host: "com.claktory.browser.dev",
    folder: "Claktory Dev",
    extension: "jpdkjilehjhinhohikojjngehpbakcmo",
    app: "dev.factory.app.dev",
    helper: "com.claktory.browser-host.dev",
};

impl Flavor {
    /// The origin Chrome passes when the extension starts this helper.
    pub fn caller(&self) -> Option<String> {
        (!self.extension.is_empty()).then(|| format!("chrome-extension://{}/", self.extension))
    }

    /// The app's socket, under the user's own home (from the user database,
    /// not the environment).
    pub fn socket(&self) -> Result<PathBuf, String> {
        Ok(home()?.join("Library/Application Support").join(self.folder).join("browser/broker.sock"))
    }

    /// Chrome's user-level manifest for this helper.
    pub fn manifest(&self) -> Result<PathBuf, String> {
        Ok(home()?.join("Library/Application Support/Google/Chrome/NativeMessagingHosts").join(format!("{}.json", self.host)))
    }

    /// The code requirement the app must meet, from this helper's own
    /// signature: the same Developer ID team and the app's identifier. In a
    /// Dev build signed ad hoc, the app bundle this helper sits in, pinned by
    /// its cdhash. Anything else (unsigned, no team, ad hoc in a real build)
    /// has no requirement, and the helper refuses to run.
    pub fn requirement(&self, own: &Signature, exe: &Path) -> Result<String, String> {
        if own.identifier != self.helper {
            return Err(format!("this helper is signed as {}, not {}", own.identifier, self.helper));
        }
        if let Some(team) = &own.team
            && !own.adhoc
        {
            return Ok(format!(
                "anchor apple generic and certificate leaf[subject.OU] = {} and identifier {}",
                peer::quoted(team)?,
                peer::quoted(self.app)?
            ));
        }
        if cfg!(feature = "dev") && own.adhoc {
            let bundle = bundle_of(exe).ok_or("a Dev helper signed ad hoc works only inside Claktory Dev's Contents/Helpers")?;
            let app = peer::of_path(&bundle)?;
            if app.identifier != self.app {
                return Err(format!("{} is {}, not {}", bundle.display(), app.identifier, self.app));
            }
            return Ok(format!("identifier {} and cdhash H\"{}\"", peer::quoted(self.app)?, app.cdhash_hex()));
        }
        Err("this helper isn't signed with a Developer ID team".into())
    }
}

/// The app bundle holding `exe` at Contents/Helpers.
pub fn bundle_of(exe: &Path) -> Option<PathBuf> {
    let helpers = exe.parent()?;
    let contents = helpers.parent()?;
    let bundle = contents.parent()?;
    let fits = helpers.file_name()? == "Helpers" && contents.file_name()? == "Contents" && bundle.extension()? == "app";
    fits.then(|| bundle.to_path_buf())
}

/// The user's home folder, as the user database has it.
pub fn home() -> Result<PathBuf, String> {
    use std::ffi::CStr;
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0 as libc::c_char; 16 * 1024];
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found: *mut libc::passwd = std::ptr::null_mut();
    let status = unsafe { libc::getpwuid_r(libc::getuid(), &mut entry, buffer.as_mut_ptr(), buffer.len(), &mut found) };
    if status != 0 || found.is_null() || entry.pw_dir.is_null() {
        return Err("no home folder for this user".into());
    }
    let dir = unsafe { CStr::from_ptr(entry.pw_dir) };
    let home = PathBuf::from(std::ffi::OsStr::from_bytes(dir.to_bytes()));
    if !home.is_absolute() {
        return Err("no home folder for this user".into());
    }
    Ok(home)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed(identifier: &str, team: Option<&str>, adhoc: bool) -> Signature {
        Signature { identifier: identifier.into(), team: team.map(Into::into), adhoc, cdhash: vec![0xab; 20] }
    }

    #[test]
    fn the_app_must_be_the_same_team_and_identifier() {
        let exe = Path::new("/Applications/Claktory.app/Contents/Helpers/claktory-browser-host");
        let requirement = FLAVOR.requirement(&signed(FLAVOR.helper, Some("ABCDE12345"), false), exe).unwrap();
        assert_eq!(
            requirement,
            format!("anchor apple generic and certificate leaf[subject.OU] = \"ABCDE12345\" and identifier \"{}\"", FLAVOR.app)
        );
        assert!(FLAVOR.requirement(&signed("com.example.other", Some("ABCDE12345"), false), exe).is_err(), "another helper's signature");
        assert!(FLAVOR.requirement(&signed(FLAVOR.helper, Some("AB\" or true"), false), exe).is_err(), "nothing breaks out of quotes");
        assert!(FLAVOR.requirement(&signed(FLAVOR.helper, None, false), exe).is_err(), "no team");
        if !cfg!(feature = "dev") {
            assert!(FLAVOR.requirement(&signed(FLAVOR.helper, None, true), exe).is_err(), "never ad hoc in a real build");
        }
    }

    #[test]
    fn the_bundle_is_the_one_whose_helpers_hold_it() {
        let bundle = bundle_of(Path::new("/A/Claktory Dev.app/Contents/Helpers/claktory-browser-host"));
        assert_eq!(bundle.as_deref(), Some(Path::new("/A/Claktory Dev.app")));
        assert!(bundle_of(Path::new("/A/Claktory Dev.app/Contents/MacOS/claktory-browser-host")).is_none());
        assert!(bundle_of(Path::new("/tmp/claktory-browser-host")).is_none());
        assert!(home().unwrap().is_absolute());
    }
}
