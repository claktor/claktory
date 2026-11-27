//! Uploads, the host's half: a file an agent gives a page is opened here,
//! checked to be a plain file of at most 100 MB inside the folders the
//! agent may upload from, and sent to its browser's runner, which stages
//! it (`browser_runner::uploads`) and says where. Only that path ever goes
//! to Chrome. The check is on the open file itself, by the path the
//! system gives for it, so a link swapped in after the caller looked
//! leads nowhere else.

use std::path::{Path, PathBuf};

use crate::browser_runner::wire::MAX_UPLOAD as CAP;

/// A file opened for an upload: the file, its name and its size.
#[derive(Debug)]
pub(crate) struct Opened {
    pub file: std::fs::File,
    pub name: String,
    pub bytes: u64,
}

/// `path`, opened for an upload when it's a plain file of at most [`CAP`]
/// inside one of `roots`; else why not, in words for the agent.
#[cfg(not(unix))]
pub(crate) fn open(_path: &Path, _roots: &[PathBuf]) -> Result<Opened, String> {
    Err("Claktory's browser can't be given files on this computer.".to_owned())
}

/// `path`, opened for an upload when it's a plain file of at most [`CAP`]
/// inside one of `roots`; else why not, in words for the agent.
#[cfg(unix)]
pub(crate) fn open(path: &Path, roots: &[PathBuf]) -> Result<Opened, String> {
    use std::os::unix::fs::OpenOptionsExt;
    let shown = path.display();
    // Never a link at the end, and never waiting on a pipe.
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .open(path)
        .map_err(|error| format!("Couldn't open {shown}: {error}"))?;
    let metadata = file.metadata().map_err(|error| format!("Couldn't read {shown}: {error}"))?;
    if !metadata.is_file() {
        return Err(format!("{shown} isn't a file."));
    }
    if metadata.len() > CAP {
        return Err(format!("{shown} is over the 100 MB a page can be given."));
    }
    let real = real_path(&file).map_err(|error| format!("Couldn't read {shown}: {error}"))?;
    let inside = roots.iter().filter_map(|root| std::fs::canonicalize(root).ok()).any(|root| real.starts_with(&root));
    if !inside {
        return Err(format!("{shown} isn't in your folder or your uploads folder: a page can be given only files from those."));
    }
    let name = real.file_name().and_then(|name| name.to_str()).filter(|name| crate::browser_runner::wire::upload_name_ok(name));
    let Some(name) = name.map(str::to_owned) else {
        return Err(format!("{shown}'s name can't go to a page: rename it, then try again."));
    };
    Ok(Opened { file, name, bytes: metadata.len() })
}

/// Where the system says the open `file` is.
#[cfg(target_os = "linux")]
fn real_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))
}

/// Where the system says the open `file` is.
#[cfg(target_os = "macos")]
fn real_path(file: &std::fs::File) -> std::io::Result<PathBuf> {
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;
    let mut buffer = vec![0u8; libc::PATH_MAX as usize];
    // SAFETY: F_GETPATH writes at most PATH_MAX bytes, NUL-terminated, into
    // a buffer of that size.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buffer.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(buffer.len());
    Ok(PathBuf::from(std::ffi::OsStr::from_bytes(&buffer[..end])))
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    struct Tmp(PathBuf);

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn only_a_plain_file_inside_the_agents_folders_is_opened() {
        let tmp = Tmp(std::env::temp_dir().join(format!("claktory-host-uploads-{}", std::process::id())));
        let _ = std::fs::remove_dir_all(&tmp.0);
        let (folder, uploads, elsewhere) = (tmp.0.join("folder"), tmp.0.join("uploads"), tmp.0.join("elsewhere"));
        for dir in [&folder, &uploads, &elsewhere] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(folder.join("cv.pdf"), b"cv").unwrap();
        std::fs::write(uploads.join("photo.jpg"), b"photo").unwrap();
        std::fs::write(elsewhere.join("id_ed25519"), b"key").unwrap();
        let roots = [folder.clone(), uploads.clone()];

        let opened = open(&folder.join("cv.pdf"), &roots).expect("in its folder");
        assert_eq!((opened.name.as_str(), opened.bytes), ("cv.pdf", 2));
        assert!(open(&uploads.join("photo.jpg"), &roots).is_ok(), "in its uploads folder");

        assert!(open(&elsewhere.join("id_ed25519"), &roots).unwrap_err().contains("isn't in your folder"));
        // Through a link: at the end, refused; on the way, where it leads.
        std::os::unix::fs::symlink(elsewhere.join("id_ed25519"), folder.join("key")).unwrap();
        assert!(open(&folder.join("key"), &roots).is_err());
        std::os::unix::fs::symlink(&elsewhere, folder.join("away")).unwrap();
        assert!(open(&folder.join("away").join("id_ed25519"), &roots).unwrap_err().contains("isn't in your folder"));
        assert!(open(&folder.join("..").join("elsewhere").join("id_ed25519"), &roots).is_err());

        assert!(open(&folder, &roots).unwrap_err().contains("isn't a file"));
        assert!(open(&folder.join("missing.pdf"), &roots).is_err());
        let big = std::fs::File::create(folder.join("big.mov")).unwrap();
        big.set_len(CAP + 1).unwrap();
        assert!(open(&folder.join("big.mov"), &roots).unwrap_err().contains("100 MB"));
    }
}
