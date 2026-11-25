//! Pipe groups (#359): a project's pipes gathered under a named row in the
//! sidebar, which folds away the same on every device. Arrangement only: a
//! pipe in a group works exactly as one outside any. Kept in factory.db,
//! never in config.json. The person arranges them from the app; an admin
//! agent such as Rivet, with pipes_group (`crate::extensions::pipes`).

use std::collections::HashMap;
use std::sync::Arc;

use factory_protocol::v1;
use rusqlite::{OptionalExtension, params};

use crate::factory::{Factory, Inner, RequestError};
use crate::transcript::now_ms;

pub(crate) const SCHEMA: &str = "
-- Groups of a project's pipes in the sidebar (crate::pipes::groups), in
-- the order they were made.
CREATE TABLE pipe_groups (
    id TEXT PRIMARY KEY,
    project TEXT NOT NULL,
    name TEXT NOT NULL,
    collapsed INTEGER NOT NULL DEFAULT 0,
    position INTEGER NOT NULL,
    created INTEGER NOT NULL
);
-- The group each grouped pipe is in.
CREATE TABLE pipe_group_members (pipe TEXT PRIMARY KEY, grp TEXT NOT NULL);
";

/// The last group number given out, kept so a deleted group's id is never
/// given to another: a device still showing the deleted one would act on
/// the new one. Databases from before it start from their highest id.
pub(crate) const LAST: &str = "
-- The number of the last pipe group made (crate::pipes::groups): ids only
-- go up, never reused. One row, or none before the first.
CREATE TABLE pipe_group_last (n INTEGER NOT NULL);
";

/// The longest a group's name gets, in characters.
const NAME_CHARS: usize = 80;

/// A group as kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Group {
    pub id: String,
    pub project: String,
    pub name: String,
    pub collapsed: bool,
    pub position: u32,
    pub created: i64,
}

impl Group {
    fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<Group> {
        Ok(Group {
            id: row.get(0)?,
            project: row.get(1)?,
            name: row.get(2)?,
            collapsed: row.get(3)?,
            position: row.get(4)?,
            created: row.get(5)?,
        })
    }
}

const COLUMNS: &str = "id, project, name, collapsed, position, created";

/// Every group, in order.
pub(crate) fn all(factory: &Factory) -> Vec<Group> {
    let read = || -> rusqlite::Result<Vec<Group>> {
        let db = factory.store.db();
        let mut statement = db.prepare(&format!("SELECT {COLUMNS} FROM pipe_groups ORDER BY position, created, id"))?;
        statement.query_map([], Group::read)?.collect()
    };
    read().unwrap_or_else(|error| {
        tracing::error!(%error, "could not read the pipe groups");
        Vec::new()
    })
}

/// One group.
pub(crate) fn get(factory: &Factory, id: &str) -> Option<Group> {
    let db = factory.store.db();
    db.query_row(&format!("SELECT {COLUMNS} FROM pipe_groups WHERE id = ?1"), [id], Group::read).optional().ok().flatten()
}

/// The group of each grouped pipe, by pipe id.
pub(crate) fn members(factory: &Factory) -> HashMap<String, String> {
    let read = || -> rusqlite::Result<HashMap<String, String>> {
        let db = factory.store.db();
        // Pipes deleted since leave their rows behind: never counted.
        let mut statement = db.prepare("SELECT m.pipe, m.grp FROM pipe_group_members m JOIN pipes p ON p.id = m.pipe")?;
        statement.query_map([], crate::store::row2)?.collect()
    };
    read().unwrap_or_default()
}

/// Workspace.pipe_groups: the groups of projects that exist, in order.
pub(crate) fn view(factory: &Factory, inner: &Inner) -> Vec<v1::PipeGroup> {
    all(factory)
        .into_iter()
        .filter(|group| inner.config.project(&group.project).is_some())
        .map(|group| v1::PipeGroup {
            id: group.id,
            project_id: group.project,
            name: group.name,
            collapsed: group.collapsed,
            position: group.position,
            created_ms: group.created,
        })
        .collect()
}

/// A group's name as kept: one line, no control characters, at most
/// `NAME_CHARS`; refused when that leaves nothing.
fn name_of(name: &str) -> Result<String, RequestError> {
    let name = super::clean(name, NAME_CHARS);
    if name.is_empty() {
        return Err(RequestError::Pipe("Give the group a name.".to_owned()));
    }
    Ok(name)
}

/// Refuses a name another group of the project has.
fn unique(factory: &Factory, project: &str, name: &str, except: &str) -> Result<(), RequestError> {
    let taken = all(factory).into_iter().any(|group| group.project == project && group.id != except && group.name.eq_ignore_ascii_case(name));
    if taken {
        return Err(RequestError::Pipe(format!("This project already has a group named {name}.")));
    }
    Ok(())
}

fn known(factory: &Factory, id: &str) -> Result<Group, RequestError> {
    get(factory, id).ok_or(RequestError::NoPipeGroup)
}

fn changed(factory: &Factory) {
    factory.broadcast_workspace(&factory.lock());
}

/// CreatePipeGroup: a new group at the end of the project's, with `pipe`
/// moved into it when given (New group… from a pipe's menu).
pub(crate) fn create(factory: &Arc<Factory>, project: &str, name: &str, pipe: &str) -> Result<Group, RequestError> {
    if factory.lock().config.project(project).is_none() {
        return Err(RequestError::UnknownProject(project.to_owned()));
    }
    let name = name_of(name)?;
    unique(factory, project, &name, "")?;
    if !pipe.is_empty() {
        pipe_in(factory, pipe, project)?;
    }
    let before = members(factory).remove(pipe).unwrap_or_default();
    let group = {
        let mut db = factory.store.db();
        let db = db.transaction()?;
        // Past every number given out, deleted groups' too.
        let next: i64 = db.query_row(
            "SELECT MAX(COALESCE((SELECT MAX(n) FROM pipe_group_last), 0),
                        COALESCE((SELECT MAX(CAST(SUBSTR(id, 7) AS INTEGER)) FROM pipe_groups WHERE id LIKE 'group-%'), 0)) + 1",
            [],
            |row| row.get(0),
        )?;
        db.execute("DELETE FROM pipe_group_last", [])?;
        db.execute("INSERT INTO pipe_group_last (n) VALUES (?1)", [next])?;
        let position: u32 = db.query_row("SELECT COALESCE(MAX(position), -1) + 1 FROM pipe_groups WHERE project = ?1", [project], |row| row.get(0))?;
        let group = Group { id: format!("group-{next}"), project: project.to_owned(), name, collapsed: false, position, created: now_ms() };
        db.execute(
            &format!("INSERT INTO pipe_groups ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6)"),
            params![group.id, group.project, group.name, group.collapsed, group.position, group.created],
        )?;
        if !pipe.is_empty() {
            db.execute("INSERT INTO pipe_group_members (pipe, grp) VALUES (?1, ?2) ON CONFLICT (pipe) DO UPDATE SET grp = excluded.grp", [pipe, &group.id])?;
        }
        db.commit()?;
        group
    };
    tracing::info!(group = %group.id, project, "a pipe group was made");
    if !pipe.is_empty() {
        // Out of its group: that group's websites stop reaching it.
        super::shares::moved(factory, pipe, &before, &group.id);
    }
    changed(factory);
    Ok(group)
}

/// RenamePipeGroup.
pub(crate) fn rename(factory: &Factory, id: &str, name: &str) -> Result<Group, RequestError> {
    let group = known(factory, id)?;
    let name = name_of(name)?;
    unique(factory, &group.project, &name, id)?;
    factory.store.db().execute("UPDATE pipe_groups SET name = ?2 WHERE id = ?1", [id, &name])?;
    changed(factory);
    Ok(Group { name, ..group })
}

/// DeletePipeGroup: its pipes stay, outside any group. How many there were.
pub(crate) fn delete(factory: &Factory, id: &str) -> Result<(Group, usize), RequestError> {
    let group = known(factory, id)?;
    // Its websites stay with the pipes they reach now.
    super::shares::group_deleted(factory, id);
    let pipes = {
        let db = factory.store.db();
        let pipes: i64 = db.query_row(
            "SELECT COUNT(*) FROM pipe_group_members m JOIN pipes p ON p.id = m.pipe WHERE m.grp = ?1",
            [id],
            |row| row.get(0),
        )?;
        db.execute("DELETE FROM pipe_group_members WHERE grp = ?1", [id])?;
        db.execute("DELETE FROM pipe_groups WHERE id = ?1", [id])?;
        usize::try_from(pipes).unwrap_or(0)
    };
    tracing::info!(group = %id, pipes, "a pipe group was deleted; its pipes stay");
    changed(factory);
    Ok((group, pipes))
}

/// The pipe exists, in `project`.
fn pipe_in(factory: &Factory, pipe: &str, project: &str) -> Result<super::Record, RequestError> {
    let record = factory.pipes.get(pipe).ok_or_else(|| RequestError::NoPipe(pipe.to_owned()))?;
    if record.project != project {
        return Err(RequestError::Pipe("A pipe can only be in a group of its own project.".to_owned()));
    }
    Ok(record)
}

/// SetPipeGroup: moves a pipe into a group of its own project, or out of
/// any (an empty `group`). The group it's in now, if any.
pub(crate) fn set(factory: &Arc<Factory>, pipe: &str, group: &str) -> Result<Option<Group>, RequestError> {
    let record = factory.pipes.get(pipe).ok_or_else(|| RequestError::NoPipe(pipe.to_owned()))?;
    let before = members(factory).remove(pipe).unwrap_or_default();
    let group = if group.is_empty() {
        factory.store.db().execute("DELETE FROM pipe_group_members WHERE pipe = ?1", [pipe])?;
        None
    } else {
        let found = known(factory, group)?;
        pipe_in(factory, &record.id, &found.project)?;
        factory.store.db().execute(
            "INSERT INTO pipe_group_members (pipe, grp) VALUES (?1, ?2) ON CONFLICT (pipe) DO UPDATE SET grp = excluded.grp",
            [pipe, group],
        )?;
        Some(found)
    };
    // The websites of the group it left stop reaching it; those of the one
    // it joined reach it.
    super::shares::moved(factory, pipe, &before, group.as_ref().map_or("", |group| group.id.as_str()));
    changed(factory);
    Ok(group)
}

/// SetPipeGroupCollapsed: folded away or open, on every device.
pub(crate) fn set_collapsed(factory: &Factory, id: &str, collapsed: bool) -> Result<(), RequestError> {
    let group = known(factory, id)?;
    if group.collapsed != collapsed {
        factory.store.db().execute("UPDATE pipe_groups SET collapsed = ?2 WHERE id = ?1", params![id, collapsed])?;
        changed(factory);
    }
    Ok(())
}
