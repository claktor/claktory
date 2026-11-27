//! A report's files: a liaison
//! names up to 3 files in its pipe's inbox, and they're copied into the
//! pipe's report store, `<home>/pipes/reports/<pipe>/`, outside every project
//! and agent folder, under names Claktory makes (their SHA-256), so no
//! agent reaches them with its file tools. A reader opens one only through
//! handoff_read, which marks it as having read the pipe; a file's name is
//! the other side's words too, so it's shown cleaned and only where words
//! are. Each file is at most 2 MB, a pipe's at most 20 MB a day, and none
//! holds one of the vault's secrets.

use std::path::{Component, Path, PathBuf};

use rusqlite::params;

use super::schema::Refusal;
use super::{DAY_MS, Limits};
use crate::factory::Factory;
use crate::pipes::Record;

/// The longest name kept, in characters.
const NAME_CHARS: usize = 120;

/// A file as copied.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Taken {
    pub name: String,
    pub file: String,
    pub size: u64,
    pub image: Option<&'static str>,
}

/// The pipe's report store.
pub(crate) fn store(factory: &Factory, pipe: &str) -> PathBuf {
    factory.home.root().join("pipes").join("reports").join(pipe)
}


/// The file `handle` names in `inbox`: relative to it (or a path inside
/// it), with no `..`, never through a symbolic link, and a file.
fn resolve(inbox: &Path, handle: &str) -> Option<PathBuf> {
    let handle = Path::new(handle.trim());
    let relative = if handle.is_absolute() { handle.strip_prefix(inbox).ok()? } else { handle };
    let mut path = inbox.to_path_buf();
    if std::fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
        return None;
    }
    let mut any = false;
    for part in relative.components() {
        let Component::Normal(part) = part else { return None };
        path.push(part);
        any = true;
        if std::fs::symlink_metadata(&path).ok()?.file_type().is_symlink() {
            return None;
        }
    }
    (any && std::fs::symlink_metadata(&path).ok()?.is_file()).then_some(path)
}

/// What kind of image `bytes` is, by its first bytes.
fn image_kind(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() > 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn extension(image: Option<&str>) -> &'static str {
    match image {
        Some("image/png") => ".png",
        Some("image/jpeg") => ".jpg",
        Some("image/gif") => ".gif",
        Some("image/webp") => ".webp",
        _ => "",
    }
}

/// A file's name as shown: its last part, one line, without characters that
/// hide or reorder what's shown, and capped.
fn shown_name(path: &Path) -> String {
    let name = path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let name: String = super::schema::cleaned(&name, true).chars().filter(|character| !matches!(character, '/' | '\\')).take(NAME_CHARS).collect();
    if name.is_empty() { "file".to_owned() } else { name }
}

/// Copies the files `handles` names from the pipe's inbox into its report
/// store, after every check; refused as a whole when one fails, naming
/// `field`, the type's field for them.
pub(crate) fn take(factory: &Factory, record: &Record, handles: &[String], field: &str, limits: Limits, now: i64) -> Result<Vec<Taken>, Refusal> {
    if handles.is_empty() {
        return Ok(Vec::new());
    }
    let refused = |code: &'static str| Refusal { code, field: Some(field.to_owned()) };
    let limits = limits.effective();
    if handles.len() > limits.attachments_per_report as usize {
        return Err(refused("too_many"));
    }
    let inbox = super::super::folder_of(&factory.lock().config, record).ok_or(refused("invalid"))?.join("inbox");
    let most = u64::from(limits.attachment_mb) * 1024 * 1024;
    let mut files = Vec::new();
    for handle in handles {
        let path = resolve(&inbox, handle).ok_or(refused("invalid"))?;
        let size = std::fs::metadata(&path).map_err(|_| refused("invalid"))?.len();
        if size > most {
            return Err(refused("too_large"));
        }
        let bytes = std::fs::read(&path).map_err(|_| refused("invalid"))?;
        if bytes.len() as u64 > most {
            return Err(refused("too_large"));
        }
        files.push((shown_name(&path), bytes));
    }
    let today: i64 = factory
        .store
        .db()
        .query_row("SELECT COALESCE(SUM(size), 0) FROM pipe_handoff_files WHERE pipe = ?1 AND received > ?2", params![record.id, now - DAY_MS], |row| {
            row.get(0)
        })
        .unwrap_or(i64::MAX);
    let adding: u64 = files.iter().map(|(_, bytes)| bytes.len() as u64).sum();
    if u64::try_from(today).unwrap_or(u64::MAX).saturating_add(adding) > u64::from(limits.attachments_mb_per_day) * 1024 * 1024 {
        return Err(refused("over_limit"));
    }
    let contents: Vec<Vec<u8>> = files.iter().map(|(_, bytes)| bytes.clone()).collect();
    let names: String = files.iter().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join("\n");
    if crate::pipes::agents::secrets_in(factory, &names, &contents).is_some() {
        return Err(refused("secret"));
    }
    let store = store(factory, &record.id);
    crate::remote::private_dir(&store).map_err(|_| refused("not_kept"))?;
    let mut taken = Vec::new();
    for (name, bytes) in files {
        let image = image_kind(&bytes);
        let file = format!("{}{}", crate::pipes::agents::sha256(&bytes), extension(image));
        let path = store.join(&file);
        if !path.exists() && crate::remote::write_private(&path, &bytes).is_err() {
            return Err(refused("not_kept"));
        }
        taken.push(Taken { name, file, size: bytes.len() as u64, image });
    }
    Ok(taken)
}

/// Keeps `files` as report `report`'s, in place of any it had.
pub(crate) fn keep(factory: &Factory, pipe: &str, report: i64, files: &[Taken], now: i64) {
    let mut db = factory.store.db();
    let result = db.transaction().and_then(|transaction| {
        transaction.execute("DELETE FROM pipe_handoff_files WHERE report = ?1", [report])?;
        for (index, file) in files.iter().enumerate() {
            transaction.execute(
                "INSERT INTO pipe_handoff_files (report, idx, pipe, name, file, size, image, received) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![report, index as i64 + 1, pipe, file.name, file.file, file.size as i64, file.image.unwrap_or_default(), now],
            )?;
        }
        transaction.commit()
    });
    if let Err(error) = result {
        tracing::error!(%error, "could not keep a report's files");
    }
}

/// Report `report`'s files, in order.
pub(crate) fn of(factory: &Factory, report: i64) -> Vec<Taken> {
    let db = factory.store.db();
    let result = db.prepare("SELECT name, file, size, image FROM pipe_handoff_files WHERE report = ?1 ORDER BY idx").and_then(|mut statement| {
        statement
            .query_map([report], |row| {
                let image: String = row.get(3)?;
                Ok(Taken {
                    name: row.get(0)?,
                    file: row.get(1)?,
                    size: row.get::<_, i64>(2)?.try_into().unwrap_or_default(),
                    image: ["image/png", "image/jpeg", "image/gif", "image/webp"].into_iter().find(|known| *known == image),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()
    });
    result.unwrap_or_default()
}

/// Files no report holds any more go (reports go after 90 days), rows and
/// copies both.
pub(crate) fn prune(factory: &Factory) {
    if let Err(error) = factory.store.db().execute("DELETE FROM pipe_handoff_files WHERE report NOT IN (SELECT id FROM pipe_handoff_reports)", []) {
        tracing::error!(%error, "could not forget old reports' files");
        return;
    }
    let Ok(pipes) = std::fs::read_dir(factory.home.root().join("pipes").join("reports")) else { return };
    for pipe in pipes.flatten() {
        let pipe_id = pipe.file_name().to_string_lossy().into_owned();
        let kept: Vec<String> = {
            let db = factory.store.db();
            let result = db.prepare("SELECT file FROM pipe_handoff_files WHERE pipe = ?1").and_then(|mut statement| {
                statement.query_map([&pipe_id], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<Vec<_>>>()
            });
            match result {
                Ok(kept) => kept,
                Err(_) => continue,
            }
        };
        let Ok(files) = std::fs::read_dir(pipe.path()) else { continue };
        for file in files.flatten() {
            if !kept.contains(&file.file_name().to_string_lossy().into_owned()) {
                let _ = std::fs::remove_file(file.path());
            }
        }
    }
}

/// What a reader opening file `index` of `report` gets: an image, or its
/// text fenced, or that it can't be shown.
pub(crate) fn open(factory: &Factory, pipe: &str, report: i64, index: usize) -> Result<(Taken, Vec<u8>), String> {
    let files = of(factory, report);
    let file = files.get(index.wrapping_sub(1)).cloned().ok_or_else(|| format!("Report {report} has no file {index}."))?;
    let bytes = std::fs::read(store(factory, pipe).join(&file.file)).map_err(|_| "That file is gone.".to_owned())?;
    Ok((file, bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder removed when the test ends.
    struct Dir(PathBuf);

    impl Dir {
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn rand_suffix() -> u64 {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("random");
        u64::from_le_bytes(bytes)
    }

    #[test]
    fn a_handle_is_a_file_in_the_inbox_and_never_through_a_link() {
        let random: u64 = rand_suffix();
        let root = std::env::temp_dir().join(format!("handoff-files-{}-{random}", std::process::id()));
        let dir = Dir(root.clone());
        let inbox = dir.path().join("inbox");
        std::fs::create_dir_all(inbox.join("shots")).expect("inbox");
        std::fs::write(inbox.join("shots/a.png"), b"x").expect("file");
        std::fs::write(dir.path().join("secret.txt"), b"x").expect("file");
        assert_eq!(resolve(&inbox, "shots/a.png"), Some(inbox.join("shots/a.png")));
        assert_eq!(resolve(&inbox, &inbox.join("shots/a.png").display().to_string()), Some(inbox.join("shots/a.png")));
        assert_eq!(resolve(&inbox, "../secret.txt"), None);
        assert_eq!(resolve(&inbox, &dir.path().join("secret.txt").display().to_string()), None);
        assert_eq!(resolve(&inbox, "shots"), None);
        assert_eq!(resolve(&inbox, ""), None);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(dir.path().join("secret.txt"), inbox.join("link.txt")).expect("link");
            std::os::unix::fs::symlink(dir.path(), inbox.join("up")).expect("link");
            assert_eq!(resolve(&inbox, "link.txt"), None);
            assert_eq!(resolve(&inbox, "up/secret.txt"), None);
        }
    }

    #[test]
    fn names_are_cleaned_and_images_known_by_their_bytes() {
        assert_eq!(shown_name(Path::new("/x/re\u{202E}port\u{200B}.png")), "report.png");
        assert_eq!(image_kind(b"\x89PNG\r\n\x1a\nrest"), Some("image/png"));
        assert_eq!(image_kind(b"plain text"), None);
    }
}
