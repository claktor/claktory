//! History: how this Claktory came to be, for
//! the Map's History mode. `record` keeps every change to the Map as it
//! happens (`map_history`), from the day it first ran; `backfill`
//! reconstructs what came before, once. This answers the app: the overview
//! (every node and edge that ever was, and when, an hourly histogram,
//! chapters), events page by page (`events`), and new ones as they happen
//! (`Watch`). Only the person's devices ask (rivet_reach: Person), and no
//! answer carries what was said, card titles or anything of another
//! Claktory's.

pub(crate) mod backfill;
pub(crate) mod events;
pub(crate) mod record;

use std::collections::{BTreeMap, HashMap};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use factory_protocol::v1;
use rusqlite::{Connection, OpenFlags, params};
use serde_json::{Map, Value};

use crate::factory::{Factory, RequestError};
use events::{Cursor, Item};

/// Each node's index in HistoryOverview.nodes (1-based; 0 is the person),
/// kept from the first overview that has it on: append-only.
pub(crate) const NODES: &str = "
CREATE TABLE IF NOT EXISTS map_history_nodes (
  node TEXT PRIMARY KEY,
  idx INTEGER NOT NULL UNIQUE
);
";

/// Events in a page unless asked for fewer, and at most.
const PAGE: usize = 20_000;
const MOST: usize = 50_000;
/// A cursor in the overview every so many events.
const CURSOR_EVERY: usize = 10_000;
/// How often a watching connection is told what's new.
const WATCH_EVERY: std::time::Duration = std::time::Duration::from_secs(2);
const HOUR: i64 = 3_600_000;

type Fields = Map<String, Value>;

fn problem(error: impl std::fmt::Display) -> RequestError {
    tracing::error!(%error, "could not read History");
    RequestError::Command("Claktory couldn't read its history. Try again in a moment.".to_owned())
}

/// factory.db to read, apart from the store's connection so a long read
/// holds no one up.
fn reader(factory: &Factory) -> rusqlite::Result<Connection> {
    Connection::open_with_flags(factory.home.db_path(), OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX)
}

fn requests(factory: &Factory) -> impl Fn(i64, usize) -> Vec<(i64, String, i64)> + '_ {
    |after, limit| factory.vault.requests_after(after, limit)
}

/// GetHistoryOverview.
pub(crate) async fn overview(factory: &Arc<Factory>, request: v1::GetHistoryOverview) -> Result<v1::HistoryOverview, RequestError> {
    let factory = factory.clone();
    tokio::task::spawn_blocking(move || build_overview(&factory, crate::transcript::now_ms(), &request.project_id).map_err(problem))
        .await
        .map_err(problem)?
}

/// GetHistoryEvents.
pub(crate) async fn page(factory: &Arc<Factory>, request: v1::GetHistoryEvents) -> Result<v1::HistoryEvents, RequestError> {
    let factory = factory.clone();
    tokio::task::spawn_blocking(move || {
        let limit = match usize::try_from(request.limit).unwrap_or(MOST) {
            0 => PAGE,
            limit => limit.min(MOST),
        };
        let db = reader(&factory).map_err(problem)?;
        let keep = keep(&factory, &db, &request.project_id).map_err(problem)?;
        // Pages as full as they can be: a read stops early where a batch
        // runs out, or (one project's) where few were its events.
        let (mut items, mut cursor, mut more) = (Vec::new(), Cursor::decode(&request.after_cursor), true);
        while more && items.len() < limit {
            let (mut read, next, left) =
                events::read(&db, &requests(&factory), cursor, request.until_ms, limit - items.len(), request.include_runs, &keep).map_err(problem)?;
            items.append(&mut read);
            more = left;
            if next == cursor {
                break;
            }
            cursor = next;
        }
        let next = cursor;
        let mut index = Index::read(&factory).map_err(problem)?;
        let page = events::columns(&items, &mut |node| index.of(&factory, node), more.then_some(next));
        Ok(page)
    })
    .await
    .map_err(problem)?
}

/// Whether an item is one project's (`project`, "" for all of them), by
/// its nodes' projects as History has them now.
type Keep = Box<dyn Fn(&Item) -> bool>;

fn keep(factory: &Factory, db: &Connection, project: &str) -> rusqlite::Result<Keep> {
    if project.is_empty() {
        return Ok(Box::new(|_| true));
    }
    let membership = membership(factory, db)?;
    let project = project.to_owned();
    Ok(Box::new(move |item| membership.keeps(item, &project)))
}

/// Which project each node was in over time, as its labels say, to count
/// and page one project's events (GetHistoryOverview.project_id).
#[derive(Default)]
pub(crate) struct Membership {
    /// Each node's, oldest first: from when, in which project ("": none).
    of: HashMap<String, Vec<(i64, String)>>,
}

impl Membership {
    /// From `nodes`' labels, and pipes History has no node of yet in their
    /// projects.
    fn of(factory: &Factory, nodes: &BTreeMap<String, Life>) -> Membership {
        let mut of: HashMap<String, Vec<(i64, String)>> = nodes
            .iter()
            .map(|(id, life)| (id.clone(), life.labels.iter().map(|label| (label.at_ms, label.project_id.clone())).collect()))
            .collect();
        for pipe in factory.pipes.all() {
            of.entry(format!("pipe:{}", pipe.id)).or_insert_with(|| vec![(0, pipe.project)]);
        }
        Membership { of }
    }

    /// Whether History has `node`'s projects.
    fn knows(&self, node: &str) -> bool {
        self.of.get(node).is_some_and(|labels| !labels.is_empty())
    }

    /// `node`'s project at `at` ("": none); before its first label, that
    /// label's.
    fn project(&self, node: &str, at: i64) -> &str {
        let Some(labels) = self.of.get(node) else { return "" };
        let after = labels.partition_point(|(from, _)| *from <= at);
        labels.get(after.saturating_sub(1)).map_or("", |(_, project)| project.as_str())
    }

    /// The projects `item` is in, each once: those of an event's sender
    /// and recipients at the time, of a run's agent when it began.
    fn projects<'a>(&'a self, item: &Item, into: &mut Vec<&'a str>) {
        into.clear();
        let mut add = |project: &'a str| {
            if !project.is_empty() && !into.contains(&project) {
                into.push(project);
            }
        };
        match item {
            Item::Event(event) => std::iter::once(&event.from).chain(&event.to).for_each(|who| add(self.project(who, event.at))),
            Item::Run(run) => add(self.project(&run.agent, run.started)),
        }
    }

    fn keeps(&self, item: &Item, project: &str) -> bool {
        match item {
            Item::Event(event) => std::iter::once(&event.from).chain(&event.to).any(|who| self.project(who, event.at) == project),
            Item::Run(run) => self.project(&run.agent, run.started) == project,
        }
    }

    /// What it says of the time up to `upto`, to tell whether what was
    /// counted with it still holds: the backfill, or a move recorded late,
    /// starts the count over; a node new since (an agent added) doesn't.
    fn up_to(&self, upto: i64) -> u64 {
        let mut hasher = DefaultHasher::new();
        let mut ids: Vec<&String> = self.of.keys().collect();
        ids.sort();
        for id in ids {
            let mut labels = self.of[id].iter().filter(|(from, _)| *from <= upto).peekable();
            if labels.peek().is_some() {
                id.hash(&mut hasher);
                labels.for_each(|label| label.hash(&mut hasher));
            }
        }
        hasher.finish()
    }
}

/// The nodes' projects over time as History has them now, kept until it
/// changes (`current_version`).
fn membership(factory: &Factory, db: &Connection) -> rusqlite::Result<Arc<Membership>> {
    let version = current_version(factory)?;
    let mut kept = locked(&factory.history.membership);
    if let Some((at, membership)) = &*kept
        && *at == version
    {
        return Ok(membership.clone());
    }
    let (mut nodes, _) = lives(db)?;
    nodes.retain(|id, _| !is_chat(id));
    let membership = Arc::new(Membership::of(factory, &nodes));
    *kept = Some((version, membership.clone()));
    Ok(membership)
}

fn locked<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// What the overview counted of the events, kept so that opening History
/// again, or HistoryChanged, reads only what's new; and the nodes'
/// projects for pages.
#[derive(Default)]
pub(crate) struct Kept {
    counted: Mutex<Option<Counted>>,
    membership: Mutex<Option<(u64, Arc<Membership>)>>,
}

/// Events counted: per hour, how many, and a cursor every CURSOR_EVERY.
#[derive(Clone, Default)]
struct Tally {
    hours: BTreeMap<i64, u32>,
    count: u64,
    cursors: Vec<v1::HistoryCursor>,
}

impl Tally {
    /// One more event, at `at`, from `before` on.
    fn add(&mut self, at: i64, before: Cursor) {
        if self.count.is_multiple_of(CURSOR_EVERY as u64) {
            self.cursors.push(v1::HistoryCursor { at_ms: at, cursor: before.encode() });
        }
        self.count += 1;
        *self.hours.entry(at.div_euclid(HOUR)).or_default() += 1;
    }
}

/// Every event up to `cursor`, counted.
#[derive(Default)]
struct Counted {
    cursor: Cursor,
    /// The whole Claktory's, and each project's.
    all: Tally,
    projects: HashMap<String, Tally>,
    seen: HashMap<String, Seen>,
    /// The first event, and the last.
    first: Option<i64>,
    upto: i64,
    /// The projects as counted (`Membership::up_to`).
    membership: u64,
    /// Each release's first run (`chapters`), from the runs up to `runs`.
    releases: BTreeMap<String, i64>,
    runs: i64,
}

impl Counted {
    /// Releases' first runs, from the runs after the last read.
    fn extend_releases(&mut self, db: &Connection) -> rusqlite::Result<()> {
        let last: i64 = db.query_row("SELECT coalesce(max(id), 0) FROM runs", [], |row| row.get(0))?;
        let firsts: Vec<(String, i64)> = crate::store::query(
            db,
            "SELECT version, min(started) FROM runs WHERE id > ?1 AND id <= ?2 AND version NOT IN ('', 'dev') GROUP BY version",
            [self.runs, last],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        for (version, at) in firsts {
            let first = self.releases.entry(version).or_insert(at);
            *first = (*first).min(at);
        }
        self.runs = last;
        Ok(())
    }

    /// Counts what came after its cursor.
    fn extend(&mut self, db: &Connection, requests: &dyn Fn(i64, usize) -> Vec<(i64, String, i64)>, membership: &Membership) -> rusqlite::Result<()> {
        let mut projects = Vec::new();
        loop {
            let (next, more) = events::walk(db, requests, self.cursor, 0, CURSOR_EVERY, false, &mut |item, before| {
                let Item::Event(event) = item else { return false };
                self.all.add(event.at, before);
                membership.projects(item, &mut projects);
                for project in &projects {
                    self.projects.entry((*project).to_owned()).or_default().add(event.at, before);
                }
                for who in std::iter::once(&event.from).chain(&event.to).filter(|who| !who.is_empty()) {
                    let seen = self.seen.entry(who.clone()).or_insert(Seen { first: event.at, last: event.at, name: String::new() });
                    seen.first = seen.first.min(event.at);
                    seen.last = seen.last.max(event.at);
                }
                self.first = Some(self.first.map_or(event.at, |first| first.min(event.at)));
                self.upto = self.upto.max(event.at);
                true
            })?;
            self.cursor = next;
            if !more {
                return Ok(());
            }
        }
    }
}

/// What the overview takes from the counts: who took part, the
/// project's tally ("" the whole Claktory's), the first event, releases.
struct Found {
    seen: HashMap<String, Seen>,
    tally: Tally,
    first: Option<i64>,
    releases: BTreeMap<String, i64>,
}

/// Every event counted, on from what was counted before.
fn counted(factory: &Factory, db: &Connection, membership: &Membership, project: &str) -> rusqlite::Result<Found> {
    let mut kept = locked(&factory.history.counted);
    let mut counted = match kept.take() {
        Some(counted) if counted.membership == membership.up_to(counted.upto) => counted,
        _ => Counted::default(),
    };
    // On an error, nothing is kept: the next overview counts again.
    counted.extend(db, &requests(factory), membership)?;
    counted.extend_releases(db)?;
    counted.membership = membership.up_to(counted.upto);
    let tally = if project.is_empty() { counted.all.clone() } else { counted.projects.get(project).cloned().unwrap_or_default() };
    let found = Found { seen: counted.seen.clone(), tally, first: counted.first, releases: counted.releases.clone() };
    *kept = Some(counted);
    Ok(found)
}

/// How long after a start the events are counted ahead: after the
/// backfill's minute (`backfill::spawn`) and the start's own work.
const WARM_AFTER: std::time::Duration = std::time::Duration::from_secs(90);

/// A while after a start, in the background: the store's indexes on
/// growing tables (never built before Claktory listens), then every event
/// counted, so the first time History opens after a restart reads only
/// what's new.
pub(crate) fn spawn_warm(factory: &Arc<Factory>) {
    let factory = Arc::downgrade(factory);
    tokio::spawn(async move {
        tokio::time::sleep(WARM_AFTER).await;
        let Some(factory) = factory.upgrade() else { return };
        if let Ok(Err(error)) = tokio::task::spawn_blocking(move || warm(&factory)).await {
            tracing::warn!(%error, "History: couldn't count the events ahead");
        }
    });
}

/// Builds the store's late indexes (`store::LATE_INDEXES`, History's
/// among them), then counts every event not counted yet; how many there
/// are in all.
fn warm(factory: &Factory) -> rusqlite::Result<u64> {
    factory.store.build_late_indexes()?;
    let started = std::time::Instant::now();
    let db = reader(factory)?;
    let (mut nodes, _) = lives(&db)?;
    nodes.retain(|id, _| !is_chat(id));
    let events = counted(factory, &db, &Membership::of(factory, &nodes), "")?.tally.count;
    tracing::info!(events, took = ?started.elapsed(), "History: counted the events ahead");
    Ok(events)
}

/// Tests: counts the events ahead now, as 90 seconds after a start; how
/// many there are.
#[doc(hidden)]
pub fn warm_for_test(factory: &Factory) -> u64 {
    warm(factory).expect("counted")
}

/// Tests: how many events the counts kept hold, if any are kept.
#[doc(hidden)]
pub fn counted_for_test(factory: &Factory) -> Option<u64> {
    locked(&factory.history.counted).as_ref().map(|counted| counted.all.count)
}

/// Where each node sits in the overview's list, as kept.
struct Index {
    of: HashMap<String, u32>,
    next: u32,
}

impl Index {
    fn read(factory: &Factory) -> rusqlite::Result<Index> {
        let db = factory.store.db();
        let rows: Vec<(String, u32)> = crate::store::query(&db, "SELECT node, idx FROM map_history_nodes", [], |row| Ok((row.get(0)?, row.get(1)?)))?;
        let next = rows.iter().map(|(_, at)| *at).max().unwrap_or_default() + 1;
        Ok(Index { of: rows.into_iter().collect(), next })
    }

    /// `node`'s index, given it at the end if it has none yet; 0 for the
    /// person.
    fn of(&mut self, factory: &Factory, node: &str) -> u32 {
        if node.is_empty() {
            return 0;
        }
        match self.of.get(node) {
            Some(at) => *at,
            None => self.place(factory, &[node]),
        }
    }

    /// Places `nodes` at the end, in this order, in one transaction; the
    /// last one's index.
    fn place(&mut self, factory: &Factory, nodes: &[&str]) -> u32 {
        let db = factory.store.db();
        let placed = (|| -> rusqlite::Result<()> {
            let transaction = db.unchecked_transaction()?;
            for (offset, node) in nodes.iter().enumerate() {
                let at = self.next + u32::try_from(offset).unwrap_or(u32::MAX);
                transaction.execute("INSERT INTO map_history_nodes (node, idx) VALUES (?1, ?2)", params![node, at])?;
            }
            transaction.commit()
        })();
        if let Err(error) = placed {
            tracing::error!(%error, "could not keep nodes' places in History");
        }
        for node in nodes {
            self.of.insert((*node).to_owned(), self.next);
            self.next += 1;
        }
        self.next - 1
    }
}

/// What History knows of one node or edge over time.
#[derive(Default)]
struct Life {
    fields: Fields,
    present: Vec<v1::HistoryInterval>,
    labels: Vec<v1::HistoryLabel>,
    /// The first time it was there.
    first: i64,
}

impl Life {

    fn add(&mut self, at: i64, inferred: bool) {
        if self.present.is_empty() {
            self.first = at;
        }
        if let Some(last) = self.present.last_mut()
            && last.to_ms == 0
        {
            // Still there: what was reconstructed carries on as recorded.
            if last.inferred == inferred {
                return;
            }
            last.to_ms = at.max(last.from_ms + 1);
        }
        self.present.push(v1::HistoryInterval { from_ms: at, to_ms: 0, inferred });
    }

    fn remove(&mut self, at: i64) {
        if let Some(last) = self.present.last_mut()
            && last.to_ms == 0
        {
            last.to_ms = at.max(last.from_ms + 1);
        }
    }

    /// A rename or a move from `at`, when it is one.
    fn label(&mut self, at: i64) {
        let text = |key: &str| self.fields.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let label =
            v1::HistoryLabel { at_ms: at, label: text("label"), detail: text("detail"), parent_id: text("parent_id"), project_id: text("project_id") };
        let same = self.labels.last().is_some_and(|last| {
            (&last.label, &last.detail, &last.parent_id, &last.project_id) == (&label.label, &label.detail, &label.parent_id, &label.project_id)
        });
        if !same {
            self.labels.push(label);
        }
    }
}

/// A row of map_history: when, kind, node, edge, data, inferred.
type Recorded = (i64, String, Option<String>, Option<String>, String, bool);

/// Every node and edge in map_history, as they lived, oldest change first.
fn lives(db: &Connection) -> rusqlite::Result<(BTreeMap<String, Life>, BTreeMap<String, Life>)> {
    let rows: Vec<Recorded> = crate::store::query(
        db,
        "SELECT at, kind, node, edge, data, inferred FROM map_history ORDER BY at, seq",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get::<_, i64>(5)? != 0)),
    )?;
    let (mut nodes, mut edges) = (BTreeMap::<String, Life>::new(), BTreeMap::<String, Life>::new());
    for (at, kind, node, edge, data, inferred) in rows {
        let data = match serde_json::from_str(&data) {
            Ok(Value::Object(data)) => data,
            _ => Fields::new(),
        };
        let (life, is_node) = match (kind.split('_').next(), node, edge) {
            (Some("node"), Some(node), _) => (nodes.entry(node).or_default(), true),
            (Some("edge"), _, Some(edge)) => (edges.entry(edge).or_default(), false),
            _ => continue,
        };
        match kind.rsplit('_').next() {
            Some("add") => {
                life.fields.extend(data);
                life.add(at, inferred);
            }
            Some("change") => life.fields.extend(data),
            Some("remove") => life.remove(at),
            _ => continue,
        }
        if is_node {
            life.label(at);
        }
    }
    Ok((nodes, edges))
}

fn text<'a>(fields: &'a Fields, key: &str) -> &'a str {
    fields.get(key).and_then(Value::as_str).unwrap_or_default()
}

fn map_node(id: &str, fields: &Fields) -> v1::MapNode {
    let kind = v1::MapNodeKind::from_str_name(&format!("MAP_NODE_KIND_{}", text(fields, "kind").to_ascii_uppercase()))
        .unwrap_or(v1::MapNodeKind::Unspecified);
    let badges = fields
        .get("badges")
        .and_then(Value::as_array)
        .map(|badges| {
            badges
                .iter()
                .filter_map(Value::as_object)
                .map(|badge| v1::MapBadge { key: text(badge, "key").to_owned(), label: text(badge, "label").to_owned(), icon: text(badge, "icon").to_owned() })
                .collect()
        })
        .unwrap_or_default();
    v1::MapNode {
        id: id.to_owned(),
        kind: kind.into(),
        label: text(fields, "label").to_owned(),
        detail: text(fields, "detail").to_owned(),
        parent_id: text(fields, "parent_id").to_owned(),
        admin: fields.get("admin").and_then(Value::as_bool).unwrap_or_default(),
        color: text(fields, "color").to_owned(),
        badges,
        ref_id: text(fields, "ref_id").to_owned(),
        project_id: text(fields, "project_id").to_owned(),
        icon: text(fields, "icon").to_owned(),
    }
}

fn map_edge(key: &str, fields: &Fields) -> Option<v1::MapEdge> {
    let mut parts = key.rsplitn(3, '|');
    let (kind, to, from) = (parts.next()?, parts.next()?, parts.next()?);
    let kind = v1::MapEdgeKind::from_str_name(&format!("MAP_EDGE_KIND_{}", kind.to_ascii_uppercase()))?;
    Some(v1::MapEdge {
        from_id: from.to_owned(),
        to_id: to.to_owned(),
        kind: kind.into(),
        both_ways: fields.get("both_ways").and_then(Value::as_bool).unwrap_or_default(),
        label: text(fields, "label").to_owned(),
    })
}

/// What the events say of who took part: when each was first and last
/// seen, and an agent's last name signed.
#[derive(Clone, Default)]
struct Seen {
    first: i64,
    last: i64,
    name: String,
}

fn build_overview(factory: &Factory, now: i64, project: &str) -> rusqlite::Result<v1::HistoryOverview> {
    let db = reader(factory)?;
    let (mut nodes, mut edges) = lives(&db)?;
    // Never a chat: what was placed or inferred of one before is
    // left out here too.
    nodes.retain(|id, _| !is_chat(id));
    edges.retain(|key, _| !key.split('|').any(is_chat));
    let since: i64 = db.query_row("SELECT since FROM map_history_meta", [], |row| row.get(0)).unwrap_or(0);

    // Every event, counted once (what was counted before is kept): the
    // histogram, the count, cursors, and who took part.
    let membership = Membership::of(factory, &nodes);
    let Found { mut seen, tally, first, releases } = counted(factory, &db, &membership, project)?;
    // Agents' names as they signed their messages, for those History has
    // no node of (gone before it was recorded).
    let unknown: Vec<String> = seen.keys().filter(|id| !nodes.contains_key(*id) && id.starts_with("agent:")).cloned().collect();
    for id in unknown {
        let agent = id.trim_start_matches("agent:");
        let name: Option<String> = db
            .query_row("SELECT sender FROM messages WHERE sender_id = ?1 AND source = 'agent' ORDER BY id DESC LIMIT 1", [agent], |row| row.get(0))
            .ok();
        if let (Some(seen), Some(name)) = (seen.get_mut(&id), name) {
            seen.name = name;
        }
    }
    // Those it has no node of: reconstructed over when they were seen.
    for (id, seen) in &seen {
        if nodes.contains_key(id) {
            continue;
        }
        let mut life = Life::default();
        let (kind, rest) = id.split_once(':').unwrap_or(("agent", id));
        let mut fields = Fields::new();
        fields.insert("kind".into(), Value::from(kind));
        fields.insert("ref_id".into(), Value::from(rest));
        match kind {
            "pipe" => {
                let record = factory.pipes.get(rest);
                let label = record.as_ref().map_or_else(|| "Pipe".to_owned(), record::pipe_label);
                fields.insert("label".into(), Value::from(label));
                fields.insert("detail".into(), Value::from(record::PIPE_DETAIL));
                if let Some(record) = record {
                    fields.insert("parent_id".into(), Value::from(format!("project:{}", record.project)));
                    fields.insert("project_id".into(), Value::from(record.project));
                }
            }
            _ => {
                let name = if seen.name.is_empty() { rest.to_owned() } else { seen.name.clone() };
                fields.insert("label".into(), Value::from(name));
            }
        }
        life.fields = fields;
        life.add(seen.first, true);
        life.remove(seen.last);
        life.label(seen.first);
        nodes.insert(id.clone(), life);
    }

    // Their places: new ones at the end, by when they first were.
    let mut index = Index::read(factory)?;
    let mut fresh: Vec<(&String, i64)> = nodes.iter().filter(|(id, _)| !index.of.contains_key(*id)).map(|(id, life)| (id, life.first)).collect();
    fresh.sort_by_key(|(id, first)| (*first, (*id).clone()));
    if !fresh.is_empty() {
        let fresh: Vec<&str> = fresh.iter().map(|(id, _)| id.as_str()).collect();
        index.place(factory, &fresh);
    }
    let mut placed: Vec<(u32, v1::HistoryNode)> = index
        .of
        .iter()
        .map(|(id, at)| {
            // A chat placed before keeps its place, empty, so no other
            // node moves.
            if is_chat(id) {
                return (*at, v1::HistoryNode::default());
            }
            let node = nodes.get(id).map_or_else(
                || v1::HistoryNode { node: Some(v1::MapNode { id: id.clone(), ..Default::default() }), ..Default::default() },
                |life| v1::HistoryNode { node: Some(map_node(id, &life.fields)), present: life.present.clone(), labels: life.labels.clone() },
            );
            (*at, node)
        })
        .collect();
    placed.sort_by_key(|(at, _)| *at);
    // An index never given leaves no gap in the list.
    let mut list = Vec::with_capacity(placed.len());
    for (at, node) in placed {
        while list.len() + 1 < at as usize {
            list.push(v1::HistoryNode::default());
        }
        list.push(node);
    }

    let start = first.into_iter().chain(nodes.values().map(|life| life.first).filter(|first| *first > 0)).min().unwrap_or(now);
    let first_hour = start.div_euclid(HOUR);
    let hours = usize::try_from(now.div_euclid(HOUR) - first_hour + 1).unwrap_or(1);
    let mut histogram = vec![0u32; hours];
    for (hour, events) in &tally.hours {
        if let Some(count) = usize::try_from(hour - first_hour).ok().and_then(|hour| histogram.get_mut(hour)) {
            *count += events;
        }
    }
    // One project's: only its nodes, at the time.
    let project_node = format!("project:{project}");
    let ours = |id: &str, at: i64| project.is_empty() || id == project_node || membership.project(id, at) == project;
    let tokens = tokens(factory, &db, &membership, project, first_hour, hours, now)?;
    let chapters = chapters(&db, &nodes, &ours, &releases, since, first_hour, &histogram)?;
    let max_seq: i64 = db.query_row("SELECT coalesce(max(seq), 0) FROM map_history", [], |row| row.get(0))?;
    Ok(v1::HistoryOverview {
        start_ms: start,
        end_ms: now,
        recording_since_ms: since,
        nodes: list,
        edges: edges
            .iter()
            .filter_map(|(key, life)| Some(v1::HistoryEdge { edge: Some(map_edge(key, &life.fields)?), present: life.present.clone() }))
            .collect(),
        histogram,
        chapters,
        message_count: tally.count,
        history_version: version(max_seq, index.next),
        cursors: tally.cursors,
        tokens,
    })
}

/// The tokens agents' model calls used in each of `hours` hours from
/// `first_hour` (hours since 1970, UTC), as the Usage pane totals them
/// (input, output, and the cache read and written), from its records
/// (`agent_usage`; none before they began). One project's (`project`):
/// its agents' in each hour, by the project History has each in at the
/// hour's end (or now, for the hour in progress); an agent History has no
/// node of, by its project now. Chats, and calls from before agents were
/// counted, are no project's.
fn tokens(
    factory: &Factory,
    db: &Connection,
    membership: &Membership,
    project: &str,
    first_hour: i64,
    hours: usize,
    now: i64,
) -> rusqlite::Result<Vec<u64>> {
    let mut tokens = vec![0u64; hours];
    let rows = crate::store::query(
        db,
        "SELECT hour, agent, SUM(input) + SUM(output) + SUM(cache_read) + SUM(cache_write) FROM agent_usage
         WHERE hour >= ?1 GROUP BY hour, agent",
        [first_hour * (HOUR / 1000)],
        |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, i64>(2)?)),
    )?;
    let projects_now: HashMap<String, String> = if project.is_empty() {
        HashMap::new()
    } else {
        factory.lock().config.agents.iter().map(|agent| (agent.id.clone(), agent.project.clone().unwrap_or_default())).collect()
    };
    for (hour, agent, used) in rows {
        let start = hour * 1000;
        if !project.is_empty() {
            if agent.is_empty() || crate::chats::is_chat(&agent) {
                continue;
            }
            let node = format!("agent:{agent}");
            let theirs = if membership.knows(&node) {
                membership.project(&node, (start + HOUR - 1).min(now))
            } else {
                projects_now.get(&agent).map_or("", String::as_str)
            };
            if theirs != project {
                continue;
            }
        }
        if let Some(sum) = usize::try_from(start.div_euclid(HOUR) - first_hour).ok().and_then(|at| tokens.get_mut(at)) {
            *sum += u64::try_from(used).unwrap_or(0);
        }
    }
    Ok(tokens)
}

/// A chat's node id ("agent:chat:1"), never one of History's.
fn is_chat(id: &str) -> bool {
    id.strip_prefix("agent:").is_some_and(crate::chats::is_chat)
}

/// What changes when the overview does: what's recorded, and the nodes
/// placed.
fn version(max_seq: i64, next_index: u32) -> u64 {
    u64::try_from(max_seq).unwrap_or_default() + u64::from(next_index)
}

fn current_version(factory: &Factory) -> rusqlite::Result<u64> {
    let max_seq: i64 = factory.store.db().query_row("SELECT coalesce(max(seq), 0) FROM map_history", [], |row| row.get(0))?;
    let next: u32 = factory.store.db().query_row("SELECT coalesce(max(idx), 0) + 1 FROM map_history_nodes", [], |row| row.get(0))?;
    Ok(version(max_seq, next))
}

/// "A", "A and B", "A, B and C", "A, B and 4 others".
fn names(names: &[String]) -> String {
    match names {
        [] => String::new(),
        [one] => one.clone(),
        [first, second, _, _, ..] => format!("{first}, {second} and {} others", names.len() - 2),
        _ => factory_agent::text::list_and(names),
    }
}

/// The big moments, oldest first. Those that name nodes name only the ones
/// `ours` keeps (a node, at a time), and are left out with none.
fn chapters(
    db: &Connection,
    nodes: &BTreeMap<String, Life>,
    ours: &dyn Fn(&str, i64) -> bool,
    releases: &BTreeMap<String, i64>,
    since: i64,
    first_hour: i64,
    histogram: &[u32],
) -> rusqlite::Result<Vec<v1::HistoryChapter>> {
    use v1::HistoryChapterKind as Kind;
    let chapter = |at: i64, kind: Kind, title: String, node_ids: Vec<String>| v1::HistoryChapter { at_ms: at, kind: kind.into(), title, node_ids };
    let mut chapters = Vec::new();
    if since > 0 {
        chapters.push(chapter(since, Kind::Recording, "Recording began".to_owned(), Vec::new()));
    }
    let label = |life: &Life| text(&life.fields, "label").to_owned();
    let mut joined: BTreeMap<i64, Vec<(&String, &Life)>> = BTreeMap::new();
    let mut first_secret: Option<(&String, &Life)> = None;
    for (id, life) in nodes.iter().filter(|(id, life)| life.first > 0 && ours(id, life.first)) {
        match text(&life.fields, "kind") {
            "project" => chapters.push(chapter(life.first, Kind::Project, format!("{} began", label(life)), vec![id.clone()])),
            "agent" => joined.entry(life.first.div_euclid(24 * HOUR)).or_default().push((id, life)),
            "pipe" => {
                let title = match label(life).as_str() {
                    "Pipe" => "A pipe to another Claktory opened".to_owned(),
                    name => format!("The pipe {name} opened"),
                };
                chapters.push(chapter(life.first, Kind::Pipe, title, vec![id.clone()]));
            }
            "app" => chapters.push(chapter(life.first, Kind::App, format!("{} was installed", label(life)), vec![id.clone()])),
            "secret" if first_secret.is_none_or(|(_, first)| life.first < first.first) => first_secret = Some((id, life)),
            _ => {}
        }
    }
    for agents in joined.into_values() {
        let mut agents = agents;
        agents.sort_by_key(|(_, life)| life.first);
        let at = agents[0].1.first;
        let who: Vec<String> = agents.iter().map(|(_, life)| label(life)).collect();
        chapters.push(chapter(at, Kind::Agents, format!("{} joined", names(&who)), agents.iter().map(|(id, _)| (*id).clone()).collect()));
    }
    if let Some((id, life)) = first_secret {
        chapters.push(chapter(life.first, Kind::Secret, format!("The first secret: {}", label(life)), vec![id.clone()]));
    }
    // A version of Claktory, when its first run began; never a build
    // that isn't a release ("dev").
    chapters.extend(releases.iter().map(|(version, at)| chapter(*at, Kind::Release, format!("Claktory {version}"), Vec::new())));
    // Goals done: whose, never what.
    let goals: Vec<(String, i64)> = crate::store::query(
        db,
        "SELECT agent, time FROM checklist_goal_events WHERE kind = 'status' AND value = 'done' ORDER BY time",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    for (agent, at) in goals.into_iter().filter(|(agent, _)| !crate::chats::is_chat(agent)) {
        let id = format!("agent:{agent}");
        if !ours(&id, at) {
            continue;
        }
        let name = nodes.get(&id).map_or_else(|| agent.clone(), label);
        chapters.push(chapter(at, Kind::Goal, format!("{name} finished a goal"), vec![id]));
    }
    if let Some((hour, count)) = histogram.iter().enumerate().max_by_key(|(hour, count)| (**count, std::cmp::Reverse(*hour)))
        && *count > 0
    {
        let at = (first_hour + hour as i64) * HOUR;
        chapters.push(chapter(at, Kind::Busiest, format!("The busiest hour: {count} messages"), Vec::new()));
    }
    chapters.sort_by_key(|chapter| chapter.at_ms);
    Ok(chapters)
}

/// A connection's WatchHistory: from where it began watching, what's new.
pub(crate) struct Watch {
    factory: Arc<Factory>,
    watching: Mutex<Option<Watching>>,
}

/// Where a watch is: the cursor after what it was sent, the overview's
/// version then, and the project it watches ("": all).
#[derive(Clone, Debug, PartialEq)]
struct Watching {
    cursor: Cursor,
    version: u64,
    project: String,
}

impl Watch {
    pub(crate) fn new(factory: &Arc<Factory>) -> Watch {
        Watch { factory: factory.clone(), watching: Mutex::new(None) }
    }

    fn watching(&self) -> MutexGuard<'_, Option<Watching>> {
        locked(&self.watching)
    }

    /// WatchHistory: on from now (one project's, or all), or off.
    pub(crate) async fn set(&self, request: v1::WatchHistory) -> Result<(), RequestError> {
        if !request.on {
            *self.watching() = None;
            return Ok(());
        }
        let factory = self.factory.clone();
        let from = tokio::task::spawn_blocking(move || -> rusqlite::Result<(Cursor, u64)> {
            let db = reader(&factory)?;
            Ok((events::end(&db, &requests(&factory))?, current_version(&factory)?))
        })
        .await
        .map_err(problem)?
        .map_err(problem)?;
        *self.watching() = Some(Watching { cursor: from.0, version: from.1, project: request.project_id });
        Ok(())
    }

    /// What's new, once there is some: events (never runs), or that the
    /// overview changed. Never, while not watching.
    pub(crate) async fn next(&self) -> v1::server_message::Body {
        loop {
            let Some(watching) = self.watching().clone() else { return std::future::pending().await };
            tokio::time::sleep(WATCH_EVERY).await;
            let factory = self.factory.clone();
            let (cursor, project) = (watching.cursor, watching.project.clone());
            let news = tokio::task::spawn_blocking(move || -> rusqlite::Result<(Vec<Item>, Cursor, u64)> {
                let db = reader(&factory)?;
                let keep = keep(&factory, &db, &project)?;
                let (items, next, _) = events::read(&db, &requests(&factory), cursor, 0, MOST, false, &keep)?;
                Ok((items, next, current_version(&factory)?))
            })
            .await;
            let Ok(Ok((items, next, now))) = news else { continue };
            let mut current = self.watching();
            // Turned off or on again meanwhile: start over from there.
            if current.as_ref() != Some(&watching) {
                continue;
            }
            if now != watching.version {
                *current = Some(Watching { version: now, ..watching });
                return v1::server_message::Body::HistoryChanged(v1::HistoryChanged { history_version: now });
            }
            // On past what another project had, sent or not.
            *current = Some(Watching { cursor: next, ..watching });
            drop(current);
            if items.is_empty() {
                continue;
            }
            let factory = &self.factory;
            let page = match Index::read(factory) {
                Ok(mut index) => events::columns(&items, &mut |node| index.of(factory, node), None),
                Err(error) => {
                    tracing::error!(%error, "could not read History's places");
                    continue;
                }
            };
            return v1::server_message::Body::HistoryEvents(page);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Membership;

    /// What was counted holds while what the projects were up to then
    /// stays the same: a node added since, or a move since, leaves it.
    #[test]
    fn counts_hold_until_the_projects_up_to_them_change() {
        let of = |nodes: &[(&str, &[(i64, &str)])]| Membership {
            of: nodes.iter().map(|(id, labels)| ((*id).to_owned(), labels.iter().map(|(at, project)| (*at, (*project).to_owned())).collect())).collect(),
        };
        let counted = of(&[("agent:mike", &[(10, "site")])]).up_to(100);
        assert_eq!(of(&[("agent:mike", &[(10, "site")]), ("agent:new", &[(150, "site")])]).up_to(100), counted, "an agent added since");
        assert_eq!(of(&[("agent:mike", &[(10, "site"), (150, "lab")])]).up_to(100), counted, "a move since");
        assert_ne!(of(&[("agent:mike", &[(10, "site"), (90, "lab")])]).up_to(100), counted, "a move recorded late");
        assert_ne!(of(&[("agent:mike", &[(10, "site")]), ("agent:old", &[(5, "lab")])]).up_to(100), counted, "the backfill's");
    }
}
