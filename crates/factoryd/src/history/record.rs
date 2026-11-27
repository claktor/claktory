//! Recording the Map: every time the Map the app is sent changes,
//! what changed is kept in `map_history`, with names, so History can play
//! this Claktory back as it was, deleted agents and all. One place covers
//! every way the setup changes (the app, Rivet editing the settings file,
//! Move, apps, the vault, pipes): the Workspace sent after each, whose Map
//! is built from the same rules agents follow (`crate::map`). Pipes aren't
//! in that Map (the app draws them from `Workspace.pipes`), so they're
//! added here: a node under their project, and an edge from each member.
//!
//! Rows (the backfill writes the same shape):
//! - `kind`: node_add, node_change, node_remove, edge_add, edge_change,
//!   edge_remove.
//! - `node`: a MapNode id ("agent:builder", "pipe:<id>"); `edge`:
//!   "<from>|<to>|<kind>", the kind's name (messages, lead, pipe…).
//! - `data`: JSON. A node's fields (kind as a name, label, detail,
//!   parent_id, project_id, ref_id, color, admin, badges, icon) when added,
//!   only those that changed when changed, `{}` when removed; an edge's
//!   both_ways and label.
//! - `why`: start (the first recording: the Map as it was then), config,
//!   vault, apps, pipes; backfill for what was reconstructed (`inferred`).
//!
//! The Map last recorded is kept in memory, rebuilt from the rows recorded
//! (never inferred ones) when first needed. Never pruned.

use std::collections::BTreeMap;
use std::sync::Mutex;

use factory_protocol::v1;
use rusqlite::{Connection, params};
use serde_json::{Map, Value, json};

use crate::factory::Factory;

pub(crate) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS map_history (
  seq INTEGER PRIMARY KEY,
  at INTEGER NOT NULL,
  kind TEXT NOT NULL,
  node TEXT,
  edge TEXT,
  data TEXT NOT NULL,
  why TEXT NOT NULL,
  inferred INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX IF NOT EXISTS map_history_at ON map_history (at);
-- When recording began, and when the backfill ran (0: not yet).
CREATE TABLE IF NOT EXISTS map_history_meta (
  one INTEGER PRIMARY KEY CHECK (one = 1),
  since INTEGER NOT NULL DEFAULT 0,
  backfilled INTEGER NOT NULL DEFAULT 0
);
";

/// A pipe's node says only this of the other side.
pub(super) const PIPE_DETAIL: &str = "Another Claktory";

type Fields = Map<String, Value>;

/// The Map as recorded: each node's and edge's fields, by id and key.
#[derive(Clone, Debug, Default, PartialEq)]
struct Shape {
    nodes: BTreeMap<String, Fields>,
    edges: BTreeMap<String, Fields>,
}

/// A pipe as the Workspace has it, to tell when one changed.
#[derive(Clone, Debug, PartialEq)]
struct PipeSeen {
    id: String,
    name: String,
    project: String,
    state: i32,
    members: Vec<String>,
    lead: String,
}

/// What was recorded last, and what it was made from.
struct Last {
    map: v1::ClaktoryMap,
    pipes: Vec<PipeSeen>,
    locked: bool,
    shape: Shape,
}

/// The Map last recorded; None until first needed.
#[derive(Default)]
pub(crate) struct Recorder {
    last: Mutex<Option<Last>>,
}

#[derive(Debug, PartialEq)]
struct Row {
    kind: &'static str,
    node: Option<String>,
    edge: Option<String>,
    data: Fields,
}

impl Recorder {
    /// Records what changed in the Map `workspace` shows since last time.
    /// Cheap when nothing did, which is nearly always.
    pub(crate) fn observe(&self, factory: &Factory, workspace: &v1::Workspace) {
        let Some(map) = &workspace.map else { return };
        let pipes = pipes_seen(workspace);
        let locked = workspace.vault.as_ref().is_some_and(|vault| vault.exists && !vault.unlocked);
        let mut last = self.last.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(last) = last.as_ref()
            && last.map == *map
            && last.pipes == pipes
            && last.locked == locked
        {
            return;
        }
        let label = |id: &str| factory.pipes.get(id).map_or_else(|| "Pipe".to_owned(), |record| pipe_label(&record));
        let db = factory.store.db();
        if let Err(error) = record(&db, &mut last, map, &pipes, locked, &label, crate::transcript::now_ms()) {
            tracing::error!(%error, "could not record a change to the Map");
        }
    }
}

/// The pipe's name only when this side chose it; never the other side's.
pub(super) fn pipe_label(record: &crate::pipes::Record) -> String {
    if record.named_here.unwrap_or(record.peer.is_none()) && !record.name.trim().is_empty() {
        record.name.trim().to_owned()
    } else {
        "Pipe".to_owned()
    }
}

/// The pipes that are connections: open or paused.
fn pipes_seen(workspace: &v1::Workspace) -> Vec<PipeSeen> {
    workspace
        .pipes
        .iter()
        .filter(|pipe| matches!(pipe.state(), v1::PipeState::Open | v1::PipeState::Paused))
        .map(|pipe| PipeSeen {
            id: pipe.id.clone(),
            name: pipe.name.clone(),
            project: pipe.project_id.clone(),
            state: pipe.state,
            members: pipe.member_ids.clone(),
            lead: pipe.lead_id.clone(),
        })
        .collect()
}

/// Records the difference between `last` (read from `db` when None) and
/// `map` with `pipes`, at `now`, in one transaction; `last` follows only
/// once it's written.
fn record(
    db: &Connection,
    last: &mut Option<Last>,
    map: &v1::ClaktoryMap,
    pipes: &[PipeSeen],
    locked: bool,
    label: &dyn Fn(&str) -> String,
    now: i64,
) -> rusqlite::Result<()> {
    let (before, since) = match last.as_ref() {
        Some(last) => (last.shape.clone(), 1),
        None => (replay(db)?, db.query_row("SELECT since FROM map_history_meta", [], |row| row.get(0)).unwrap_or(0)),
    };
    let mut shape = shape(map, pipes, label);
    // A locked vault shows what it can: keep its secrets as they were.
    if locked {
        shape.nodes.retain(|_, fields| kind(fields) != "secret");
        shape.edges.retain(|key, _| !key.ends_with("|secret"));
        shape.nodes.extend(before.nodes.iter().filter(|(_, fields)| kind(fields) == "secret").map(|(id, fields)| (id.clone(), fields.clone())));
        shape.edges.extend(before.edges.iter().filter(|(key, _)| key.ends_with("|secret")).map(|(key, fields)| (key.clone(), fields.clone())));
    }
    let rows = diff(&before, &shape);
    if !rows.is_empty() {
        let start = since == 0;
        let transaction = db.unchecked_transaction()?;
        if start {
            transaction.execute(
                "INSERT INTO map_history_meta (one, since) VALUES (1, ?1) ON CONFLICT(one) DO UPDATE SET since = ?1",
                params![now],
            )?;
        }
        let mut insert = transaction
            .prepare_cached("INSERT INTO map_history (at, kind, node, edge, data, why, inferred) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)")?;
        for row in &rows {
            let why = if start { "start" } else { why(row, &before, &shape) };
            insert.execute(params![now, row.kind, row.node, row.edge, Value::Object(row.data.clone()).to_string(), why])?;
        }
        drop(insert);
        transaction.commit()?;
    }
    *last = Some(Last { map: map.clone(), pipes: pipes.to_vec(), locked, shape });
    Ok(())
}

/// The Map recorded so far: every row that isn't inferred, in order.
fn replay(db: &Connection) -> rusqlite::Result<Shape> {
    let mut shape = Shape::default();
    let rows = crate::store::query(
        db,
        "SELECT kind, node, edge, data FROM map_history WHERE inferred = 0 ORDER BY seq",
        [],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?, row.get::<_, Option<String>>(2)?, row.get::<_, String>(3)?)),
    )?;
    for (kind, node, edge, data) in rows {
        let data = match serde_json::from_str(&data) {
            Ok(Value::Object(data)) => data,
            _ => Fields::new(),
        };
        let (place, id) = match (kind.as_str(), node, edge) {
            ("node_add" | "node_change" | "node_remove", Some(node), _) => (&mut shape.nodes, node),
            ("edge_add" | "edge_change" | "edge_remove", _, Some(edge)) => (&mut shape.edges, edge),
            _ => continue,
        };
        match kind.as_str() {
            "node_add" | "edge_add" => {
                place.insert(id, data);
            }
            "node_change" | "edge_change" => place.entry(id).or_default().extend(data),
            _ => {
                place.remove(&id);
            }
        }
    }
    Ok(shape)
}

/// The Map's nodes and edges as recorded, with the pipes' added.
fn shape(map: &v1::ClaktoryMap, pipes: &[PipeSeen], label: &dyn Fn(&str) -> String) -> Shape {
    let mut shape = Shape::default();
    for node in &map.nodes {
        let badges: Vec<Value> =
            node.badges.iter().map(|badge| json!({"key": badge.key, "label": badge.label, "icon": badge.icon})).collect();
        let fields = json!({
            "kind": name(node.kind().as_str_name(), "MAP_NODE_KIND_"),
            "label": node.label,
            "detail": node.detail,
            "parent_id": node.parent_id,
            "project_id": node.project_id,
            "ref_id": node.ref_id,
            "color": node.color,
            "admin": node.admin,
            "badges": badges,
            "icon": node.icon,
        });
        shape.nodes.insert(node.id.clone(), object(fields));
    }
    for edge in &map.edges {
        let key = format!("{}|{}|{}", edge.from_id, edge.to_id, name(edge.kind().as_str_name(), "MAP_EDGE_KIND_"));
        shape.edges.insert(key, object(json!({"both_ways": edge.both_ways, "label": edge.label})));
    }
    for pipe in pipes {
        let project = format!("project:{}", pipe.project);
        // A pipe of a project the Map leaves out (archived) is left out too.
        if !shape.nodes.contains_key(&project) {
            continue;
        }
        let id = format!("pipe:{}", pipe.id);
        let fields = json!({
            "kind": "pipe",
            "label": label(&pipe.id),
            "detail": PIPE_DETAIL,
            "parent_id": project,
            "project_id": pipe.project,
            "ref_id": pipe.id,
            "color": "",
            "admin": false,
            "badges": [],
            "icon": "",
        });
        shape.nodes.insert(id.clone(), object(fields));
        for member in &pipe.members {
            let agent = format!("agent:{member}");
            if shape.nodes.contains_key(&agent) {
                let label = if *member == pipe.lead { "Leads" } else { "" };
                shape.edges.insert(format!("{agent}|{id}|pipe"), object(json!({"both_ways": false, "label": label})));
            }
        }
    }
    shape
}

fn object(value: Value) -> Fields {
    match value {
        Value::Object(fields) => fields,
        _ => Fields::new(),
    }
}

/// An enum value's name without its prefix, lowercased ("agent").
fn name(full: &str, prefix: &str) -> String {
    full.strip_prefix(prefix).unwrap_or(full).to_ascii_lowercase()
}

fn kind(fields: &Fields) -> &str {
    fields.get("kind").and_then(Value::as_str).unwrap_or_default()
}

/// What changed from `before` to `after`: nodes added and changed, edges
/// added, changed and removed, then nodes removed, each in id order.
fn diff(before: &Shape, after: &Shape) -> Vec<Row> {
    let mut rows = Vec::new();
    let changes = |before: &BTreeMap<String, Fields>, after: &BTreeMap<String, Fields>, edges: bool, rows: &mut Vec<Row>| {
        let at = |id: &str| if edges { (None, Some(id.to_owned())) } else { (Some(id.to_owned()), None) };
        for (id, fields) in after {
            let (node, edge) = at(id);
            match before.get(id) {
                None => rows.push(Row { kind: if edges { "edge_add" } else { "node_add" }, node, edge, data: fields.clone() }),
                Some(was) if was != fields => {
                    let changed: Fields =
                        fields.iter().filter(|(key, value)| was.get(*key) != Some(*value)).map(|(key, value)| (key.clone(), value.clone())).collect();
                    rows.push(Row { kind: if edges { "edge_change" } else { "node_change" }, node, edge, data: changed });
                }
                Some(_) => {}
            }
        }
    };
    let removed = |before: &BTreeMap<String, Fields>, after: &BTreeMap<String, Fields>, edges: bool, rows: &mut Vec<Row>| {
        for id in before.keys().filter(|id| !after.contains_key(*id)) {
            let (node, edge) = if edges { (None, Some(id.clone())) } else { (Some(id.clone()), None) };
            rows.push(Row { kind: if edges { "edge_remove" } else { "node_remove" }, node, edge, data: Fields::new() });
        }
    };
    changes(&before.nodes, &after.nodes, false, &mut rows);
    changes(&before.edges, &after.edges, true, &mut rows);
    removed(&before.edges, &after.edges, true, &mut rows);
    removed(&before.nodes, &after.nodes, false, &mut rows);
    rows
}

/// What a row's change came from: the vault for secrets, apps for apps
/// (and what they read or hear), pipes for pipes, the settings otherwise.
fn why(row: &Row, before: &Shape, after: &Shape) -> &'static str {
    let by_kind = |kind: &str| match kind {
        "secret" => "vault",
        "app" | "hears" => "apps",
        "pipe" => "pipes",
        _ => "config",
    };
    if let Some(edge) = &row.edge {
        // An app reading something is the app's.
        let from_app = edge.starts_with("app:");
        let kind = edge.rsplit('|').next().unwrap_or_default();
        return if from_app { "apps" } else { by_kind(kind) };
    }
    let node = row.node.as_deref().unwrap_or_default();
    let fields = after.nodes.get(node).or_else(|| before.nodes.get(node));
    by_kind(fields.map(kind).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Connection {
        let db = Connection::open_in_memory().unwrap();
        db.execute_batch(SCHEMA).unwrap();
        db
    }

    fn node(id: &str, kind: v1::MapNodeKind, label: &str, parent: &str) -> v1::MapNode {
        v1::MapNode { id: id.into(), kind: kind.into(), label: label.into(), parent_id: parent.into(), ..Default::default() }
    }

    fn edge(from: &str, to: &str, kind: v1::MapEdgeKind) -> v1::MapEdge {
        v1::MapEdge { from_id: from.into(), to_id: to.into(), kind: kind.into(), ..Default::default() }
    }

    /// A project with Mike (who leads) and Sam, messaging each other.
    fn team() -> v1::ClaktoryMap {
        use v1::{MapEdgeKind as E, MapNodeKind as N};
        v1::ClaktoryMap {
            nodes: vec![
                node("workspace", N::Workspace, "Workspace", ""),
                node("project:site", N::Project, "Site", ""),
                node("agent:mike", N::Agent, "Mike", "project:site"),
                node("agent:sam", N::Agent, "Sam", "agent:mike"),
            ],
            edges: vec![v1::MapEdge { both_ways: true, ..edge("agent:mike", "agent:sam", E::Messages) }, edge("agent:sam", "agent:mike", E::Lead)],
        }
    }

    fn rows(db: &Connection) -> Vec<(i64, String, String, String, String)> {
        crate::store::query(
            db,
            "SELECT at, kind, coalesce(node, edge), data, why FROM map_history ORDER BY seq",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )
        .unwrap()
    }

    fn named(_: &str) -> String {
        "Launch".to_owned()
    }

    #[test]
    fn every_change_to_the_map_is_recorded_once_in_order() {
        use v1::{MapEdgeKind as E, MapNodeKind as N};
        let db = db();
        let mut last = None;
        let mut map = team();
        // The first recording is the Map as it was: start, at since.
        record(&db, &mut last, &map, &[], false, &named, 100).unwrap();
        let start = rows(&db);
        assert_eq!(start.len(), 6);
        assert!(start.iter().all(|row| row.0 == 100 && row.4 == "start"), "{start:?}");
        assert_eq!(db.query_row("SELECT since, backfilled FROM map_history_meta", [], |row| Ok((row.get(0)?, row.get(1)?))), Ok((100_i64, 0_i64)));
        let sam = start.iter().find(|row| row.2 == "agent:sam").unwrap();
        let sam: Value = serde_json::from_str(&sam.3).unwrap();
        assert_eq!(sam["kind"], "agent");
        assert_eq!(sam["label"], "Sam");
        assert_eq!(sam["parent_id"], "agent:mike");
        // Nothing changed: nothing recorded.
        record(&db, &mut last, &map, &[], false, &named, 150).unwrap();
        assert_eq!(rows(&db).len(), 6);

        // A rename, a move (out from under its lead), a lead dropped.
        map.nodes[3].label = "Samantha".into();
        map.nodes[3].parent_id = "project:site".into();
        map.edges.retain(|edge| edge.kind() != E::Lead);
        record(&db, &mut last, &map, &[], false, &named, 200).unwrap();
        let now: Vec<_> = rows(&db).into_iter().filter(|row| row.0 == 200).map(|row| (row.1, row.2, row.3, row.4)).collect();
        assert_eq!(
            now,
            [
                ("node_change".into(), "agent:sam".into(), r#"{"label":"Samantha","parent_id":"project:site"}"#.into(), "config".into()),
                ("edge_remove".into(), "agent:sam|agent:mike|lead".into(), "{}".into(), "config".into()),
            ]
        );

        // A secret given to Sam, a pipe Mike leads, Zoe joining through an
        // `also` (one way); then the secret taken back.
        map.nodes.push(node("secret:k1", N::Secret, "Stripe", ""));
        map.nodes.push(node("agent:zoe", N::Agent, "Zoe", "workspace"));
        map.edges.push(edge("secret:k1", "agent:sam", E::Secret));
        map.edges.push(edge("agent:zoe", "agent:mike", E::Messages));
        let pipe = PipeSeen {
            id: "p1".into(),
            name: "Their name".into(),
            project: "site".into(),
            state: v1::PipeState::Open.into(),
            members: vec!["mike".into(), "sam".into()],
            lead: "mike".into(),
        };
        record(&db, &mut last, &map, std::slice::from_ref(&pipe), false, &named, 300).unwrap();
        let now: Vec<_> = rows(&db).into_iter().filter(|row| row.0 == 300).map(|row| (row.1, row.2, row.4)).collect();
        let expect = [
            ("node_add", "agent:zoe", "config"),
            ("node_add", "pipe:p1", "pipes"),
            ("node_add", "secret:k1", "vault"),
            ("edge_add", "agent:mike|pipe:p1|pipe", "pipes"),
            ("edge_add", "agent:sam|pipe:p1|pipe", "pipes"),
            ("edge_add", "agent:zoe|agent:mike|messages", "config"),
            ("edge_add", "secret:k1|agent:sam|secret", "vault"),
        ];
        assert_eq!(now, expect.map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned())));
        let pipe_row = rows(&db).into_iter().find(|row| row.2 == "pipe:p1").unwrap();
        let pipe_fields: Value = serde_json::from_str(&pipe_row.3).unwrap();
        assert_eq!((pipe_fields["label"].as_str(), pipe_fields["detail"].as_str()), (Some("Launch"), Some(PIPE_DETAIL)));
        assert_eq!(pipe_fields["parent_id"], "project:site");
        assert!(!pipe_row.3.contains("Their name"), "never the other side's name");
        let lead: Value =
            serde_json::from_str(&rows(&db).into_iter().find(|row| row.2 == "agent:mike|pipe:p1|pipe").unwrap().3).unwrap();
        assert_eq!(lead["label"], "Leads");

        // While the vault is locked, its secrets stay as they were.
        let mut locked = map.clone();
        locked.nodes.retain(|node| node.kind() != N::Secret);
        locked.edges.retain(|edge| edge.kind() != E::Secret);
        record(&db, &mut last, &locked, std::slice::from_ref(&pipe), true, &named, 350).unwrap();
        assert!(rows(&db).iter().all(|row| row.0 != 350), "nothing recorded while locked");

        // The secret taken back; Zoe deleted; the pipe closed.
        map.edges.retain(|edge| edge.kind() != E::Secret);
        map.nodes.retain(|node| node.id != "agent:zoe");
        map.edges.retain(|edge| edge.from_id != "agent:zoe");
        record(&db, &mut last, &map, &[], false, &named, 400).unwrap();
        let now: Vec<_> = rows(&db).into_iter().filter(|row| row.0 == 400).map(|row| (row.1, row.2, row.4)).collect();
        let expect = [
            ("edge_remove", "agent:mike|pipe:p1|pipe", "pipes"),
            ("edge_remove", "agent:sam|pipe:p1|pipe", "pipes"),
            ("edge_remove", "agent:zoe|agent:mike|messages", "config"),
            ("edge_remove", "secret:k1|agent:sam|secret", "vault"),
            ("node_remove", "agent:zoe", "config"),
            ("node_remove", "pipe:p1", "pipes"),
        ];
        assert_eq!(now, expect.map(|(a, b, c)| (a.to_owned(), b.to_owned(), c.to_owned())));

        // After a restart, the Map recorded is read back: nothing new for
        // the same Map, and an inferred row (the backfill's) never counts.
        db.execute(
            "INSERT INTO map_history (at, kind, node, data, why, inferred) VALUES (50, 'node_add', 'agent:old', '{\"kind\":\"agent\",\"label\":\"Old\"}', 'backfill', 1)",
            [],
        )
        .unwrap();
        let count = rows(&db).len();
        let mut fresh = None;
        record(&db, &mut fresh, &map, &[], false, &named, 500).unwrap();
        assert_eq!(rows(&db).len(), count, "{:?}", rows(&db).split_off(count));
        assert_eq!(fresh.unwrap().shape, replay(&db).unwrap());
    }

    #[test]
    fn a_pair_that_now_messages_both_ways_is_one_edge_from_the_lower_id() {
        use v1::MapEdgeKind as E;
        let db = db();
        let mut last = None;
        let mut map = team();
        map.edges = vec![edge("agent:sam", "agent:mike", E::Messages)];
        record(&db, &mut last, &map, &[], false, &named, 100).unwrap();
        map.edges = vec![v1::MapEdge { both_ways: true, ..edge("agent:mike", "agent:sam", E::Messages) }];
        record(&db, &mut last, &map, &[], false, &named, 200).unwrap();
        let now: Vec<_> = rows(&db).into_iter().filter(|row| row.0 == 200).map(|row| (row.1, row.2, row.3)).collect();
        assert_eq!(
            now,
            [
                ("edge_add".into(), "agent:mike|agent:sam|messages".into(), r#"{"both_ways":true,"label":""}"#.into()),
                ("edge_remove".into(), "agent:sam|agent:mike|messages".into(), "{}".into()),
            ]
        );
        // A label that changes is a change of the same edge.
        map.edges[0].label = "Reads".into();
        record(&db, &mut last, &map, &[], false, &named, 300).unwrap();
        let now: Vec<_> = rows(&db).into_iter().filter(|row| row.0 == 300).map(|row| (row.1, row.3)).collect();
        assert_eq!(now, [("edge_change".into(), r#"{"label":"Reads"}"#.into())]);
    }
}
