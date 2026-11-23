//! Files apps read (#339 follow-up 4, Fugo's ruling): a CSV or TSV file in
//! the workspace or in one of a project's folders, as one read-only table,
//! `rows`, read again at every query, so it's live.
//!
//! A file's source id is `file:<place>:<path>`: `<place>` is `workspace`,
//! a project's id, or `<project id>/<folder id>`, and `<path>` the file's
//! path in that folder, with `/` between names. Another path is another
//! source, given on a card of its own: a file is private, whatever folder
//! it's in, since any agent can write anywhere with bash.
//!
//! What's read is held to more than its name, at every read:
//! - the path is walked from the place's folder one name at a time with
//!   openat and O_NOFOLLOW, so no symbolic link is followed anywhere in it,
//!   even one put there after the last read; no name may be empty, `.`,
//!   `..`, or start with `.` (.ssh, .aws, .config…);
//! - never inside Claktory's own folders but its workspace, compared by
//!   device and inode while walking, since a project's folder can be any
//!   folder, the home folder too;
//! - a regular file with one name (a hard link to factory.db would pass
//!   every check of its path), opened without waiting (a FIFO), at most
//!   10 MB, read from the descriptor that was checked.

use std::collections::HashSet;
use std::ffi::CString;
use std::io::Read as _;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};

use crate::config::Config;
use crate::home::Home;

/// A file source's id: this, its place, `:`, its path.
pub(crate) const PREFIX: &str = "file:";
/// The workspace's place.
pub(crate) const WORKSPACE: &str = "workspace";
/// The largest file read.
pub(crate) const MOST_BYTES: u64 = 10 * 1024 * 1024;
/// The most rows (past the header) and columns a file may have.
pub(crate) const MOST_ROWS: usize = 100_000;
const MOST_COLUMNS: usize = 200;
/// The longest path in a place.
const MOST_PATH: usize = 1024;
/// How deep a place's folder may be, looking for Claktory's own above it.
const MOST_DEPTH: usize = 256;
/// The table a file is.
pub(crate) const TABLE: &str = "rows";
const TIME_LIMIT: Duration = Duration::from_secs(10);

/// A file source's id, split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FileId {
    /// `workspace`, a project's id, or `<project>/<folder>`.
    pub place: String,
    /// In the place, `/` between names.
    pub path: String,
}

impl FileId {
    fn names(&self) -> Vec<&str> {
        self.path.split('/').collect()
    }

    /// Tab-separated (`.tsv`) rather than comma.
    fn tsv(&self) -> bool {
        self.path.to_ascii_lowercase().ends_with(".tsv")
    }
}

/// Whether `id` names a file.
pub(crate) fn is_file(id: &str) -> bool {
    id.starts_with(PREFIX)
}

/// `id` as a file source: its place and path, each well formed.
pub(crate) fn parse(id: &str) -> Result<FileId, String> {
    let how = "name it file:<place>:<path>, the place being workspace, a project's id or <project>/<folder>, such as file:web-site:reports/bugs.csv";
    let Some((place, path)) = id.strip_prefix(PREFIX).and_then(|rest| rest.split_once(':')) else {
        return Err(format!("\"{id}\" isn't a file: {how}."));
    };
    let parts: Vec<&str> = place.split('/').collect();
    if parts.len() > 2 || parts.iter().any(|part| part.is_empty() || part.starts_with('.') || part.contains('\0')) {
        return Err(format!("\"{place}\" isn't a place: {how}."));
    }
    if path.is_empty() || path.len() > MOST_PATH || path.contains('\0') {
        return Err(format!("\"{path}\" isn't a file's path in {place}: {how}."));
    }
    for name in path.split('/') {
        if name.is_empty() || name == "." || name == ".." {
            return Err(format!(
                "\"{path}\" has an empty name, . or .. in it: give the file's own path in {place}, such as reports/bugs.csv."
            ));
        }
        if name.starts_with('.') {
            return Err(format!(
                "\"{path}\" goes through {name}, a hidden name: apps never read files under a name that starts with a dot."
            ));
        }
    }
    let lower = path.to_ascii_lowercase();
    if !lower.ends_with(".csv") && !lower.ends_with(".tsv") {
        return Err(format!("\"{path}\" isn't a .csv or .tsv file: apps read only those."));
    }
    Ok(FileId { place: place.to_owned(), path: path.to_owned() })
}

/// The place's folder now (None for the workspace, which is the home's),
/// and its words ("Web Site", "App in Web Site", "the workspace").
fn found(config: &Config, place: &str) -> Result<(Option<PathBuf>, String), String> {
    if place == WORKSPACE {
        return Ok((None, "the workspace".to_owned()));
    }
    let (project, folder) = match place.split_once('/') {
        Some((project, folder)) => (project, Some(folder)),
        None => (place, None),
    };
    let Some(found) = config.project(project) else {
        return Err(format!("There's no project \"{project}\", so there's no file there."));
    };
    match folder {
        None => Ok((Some(found.path.clone()), found.name.clone())),
        Some(folder) => match found.folders.iter().find(|known| known.id == folder) {
            Some(known) => Ok((Some(known.path.clone()), format!("{} in {}", known.name, found.name))),
            None => Err(format!("{} has no folder \"{folder}\", so there's no file there.", found.name)),
        },
    }
}

/// The place's folder now, and its words.
pub(crate) fn place(config: &Config, home: &Home, place: &str) -> Result<(PathBuf, String), String> {
    let (folder, words) = found(config, place)?;
    Ok((folder.unwrap_or_else(|| home.workspace()), words))
}

/// How the person sees a file: "reports/bugs.csv in Web Site", its place
/// and its path there exactly; the id when it isn't one.
pub(crate) fn label(config: &Config, id: &str) -> String {
    let Ok(file) = parse(id) else { return id.to_owned() };
    match found(config, &file.place) {
        Ok((_, words)) => format!("{} in {words}", file.path),
        Err(_) => format!("{} in {}", file.path, file.place),
    }
}

/// What it holds, a line for the person, with where it is.
pub(crate) fn holds(config: &Config, id: &str) -> String {
    let Ok(file) = parse(id) else { return String::new() };
    match found(config, &file.place) {
        Ok((Some(folder), _)) => {
            format!("Whatever is in the file {}, which you or any of your agents may have written.", folder.join(&file.path).display())
        }
        Ok((None, words)) => {
            format!("Whatever is in the file {} in {words}, which you or any of your agents may have written.", file.path)
        }
        Err(why) => why,
    }
}

/// The most files ListAppFiles shows, and the most entries it looks at.
const MOST_CHOICES: usize = 200;
const MOST_LOOKED_AT: usize = 20_000;
/// How deep it looks in a place.
const MOST_LIST_DEPTH: usize = 12;

/// The files an app could be given (ListAppFiles): each place's CSV and TSV
/// files, never through a link, a hidden name, node_modules or target, nor
/// into Claktory's own folders (the workspace is a place of its own), nor
/// into another place (it lists its own), newest first, those whose label
/// has every word of `query`; and whether there were more.
pub(crate) fn choices(config: &Config, home: &Home, query: &str) -> (Vec<factory_protocol::v1::AppFileChoice>, bool) {
    let mut places: Vec<(String, PathBuf)> = vec![(WORKSPACE.to_owned(), home.workspace())];
    for project in config.projects.iter().filter(|project| !project.archived) {
        places.push((project.id.clone(), project.path.clone()));
        for folder in &project.folders {
            places.push((format!("{}/{}", project.id, folder.id), folder.path.clone()));
        }
    }
    let roots: HashSet<PathBuf> = places.iter().map(|(_, path)| path.clone()).collect();
    let homes: HashSet<Key> = crate::vault::import::homes(home.root())
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| (meta.dev(), meta.ino()))
        .collect();
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    let mut found = Vec::new();
    let mut looked = 0;
    let mut truncated = false;
    'places: for (place, root) in &places {
        let mut stack: Vec<(PathBuf, String, usize)> = vec![(root.clone(), String::new(), 0)];
        while let Some((dir, rel, depth)) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else { continue };
            for entry in entries.flatten() {
                looked += 1;
                if looked > MOST_LOOKED_AT {
                    truncated = true;
                    break 'places;
                }
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                if name.starts_with('.') || name == "node_modules" || name == "target" {
                    continue;
                }
                // Never following a link: the entry's own type.
                let Ok(meta) = std::fs::symlink_metadata(entry.path()) else { continue };
                let path = if rel.is_empty() { name.to_owned() } else { format!("{rel}/{name}") };
                if meta.is_dir() {
                    let full = entry.path();
                    if depth + 1 < MOST_LIST_DEPTH && !roots.contains(&full) && !homes.contains(&(meta.dev(), meta.ino())) {
                        stack.push((full, path, depth + 1));
                    }
                    continue;
                }
                if !meta.file_type().is_file() {
                    continue;
                }
                let id = format!("{PREFIX}{place}:{path}");
                if parse(&id).is_err() {
                    continue;
                }
                let label = label(config, &id);
                let lower = label.to_lowercase();
                if !words.iter().all(|word| lower.contains(word)) {
                    continue;
                }
                let why = if meta.len() > MOST_BYTES {
                    format!("It's over {} MB, more than an app can read.", MOST_BYTES >> 20)
                } else if meta.nlink() != 1 {
                    "It has other names on this computer (hard links), so apps can't read it.".to_owned()
                } else {
                    String::new()
                };
                found.push(factory_protocol::v1::AppFileChoice {
                    id,
                    label,
                    size_bytes: meta.len() as i64,
                    modified_ms: meta.mtime() * 1000 + meta.mtime_nsec() / 1_000_000,
                    why,
                });
            }
        }
    }
    found.sort_by(|a, b| b.modified_ms.cmp(&a.modified_ms).then_with(|| a.label.cmp(&b.label)));
    if found.len() > MOST_CHOICES {
        truncated = true;
        found.truncate(MOST_CHOICES);
    }
    (found, truncated)
}

/// The file `id` names, opened as the ruling says, and checked; not read.
pub(crate) fn check(config: &Config, home: &Home, id: &str) -> Result<(), String> {
    let file = parse(id)?;
    let (folder, words) = place(config, home, &file.place)?;
    open(home, &folder, &file, &words).map(|_| ())
}

/// The file `file` in `folder`, read now (at most `MOST_BYTES`).
pub(crate) fn read(home: &Home, folder: &Path, file: &FileId, words: &str) -> Result<Vec<u8>, String> {
    let (opened, _) = open(home, folder, file, words)?;
    let shown = format!("{} in {words}", file.path);
    let mut bytes = Vec::new();
    (&opened).take(MOST_BYTES + 1).read_to_end(&mut bytes).map_err(|error| format!("{shown} can't be read: {error}"))?;
    if bytes.len() as u64 > MOST_BYTES {
        return Err(format!("{shown} is over {} MB, more than an app can read.", MOST_BYTES >> 20));
    }
    Ok(bytes)
}

/// Where a walk is, as to Claktory's own folders.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Zone {
    /// Outside them, or in this Claktory's workspace.
    Free,
    /// In one of Claktory's own folders itself: only its workspace may
    /// come next.
    Home,
}

type Key = (u64, u64);

fn key_of(fd: &OwnedFd) -> std::io::Result<Key> {
    // SAFETY: an open descriptor and a zeroed stat to fill.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    if unsafe { libc::fstat(fd.as_raw_fd(), &mut stat) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    #[allow(clippy::unnecessary_cast)]
    Ok((stat.st_dev as u64, stat.st_ino as u64))
}

fn openat(dir: Option<&OwnedFd>, name: &[u8], flags: libc::c_int) -> std::io::Result<OwnedFd> {
    let name = CString::new(name).map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "a NUL in the path"))?;
    // SAFETY: a valid C string and, when given, an open folder; the
    // descriptor returned is owned here.
    let fd = unsafe {
        match dir {
            Some(dir) => libc::openat(dir.as_raw_fd(), name.as_ptr(), flags),
            None => libc::open(name.as_ptr(), flags),
        }
    };
    if fd < 0 { Err(std::io::Error::last_os_error()) } else { Ok(unsafe { OwnedFd::from_raw_fd(fd) }) }
}

/// Whether `name` in `dir` is a symbolic link itself (never followed).
fn is_link(dir: &OwnedFd, name: &str) -> bool {
    let Ok(name) = CString::new(name.as_bytes()) else { return false };
    // SAFETY: an open folder, a valid C string and a zeroed stat to fill.
    let mut stat: libc::stat = unsafe { std::mem::zeroed() };
    let found = unsafe { libc::fstatat(dir.as_raw_fd(), name.as_ptr(), &mut stat, libc::AT_SYMLINK_NOFOLLOW) };
    found == 0 && (stat.st_mode & libc::S_IFMT) == libc::S_IFLNK
}

const FOLDER: libc::c_int = libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC;

/// Why a place's folder couldn't be placed.
#[derive(Debug)]
enum Ancestry {
    /// More than the most folders above it, never reaching the root.
    TooDeep,
    Io(std::io::Error),
}

/// Where the place's folder `start` is, looking up from it: Free outside
/// Claktory's own folders or in this workspace, Home when it's one of them
/// itself (only its workspace may come next), None inside one but not its
/// workspace. Fails closed: past `most` folders up without an answer,
/// TooDeep.
fn ancestry(start: &OwnedFd, homes: &HashSet<Key>, workspace: Option<Key>, most: usize) -> Result<Option<Zone>, Ancestry> {
    let mut here = openat(Some(start), b".", FOLDER).map_err(Ancestry::Io)?;
    for depth in 0..most {
        let key = key_of(&here).map_err(Ancestry::Io)?;
        if Some(key) == workspace {
            return Ok(Some(Zone::Free));
        }
        if homes.contains(&key) {
            return Ok(if depth == 0 { Some(Zone::Home) } else { None });
        }
        let up = openat(Some(&here), b"..", FOLDER).map_err(Ancestry::Io)?;
        if key_of(&up).map_err(Ancestry::Io)? == key {
            // The root: outside all of them.
            return Ok(Some(Zone::Free));
        }
        here = up;
    }
    Err(Ancestry::TooDeep)
}

/// Opens `file` in `folder`: see the module's comment. Its metadata too.
fn open(home: &Home, folder: &Path, file: &FileId, words: &str) -> Result<(std::fs::File, std::fs::Metadata), String> {
    let shown = format!("{} in {words}", file.path);
    let homes: HashSet<Key> = crate::vault::import::homes(home.root())
        .iter()
        .filter_map(|path| std::fs::metadata(path).ok())
        .map(|meta| (meta.dev(), meta.ino()))
        .collect();
    let workspace = std::fs::metadata(home.workspace()).ok().map(|meta| (meta.dev(), meta.ino()));
    let inside = || format!("{shown} is inside Claktory's own folder, outside its workspace, so apps can't read it.");
    let failed = |error: std::io::Error, at: &str| match error.raw_os_error() {
        Some(libc::ELOOP) | Some(libc::EMLINK) => {
            format!("{shown} goes through a symbolic link ({at}), so apps can't read it: name the file itself, with no link in its path.")
        }
        Some(libc::ENOENT) => format!("There's no file {shown}."),
        Some(libc::ENOTDIR) => format!("{shown} can't be reached: {at} isn't a folder."),
        _ => format!("{shown} can't be opened: {error}"),
    };
    // The place's own folder, as the settings say (the person's choice);
    // then whether it's in Claktory's own folder, looking up from it.
    let start = openat(None, folder.as_os_str().as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC)
        .map_err(|error| format!("{words}'s folder can't be opened: {error}"))?;
    let mut zone = match ancestry(&start, &homes, workspace, MOST_DEPTH) {
        Ok(Some(zone)) => zone,
        Ok(None) => return Err(inside()),
        Err(Ancestry::TooDeep) => return Err(format!("{words}'s folder is too deep to check, so apps can't read files in it.")),
        Err(Ancestry::Io(error)) => return Err(format!("{words}'s folder can't be read: {error}")),
    };
    let names = file.names();
    let Some((last, folders)) = names.split_last() else { return Err(format!("There's no file {shown}.")) };
    let mut dir = start;
    for (index, name) in folders.iter().enumerate() {
        if zone == Zone::Home && *name != WORKSPACE {
            return Err(inside());
        }
        let at = folders[..=index].join("/");
        dir = match openat(Some(&dir), name.as_bytes(), FOLDER) {
            Ok(next) => next,
            // A link where a folder was asked for: macOS says ENOTDIR.
            Err(_) if is_link(&dir, name) => return Err(failed(std::io::Error::from_raw_os_error(libc::ELOOP), &at)),
            Err(error) => return Err(failed(error, &at)),
        };
        let key = key_of(&dir).map_err(|error| failed(error, &at))?;
        zone = if Some(key) == workspace {
            Zone::Free
        } else if homes.contains(&key) {
            Zone::Home
        } else if zone == Zone::Home {
            // Another Claktory's workspace, under its own home.
            return Err(inside());
        } else {
            Zone::Free
        };
    }
    if zone == Zone::Home {
        return Err(inside());
    }
    let opened = openat(Some(&dir), last.as_bytes(), libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC)
        .map_err(|error| failed(error, &file.path))?;
    let opened = std::fs::File::from(opened);
    let meta = opened.metadata().map_err(|error| format!("{shown} can't be read: {error}"))?;
    if !meta.file_type().is_file() {
        return Err(format!("{shown} isn't a file: apps read only regular files."));
    }
    if meta.nlink() != 1 {
        return Err(format!(
            "{shown} has other names on this computer (hard links), so it can't be known which file it is: apps read only a file with one name."
        ));
    }
    if meta.len() > MOST_BYTES {
        return Err(format!("{shown} is over {} MB, more than an app can read.", MOST_BYTES >> 20));
    }
    Ok((opened, meta))
}

/// A column's type, from every value it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Integer,
    Real,
    Text,
}

/// One record per line, as RFC 4180 has it (quotes, doubled quotes inside,
/// line breaks inside quotes), with `separator` between fields.
fn records(text: &str, separator: char) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut record: Vec<String> = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut started = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            if c == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    quoted = false;
                }
            } else {
                field.push(c);
            }
            continue;
        }
        match c {
            '"' if field.is_empty() => {
                quoted = true;
                started = true;
            }
            c if c == separator => {
                record.push(std::mem::take(&mut field));
                started = true;
            }
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                if started || !field.is_empty() {
                    record.push(std::mem::take(&mut field));
                    records.push(std::mem::take(&mut record));
                }
                started = false;
                if records.len() > MOST_ROWS + 1 {
                    return Err(format!("it has more than {MOST_ROWS} rows"));
                }
            }
            c => {
                field.push(c);
                started = true;
            }
        }
    }
    if quoted {
        return Err("a quote opened in it is never closed".to_owned());
    }
    if started || !field.is_empty() {
        record.push(field);
        records.push(record);
    }
    if records.len() > MOST_ROWS + 1 {
        return Err(format!("it has more than {MOST_ROWS} rows"));
    }
    Ok(records)
}

/// The header's names as columns: each one, "column_<n>" when empty, and
/// "_2", "_3"… after one already there (SQLite ignores case).
fn columns(header: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    header
        .iter()
        .enumerate()
        .map(|(index, name)| {
            let name = name.trim();
            let base = if name.is_empty() { format!("column_{}", index + 1) } else { name.to_owned() };
            let mut column = base.clone();
            let mut n = 2;
            while !seen.insert(column.to_lowercase()) {
                column = format!("{base}_{n}");
                n += 1;
            }
            column
        })
        .collect()
}

fn quote(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// The file's bytes as a database of one table, `rows`, read only from then
/// on: only reading `rows` is allowed, for 10 s.
pub(crate) fn table(bytes: &[u8], file: &FileId, shown: &str) -> Result<rusqlite::Connection, String> {
    let bytes = bytes.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(bytes);
    let text = std::str::from_utf8(bytes).map_err(|_| format!("{shown} isn't UTF-8 text, so it can't be read as a table."))?;
    let separator = if file.tsv() { '\t' } else { ',' };
    let records = records(text, separator).map_err(|why| format!("{shown} can't be read as a table: {why}."))?;
    let Some((header, rows)) = records.split_first() else {
        return Err(format!("{shown} is empty: it needs a header row naming its columns."));
    };
    if header.len() > MOST_COLUMNS {
        return Err(format!("{shown} has more than {MOST_COLUMNS} columns."));
    }
    let columns = columns(header);
    let width = columns.len();
    if let Some((index, _)) = rows.iter().enumerate().find(|(_, row)| row.len() > width) {
        return Err(format!("{shown} can't be read as a table: row {} has more fields than its header names ({width}).", index + 2));
    }
    let kinds: Vec<Kind> = (0..width)
        .map(|column| {
            let values = rows.iter().filter_map(|row| row.get(column)).map(|value| value.trim()).filter(|value| !value.is_empty());
            let mut kind = Kind::Integer;
            for value in values {
                if kind == Kind::Integer && value.parse::<i64>().is_err() {
                    kind = Kind::Real;
                }
                if kind == Kind::Real && !value.parse::<f64>().is_ok_and(f64::is_finite) {
                    return Kind::Text;
                }
            }
            kind
        })
        .collect();
    let failed = |error: rusqlite::Error| format!("{shown} couldn't be made a table: {error}");
    let mut db = rusqlite::Connection::open_in_memory().map_err(failed)?;
    let definition: Vec<String> = columns
        .iter()
        .zip(&kinds)
        .map(|(name, kind)| {
            let kind = match kind {
                Kind::Integer => "INTEGER",
                Kind::Real => "REAL",
                Kind::Text => "TEXT",
            };
            format!("{} {kind}", quote(name))
        })
        .collect();
    db.execute_batch(&format!("CREATE TABLE {TABLE} ({})", definition.join(", "))).map_err(failed)?;
    {
        let tx = db.transaction().map_err(failed)?;
        {
            let marks = vec!["?"; width].join(", ");
            let mut insert = tx.prepare(&format!("INSERT INTO {TABLE} VALUES ({marks})")).map_err(failed)?;
            for row in rows {
                let values = (0..width).map(|column| {
                    let value = row.get(column).map(|value| value.trim()).unwrap_or("");
                    if value.is_empty() {
                        return rusqlite::types::Value::Null;
                    }
                    match kinds[column] {
                        Kind::Integer => value.parse::<i64>().map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Integer),
                        Kind::Real => value.parse::<f64>().map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Real),
                        Kind::Text => rusqlite::types::Value::Text(row[column].clone()),
                    }
                });
                insert.execute(rusqlite::params_from_iter(values)).map_err(failed)?;
            }
        }
        tx.commit().map_err(failed)?;
    }
    db.pragma_update(None, "query_only", true).map_err(failed)?;
    let deadline = Instant::now() + TIME_LIMIT;
    db.progress_handler(10_000, Some(move || Instant::now() > deadline)).map_err(failed)?;
    db.authorizer(Some(|context: AuthContext<'_>| match context.action {
        AuthAction::Select | AuthAction::Recursive => Authorization::Allow,
        AuthAction::Function { function_name } if !function_name.eq_ignore_ascii_case("load_extension") => Authorization::Allow,
        AuthAction::Read { table_name, .. } if table_name == TABLE => Authorization::Allow,
        _ => Authorization::Deny,
    }))
    .map_err(failed)?;
    Ok(db)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_too_deep_to_place_is_refused_not_taken_as_outside() {
        let root = std::env::temp_dir().join(format!("files-ancestry-{}", std::process::id()));
        let deep = root.join("a/b/c/d");
        std::fs::create_dir_all(&deep).unwrap();
        let open = |path: &Path| openat(None, path.as_os_str().as_bytes(), libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC).unwrap();
        let key = |path: &Path| {
            let meta = std::fs::metadata(path).unwrap();
            (meta.dev(), meta.ino())
        };
        let none = HashSet::new();
        assert!(matches!(ancestry(&open(&deep), &none, None, 3), Err(Ancestry::TooDeep)));
        assert!(matches!(ancestry(&open(&deep), &none, None, 1000), Ok(Some(Zone::Free))));
        let home: HashSet<Key> = [key(&root.join("a"))].into();
        assert!(matches!(ancestry(&open(&deep), &home, None, 1000), Ok(None)));
        assert!(matches!(ancestry(&open(&root.join("a")), &home, None, 1000), Ok(Some(Zone::Home))));
        assert!(matches!(ancestry(&open(&deep), &home, Some(key(&root.join("a/b"))), 1000), Ok(Some(Zone::Free))));
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn ids_name_a_place_and_a_plain_path() {
        let file = parse("file:web-site:reports/bugs.csv").unwrap();
        assert_eq!((file.place.as_str(), file.path.as_str()), ("web-site", "reports/bugs.csv"));
        assert_eq!(parse("file:web-site/app:Data.TSV").unwrap().place, "web-site/app");
        for (id, why) in [
            ("file:web-site", "isn't a file"),
            ("activity", "isn't a file"),
            ("file::bugs.csv", "isn't a place"),
            ("file:a/b/c:bugs.csv", "isn't a place"),
            ("file:web-site:", "isn't a file's path"),
            ("file:web-site:a//bugs.csv", "an empty name"),
            ("file:web-site:../bugs.csv", ". or .."),
            ("file:web-site:./bugs.csv", ". or .."),
            ("file:web-site:.ssh/x.csv", "hidden name"),
            ("file:web-site:a/.bugs.csv", "hidden name"),
            ("file:web-site:bugs.txt", "isn't a .csv or .tsv"),
            ("file:web-site:/etc/x.csv", "an empty name"),
        ] {
            let error = parse(id).unwrap_err();
            assert!(error.contains(why), "{id}: {error}");
        }
    }

    fn rows(db: &rusqlite::Connection, sql: &str) -> Vec<Vec<rusqlite::types::Value>> {
        let mut statement = db.prepare(sql).unwrap();
        let width = statement.column_count();
        statement
            .query_map([], |row| (0..width).map(|index| row.get::<_, rusqlite::types::Value>(index)).collect())
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn a_csv_is_one_table_of_typed_columns_and_only_reading_it_is_allowed() {
        use rusqlite::types::Value::{Integer, Null, Real, Text};
        let file = parse("file:workspace:bugs.csv").unwrap();
        let text = "\u{feff}severity,count,ratio,note,\r\nhigh,3,0.5,\"a, b\",x\nlow,,1e3,\"say \"\"hi\"\"\nthere\"\n\nlow,2\n";
        let db = table(text.as_bytes(), &file, "bugs.csv").unwrap();
        let names = |db: &rusqlite::Connection| -> Vec<String> {
            db.prepare("SELECT * FROM rows").unwrap().column_names().map(str::to_owned).collect()
        };
        assert_eq!(names(&db), ["severity", "count", "ratio", "note", "column_5"]);
        assert_eq!(
            rows(&db, "SELECT * FROM rows"),
            [
                vec![Text("high".into()), Integer(3), Real(0.5), Text("a, b".into()), Text("x".into())],
                vec![Text("low".into()), Null, Real(1000.0), Text("say \"hi\"\nthere".into()), Null],
                vec![Text("low".into()), Integer(2), Null, Null, Null],
            ]
        );
        assert_eq!(rows(&db, "SELECT severity, sum(count) FROM rows GROUP BY 1 ORDER BY 1")[0], [Text("high".into()), Integer(3)]);
        for sql in ["DELETE FROM rows", "ATTACH 'x.db' AS x", "SELECT * FROM sqlite_master", "CREATE TABLE t (a)"] {
            assert!(db.prepare(sql).and_then(|mut statement| statement.execute([])).is_err(), "{sql}");
        }

        let tsv = parse("file:workspace:a.tsv").unwrap();
        let db = table(b"a\ta\tA\n1\t2\t3\n", &tsv, "a.tsv").unwrap();
        assert_eq!(names(&db), ["a", "a_2", "A_3"]);
        assert_eq!(rows(&db, "SELECT * FROM rows"), [[Integer(1), Integer(2), Integer(3)]]);

        for (text, why) in [("", "is empty"), ("a,b\n1,2,3\n", "row 2 has more fields"), ("a\n\"open\n", "never closed")] {
            let error = table(text.as_bytes(), &file, "bugs.csv").unwrap_err();
            assert!(error.contains(why), "{text:?}: {error}");
        }
        assert!(table(b"a\n\xff\n", &file, "bugs.csv").unwrap_err().contains("isn't UTF-8"));
        let many = format!("a\n{}", "1\n".repeat(MOST_ROWS + 1));
        assert!(table(many.as_bytes(), &file, "bugs.csv").unwrap_err().contains("more than 100000 rows"));
    }
}
