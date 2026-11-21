//! Running an agent's SQL: opening the database it means, deciding what it
//! may touch, and putting results in a compact form for the model.
//!
//! Every statement goes through `open`, whose authorizer is the one place
//! that allows or refuses reads and writes, table by table, so sharing a
//! database with other agents can hook in there later.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use rusqlite::hooks::{AuthAction, AuthContext, Authorization};
use rusqlite::types::{Value as SqlValue, ValueRef};
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};

use crate::home::Home;

/// What a call runs against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// The agent's own database, `<home>/agents/<id>/database.sqlite`.
    Own(String),
    /// Factory's own records, factory.db, read only.
    Factory,
}

/// How long one call may run before it's cut off.
const TIME_LIMIT: Duration = Duration::from_secs(10);
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);
/// The largest an agent's database may grow.
const MAX_BYTES: i64 = 1 << 30;
/// What db_query shows at most…
const ROWS_SHOWN: usize = 200;
const BYTES_SHOWN: usize = 16 * 1024;
const CELL_CHARS: usize = 300;
/// …and how far past that it counts.
const ROWS_COUNTED: usize = 10_000;

/// What a call says: its text for the model, and its details for the app's
/// view of it (counts and names). Reads as its text.
#[derive(Debug)]
pub(crate) struct Answer {
    pub text: String,
    pub details: Value,
}

impl Answer {
    fn new(text: impl Into<String>, details: Value) -> Answer {
        Answer { text: text.into(), details }
    }
}

impl std::ops::Deref for Answer {
    type Target = str;

    fn deref(&self) -> &str {
        &self.text
    }
}

pub(crate) fn path(home: &Home, target: &Target) -> PathBuf {
    match target {
        Target::Own(agent) => home.agent_dir(agent).join("database.sqlite"),
        Target::Factory => home.db_path(),
    }
}

/// Factory's tables no agent reads, even an admin: each agent's memory is
/// its own (the person's decision); pipes, whose messages are another
/// Claktory's untrusted text that agents read only fenced, through a pipe
/// granted to them (`crate::pipes`); and comments on files, which hold the
/// person's unsent drafts: a comment reaches an agent only when the person
/// sends it, and agents read sent ones with the Comments tools
/// (`crate::annotations`).
fn private(table: &str) -> bool {
    table.starts_with("memory_") || table == "pipes" || table.starts_with("pipe_") || table == "threads" || table.starts_with("thread_")
}

/// What `target`'s statements may do. Writes to factory.db are already
/// impossible: it's opened read only.
fn authorize(target: &Target, context: &AuthContext<'_>) -> Authorization {
    match context.action {
        AuthAction::Attach { .. } | AuthAction::Detach { .. } => Authorization::Deny,
        AuthAction::Read { table_name, .. } if *target == Target::Factory && private(table_name) => Authorization::Deny,
        AuthAction::Function { function_name } if function_name.eq_ignore_ascii_case("load_extension") => Authorization::Deny,
        _ => Authorization::Allow,
    }
}

/// Opens `target`; None when it's the agent's own database and it doesn't
/// exist yet and `create` is false.
pub(crate) fn open(home: &Home, target: &Target, create: bool) -> Result<Option<Connection>, String> {
    let path = path(home, target);
    let failed = |error: rusqlite::Error| format!("The database couldn't be opened: {error}");
    let mut db = match target {
        Target::Own(_) => {
            if !path.exists() {
                if !create {
                    return Ok(None);
                }
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent).map_err(|error| format!("The database couldn't be created: {error}"))?;
                }
            }
            let db = Connection::open(&path).map_err(failed)?;
            db.pragma_update(None, "journal_mode", "wal").map_err(failed)?;
            let page_size: i64 = db.pragma_query_value(None, "page_size", |row| row.get(0)).map_err(failed)?;
            db.pragma_update(None, "max_page_count", MAX_BYTES / page_size.max(512)).map_err(failed)?;
            db
        }
        Target::Factory => {
            let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
            let db = Connection::open_with_flags(&path, flags).map_err(failed)?;
            db.pragma_update(None, "query_only", true).map_err(failed)?;
            db
        }
    };
    db.busy_timeout(BUSY_TIMEOUT).map_err(failed)?;
    let deadline = Instant::now() + TIME_LIMIT;
    db.progress_handler(10_000, Some(move || Instant::now() > deadline)).map_err(failed)?;
    let target = target.clone();
    db.authorizer(Some(move |context: AuthContext<'_>| authorize(&target, &context))).map_err(failed)?;
    Ok(Some(db))
}

/// An error from SQLite, said so the agent can fix its call.
fn sql_error(error: &rusqlite::Error, target: &Target) -> String {
    let text = error.to_string();
    if text.contains("interrupted") {
        return format!("It ran longer than {}s and was stopped; narrow it (WHERE, LIMIT, an index).", TIME_LIMIT.as_secs());
    }
    if *target == Target::Factory && (text.contains("prohibited") || text.contains("not authorized")) {
        return format!(
            "{text}. Agents' memory (the memory_ tables), pipes (pipes, pipe_) and comments on files (threads, thread_) aren't readable, even by admin agents; comments sent to you are in comments_list."
        );
    }
    if text.contains("not authorized") {
        return format!("{text}: ATTACH and load_extension aren't allowed.");
    }
    if text.contains("cannot start a transaction within a transaction") {
        return "db_exec runs everything in one transaction of its own; leave out BEGIN and COMMIT.".to_owned();
    }
    if text.contains("database or disk is full") {
        return format!("Your database is full ({} GB at most); delete what you no longer need.", MAX_BYTES >> 30);
    }
    text
}

/// `params` as SQL values: a JSON array of null, numbers, strings and booleans.
fn params(params: Option<&Value>) -> Result<Vec<SqlValue>, String> {
    let Some(params) = params.filter(|params| !params.is_null()) else { return Ok(Vec::new()) };
    let Some(list) = params.as_array() else { return Err("params must be a list, e.g. [1, \"a\"], for ?1, ?2…".to_owned()) };
    list.iter()
        .map(|value| match value {
            Value::Null => Ok(SqlValue::Null),
            Value::Bool(on) => Ok(SqlValue::Integer(i64::from(*on))),
            Value::Number(number) => match number.as_i64() {
                Some(integer) => Ok(SqlValue::Integer(integer)),
                None => Ok(SqlValue::Real(number.as_f64().unwrap_or_default())),
            },
            Value::String(text) => Ok(SqlValue::Text(text.clone())),
            other => Err(format!("params can hold null, numbers, strings and booleans; got {other}.")),
        })
        .collect()
}

fn cell(value: ValueRef<'_>) -> String {
    let text = match value {
        ValueRef::Null => return "NULL".to_owned(),
        ValueRef::Integer(integer) => return integer.to_string(),
        ValueRef::Real(real) => return real.to_string(),
        ValueRef::Blob(blob) => return format!("<blob {} bytes>", blob.len()),
        ValueRef::Text(text) => String::from_utf8_lossy(text),
    };
    let mut text = text.replace('\\', "\\\\").replace('\n', "\\n").replace('\r', "\\r");
    if text.chars().count() > CELL_CHARS {
        text = text.chars().take(CELL_CHARS).collect::<String>() + "…";
    }
    text
}

/// One read-only statement's rows: a header line, then a line per row,
/// cells joined with " | ", capped, with how many more there were. Its
/// details: the columns, the rows shown, how many more, and `capped` when it
/// stopped counting them.
pub(crate) fn query(db: &Connection, target: &Target, sql: &str, args: Option<&Value>) -> Result<Answer, String> {
    let values = params(args)?;
    let mut statement = db.prepare(sql).map_err(|error| sql_error(&error, target))?;
    if !statement.readonly() {
        return Err("db_query only reads; change things with db_exec.".to_owned());
    }
    let columns: Vec<String> = statement.column_names().map(str::to_owned).collect();
    let mut rows = statement.query(rusqlite::params_from_iter(values)).map_err(|error| sql_error(&error, target))?;
    let mut out = columns.join(" | ");
    let (mut shown, mut more) = (0, 0);
    while let Some(row) = rows.next().map_err(|error| sql_error(&error, target))? {
        if shown < ROWS_SHOWN && out.len() < BYTES_SHOWN {
            out.push('\n');
            let cells: Vec<String> = (0..columns.len()).map(|index| row.get_ref(index).map(cell).unwrap_or_default()).collect();
            out.push_str(&cells.join(" | "));
            shown += 1;
        } else {
            more += 1;
            if shown + more >= ROWS_COUNTED {
                let _ = write!(out, "\n(+{more} or more rows; add LIMIT/OFFSET or narrow it)");
                return Ok(Answer::new(out, json!({"columns": columns, "rows": shown, "more": more, "capped": true})));
            }
        }
    }
    let details = json!({"columns": columns, "rows": shown, "more": more});
    if shown == 0 {
        return Ok(Answer::new(format!("No rows. Columns: {}.", columns.join(", ")), details));
    }
    if more > 0 {
        let _ = write!(out, "\n(+{more} more rows; add LIMIT/OFFSET or narrow it)");
    }
    Ok(Answer::new(out, details))
}

/// Runs `sql`, all or none, and says what it changed. With params it's one
/// statement; without, it may be several. Its details: how many rows it
/// `changed`, and the `last_id` it inserted.
pub(crate) fn exec(db: &mut Connection, target: &Target, sql: &str, args: Option<&Value>) -> Result<Answer, String> {
    let values = params(args)?;
    let before = db.total_changes();
    let transaction = db.transaction().map_err(|error| sql_error(&error, target))?;
    if values.is_empty() {
        transaction.execute_batch(sql).map_err(|error| sql_error(&error, target))?;
    } else {
        transaction.execute(sql, rusqlite::params_from_iter(values)).map_err(|error| sql_error(&error, target))?;
    }
    transaction.commit().map_err(|error| sql_error(&error, target))?;
    let changed = db.total_changes() - before;
    let mut details = json!({"changed": changed});
    if changed == 0 {
        return Ok(Answer::new("ok", details));
    }
    let mut said = format!("ok: {changed} row{} changed", if changed == 1 { "" } else { "s" });
    let last = db.last_insert_rowid();
    if last != 0 {
        let _ = write!(said, ", last id {last}");
        details["last_id"] = json!(last);
    }
    Ok(Answer::new(said, details))
}

pub(crate) struct Table {
    pub name: String,
    /// "table" or "view".
    pub kind: String,
    pub rows: u64,
    /// Name and declared type.
    pub columns: Vec<(String, String)>,
}

/// The tables and views, by name; those `target` can't read are left out.
pub(crate) fn tables(db: &Connection, target: &Target) -> Result<Vec<Table>, String> {
    let error = |error: rusqlite::Error| sql_error(&error, target);
    let mut statement = db
        .prepare("SELECT name, type FROM sqlite_schema WHERE type IN ('table', 'view') AND name NOT LIKE 'sqlite_%' ORDER BY name")
        .map_err(error)?;
    let named: Vec<(String, String)> =
        statement.query_map([], crate::store::row2).map_err(error)?.collect::<Result<_, _>>().map_err(error)?;
    let mut tables = Vec::new();
    for (name, kind) in named {
        if *target == Target::Factory && private(&name) {
            continue;
        }
        let quoted = format!("\"{}\"", name.replace('"', "\"\""));
        let mut info = db.prepare(&format!("PRAGMA table_info({quoted})")).map_err(error)?;
        let columns = info.query_map([], |row| Ok((row.get(1)?, row.get(2)?))).map_err(error)?.collect::<Result<_, _>>().map_err(error)?;
        let rows: i64 = db.query_row(&format!("SELECT COUNT(*) FROM {quoted}"), [], |row| row.get(0)).map_err(error)?;
        tables.push(Table { name, kind, rows: u64::try_from(rows).unwrap_or_default(), columns });
    }
    Ok(tables)
}

/// db_schema: every table and view in a line, or one table's definition.
/// Its details: how many `tables` there are, or the `table` asked for.
pub(crate) fn schema(db: &Connection, target: &Target, table: Option<&str>) -> Result<Answer, String> {
    let tables = tables(db, target)?;
    let Some(wanted) = table else {
        let details = json!({"tables": tables.len()});
        if tables.is_empty() {
            return Ok(Answer::new("No tables yet; create them with db_exec.", details));
        }
        let lines: Vec<String> = tables
            .iter()
            .map(|table| {
                let columns: Vec<String> = table
                    .columns
                    .iter()
                    .map(|(name, kind)| if kind.is_empty() { name.clone() } else { format!("{name} {kind}") })
                    .collect();
                let kind = if table.kind == "view" { "view, " } else { "" };
                format!("{} ({kind}{} rows): {}", table.name, table.rows, columns.join(", "))
            })
            .collect();
        return Ok(Answer::new(lines.join("\n"), details));
    };
    if !tables.iter().any(|table| table.name == wanted) {
        let names: Vec<&str> = tables.iter().map(|table| table.name.as_str()).collect();
        return Err(format!(
            "There's no table \"{wanted}\". Tables: {}.",
            if names.is_empty() { "none".to_owned() } else { names.join(", ") }
        ));
    }
    let error = |error: rusqlite::Error| sql_error(&error, target);
    let mut statement =
        db.prepare("SELECT sql FROM sqlite_schema WHERE tbl_name = ?1 AND sql IS NOT NULL ORDER BY type DESC, name").map_err(error)?;
    let sql: Vec<String> = statement.query_map([wanted], |row| row.get(0)).map_err(error)?.collect::<Result<_, _>>().map_err(error)?;
    Ok(Answer::new(sql.join(";\n") + ";", json!({"table": wanted})))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home() -> (Home, PathBuf) {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("factory-database-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        (Home::new(dir.clone()), dir)
    }

    #[test]
    fn an_agents_database_is_made_on_first_write_and_read_back_compactly() {
        let (home, _dir) = home();
        let own = Target::Own("scout".into());
        assert!(open(&home, &own, false).expect("open").is_none(), "nothing until the first write");
        let mut db = open(&home, &own, true).expect("open").expect("made");
        let made = exec(
            &mut db,
            &own,
            "CREATE TABLE leads (id INTEGER PRIMARY KEY, name TEXT, note TEXT); CREATE INDEX by_name ON leads (name);",
            None,
        );
        assert_eq!(made.as_deref(), Ok("ok"));
        let added = exec(&mut db, &own, "INSERT INTO leads (name, note) VALUES (?1, ?2)", Some(&serde_json::json!(["Ada", "two\nlines"])));
        assert_eq!(added.as_deref(), Ok("ok: 1 row changed, last id 1"));
        assert_eq!(query(&db, &own, "SELECT * FROM leads", None).as_deref(), Ok("id | name | note\n1 | Ada | two\\nlines"));
        assert_eq!(
            query(&db, &own, "SELECT * FROM leads WHERE id = ?1", Some(&serde_json::json!([9]))).as_deref(),
            Ok("No rows. Columns: id, name, note.")
        );
        assert_eq!(query(&db, &own, "DELETE FROM leads", None).unwrap_err(), "db_query only reads; change things with db_exec.");

        // All or none.
        let failed = exec(&mut db, &own, "INSERT INTO leads (name) VALUES ('Bo'); INSERT INTO nowhere VALUES (1);", None);
        assert!(failed.unwrap_err().contains("no such table: nowhere"));
        assert_eq!(query(&db, &own, "SELECT COUNT(*) AS n FROM leads", None).as_deref(), Ok("n\n1"));

        // Many rows: capped, with how many more.
        exec(
            &mut db,
            &own,
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 300) INSERT INTO leads (name) SELECT 'x' || i FROM n",
            None,
        )
        .expect("insert");
        let many = query(&db, &own, "SELECT id FROM leads", None).expect("query").text;
        assert_eq!(many.lines().count(), 1 + ROWS_SHOWN + 1);
        assert!(many.ends_with("(+101 more rows; add LIMIT/OFFSET or narrow it)"), "{many}");

        assert_eq!(schema(&db, &own, None).as_deref(), Ok("leads (301 rows): id INTEGER, name TEXT, note TEXT"));
        let one = schema(&db, &own, Some("leads")).expect("schema").text;
        assert!(one.starts_with("CREATE TABLE leads") && one.contains("CREATE INDEX by_name"), "{one}");
        let attach = exec(&mut db, &own, "ATTACH DATABASE '/tmp/elsewhere.db' AS other", None).unwrap_err();
        assert!(attach.contains("ATTACH and load_extension aren't allowed"), "{attach}");
        assert!(
            exec(&mut db, &own, "BEGIN; INSERT INTO leads (name) VALUES ('c'); COMMIT;", None).unwrap_err().contains("leave out BEGIN")
        );
    }

    #[test]
    fn factorys_records_are_read_only_and_keep_agents_memory_private() {
        let (home, dir) = home();
        std::fs::create_dir_all(&dir).expect("mkdir");
        let factory = Connection::open(home.db_path()).expect("open");
        factory
            .execute_batch(
                "PRAGMA journal_mode = wal; CREATE TABLE messages (agent TEXT, text TEXT); INSERT INTO messages VALUES ('rivet', 'hi'); CREATE TABLE memory_notes (agent TEXT, body TEXT); INSERT INTO memory_notes VALUES ('scout', 'secret'); CREATE TABLE threads (id INTEGER, agent TEXT); CREATE TABLE thread_comments (thread INTEGER, body TEXT, draft INTEGER); INSERT INTO thread_comments VALUES (1, 'not sent yet', 1); CREATE TABLE thread_files (path TEXT, text TEXT);",
            )
            .expect("schema");
        let target = Target::Factory;
        let mut db = open(&home, &target, false).expect("open").expect("there");
        assert_eq!(query(&db, &target, "SELECT * FROM messages", None).as_deref(), Ok("agent | text\nrivet | hi"));
        let memory = query(&db, &target, "SELECT body FROM memory_notes", None).unwrap_err();
        assert!(
            memory.contains("(the memory_ tables), pipes (pipes, pipe_) and comments on files (threads, thread_) aren't readable"),
            "{memory}"
        );
        // Comments on files hold the person's drafts: no table of them is read, joined or listed.
        for sql in [
            "SELECT body FROM thread_comments WHERE draft = 1",
            "SELECT * FROM threads",
            "SELECT text FROM thread_files",
            "SELECT m.text FROM messages m JOIN thread_comments c ON 1",
            "SELECT (SELECT body FROM thread_comments) AS b",
        ] {
            let refused = query(&db, &target, sql, None).unwrap_err();
            assert!(refused.contains("comments on files (threads, thread_) aren't readable"), "{sql}: {refused}");
        }
        assert!(exec(&mut db, &target, "INSERT INTO messages VALUES ('x', 'y')", None).is_err());
        assert_eq!(schema(&db, &target, None).as_deref(), Ok("messages (1 rows): agent TEXT, text TEXT"));
        // Factory goes on writing meanwhile.
        factory.execute("INSERT INTO messages VALUES ('rivet', 'again')", []).expect("write");
        assert_eq!(query(&db, &target, "SELECT COUNT(*) AS n FROM messages", None).as_deref(), Ok("n\n2"));
    }
}
