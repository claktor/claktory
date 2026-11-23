//! Claktory's Charts app: dashboards of live charts over the data the
//! person lets it read (your agents' activity, their own databases), drawn
//! by Claktory's own chart components (catalog v1's Stat, Chart and Table).
//!
//! A dashboard is a definition kept in the app's key-value store
//! ([`model`]): a range, variables and panels, each a read-only SQL query
//! against a granted source and how to show its answer. Values (the range,
//! the variables) are bound to the query, never spliced into it ([`sql`]).
//! Every save keeps the version before it.
//!
//! While someone has a view open ([`view`]), the app runs its queries every
//! `refresh_s` seconds and pushes only what changed; once the last view
//! closes, nothing runs. In a view the person picks the dashboard, the
//! range and the variables' values, and changes panels in an editor that
//! runs the query as a preview first. Agents given its permissions use its
//! tools ([`tools`]): `view` reads the dashboards, `edit` changes them and
//! runs queries.
//!
//! The logic speaks to the host only through [`Host`], so it's tested
//! natively with a stand-in ([`tests`]); `wasm.rs` is the real host, through
//! the `claktory:app@1.1` world `app-with-views`.

pub mod model;
pub mod sql;
pub mod tools;
pub mod view;

#[cfg(test)]
mod tests;
#[cfg(target_arch = "wasm32")]
mod wasm;

use std::collections::BTreeMap;

use serde_json::{Value, json};

use model::{Dashboard, MIN_REFRESH_S};
use view::{Draft, Editor, Form, Scene, View};

/// The catalog its views are made of.
pub const CATALOG: &str = "https://claktory.com/a2ui/catalog/v1";

/// Your agents' activity: when they run, their messages, tasks, monitors,
/// checklist items and token use, never what was written. What the
/// default dashboard reads.
pub const ACTIVITY: &str = "activity";

/// All of Claktory's records, as Charts before 0.3 read them: only where
/// the person granted them before.
pub const RECORDS: &str = "factory-records";

/// The source a query names, as it reads now: a dashboard made before 0.3
/// names all of Claktory's records, which reads as Activity where Charts
/// no longer has them.
pub fn source_for<'a>(asked: &'a str, sources: &[DataSource]) -> &'a str {
    if asked == RECORDS && !sources.iter().any(|source| source.id == RECORDS) { ACTIVITY } else { asked }
}

/// The timer that refreshes open views.
pub const REFRESH: &str = "refresh";

/// While a view waits for a source, how soon it looks again (reading
/// nothing but whether it's granted), so it shows the charts within moments
/// of the person turning the source on.
pub const ASK_AGAIN_MS: u64 = 3_000;

/// A source of data the manifest asks for, and whether the person granted
/// it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataSource {
    pub id: String,
    pub label: String,
    pub granted: bool,
}

/// Why a query didn't answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataError {
    /// The person hasn't granted the source, or took it back.
    NotGranted,
    /// The manifest doesn't ask for it, or the agent is gone.
    UnknownSource,
    /// Not one read-only statement, or params that can't be bound.
    Invalid(String),
    /// It ran but failed, or ran too long.
    Failed(String),
}

/// How much a log line matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
    Info,
    Debug,
}

/// What the app can ask of Claktory's app host.
pub trait Host {
    fn kv_get(&mut self, key: &str) -> Option<Vec<u8>>;
    fn kv_set(&mut self, key: &str, value: &[u8]) -> Result<(), String>;
    fn kv_delete(&mut self, key: &str);
    fn set_timer(&mut self, name: &str, after_ms: u64);
    fn cancel_timer(&mut self, name: &str);
    /// One A2UI v1.0 message for an open surface.
    fn push_view(&mut self, surface: &str, message: &Value) -> Result<(), String>;
    fn data_sources(&mut self) -> Vec<DataSource>;
    /// A read-only query; answers {"columns", "rows", "more"}.
    fn data_query(&mut self, source: &str, sql: &str, params: &Value) -> Result<Value, DataError>;
    /// Now, in milliseconds since 1970.
    fn now_ms(&mut self) -> i64;
    fn log(&mut self, level: Level, message: &str);
}

/// The app: the views open on its dashboards.
#[derive(Debug, Default)]
pub struct App {
    views: BTreeMap<String, View>,
}

fn message(kind: &str, surface: &str, body: Value) -> Value {
    let mut body = body;
    body["surfaceId"] = json!(surface);
    json!({"version": "1.0", kind: body})
}

impl App {
    /// The first time, the default dashboard; later, its fixes.
    pub fn start(&mut self, host: &mut impl Host) {
        if let Err(why) = model::first_start(host) {
            host.log(Level::Error, &format!("Couldn't keep the dashboard: {why}"));
        }
    }

    pub fn stop(&mut self, host: &mut impl Host) {
        host.cancel_timer(REFRESH);
        self.views.clear();
    }

    /// The person opened `view` as `surface`: its first contents, now.
    pub fn view_open(&mut self, host: &mut impl Host, view: &str, surface: &str) -> Result<(), String> {
        if !view.is_empty() && view != "main" {
            return Err(format!("Charts has no view \"{view}\"."));
        }
        let first = model::ids(host).into_iter().next().unwrap_or_default();
        self.views.insert(surface.to_owned(), View { dashboard: first, ..View::default() });
        let shown = self.show(host, surface, true, Vec::new());
        if let Err(why) = shown {
            self.views.remove(surface);
            return Err(why);
        }
        self.schedule(host);
        Ok(())
    }

    /// Nobody shows `surface` any more: once none is open, nothing runs.
    pub fn view_closed(&mut self, host: &mut impl Host, surface: &str) {
        self.views.remove(surface);
        if self.views.is_empty() {
            host.cancel_timer(REFRESH);
        }
    }

    pub fn on_timer(&mut self, host: &mut impl Host, name: &str) {
        if name == REFRESH {
            self.refresh(host);
        }
    }

    /// The surfaces open now.
    pub fn open_surfaces(&self) -> Vec<String> {
        self.views.keys().cloned().collect()
    }

    /// Runs the queries again for the views that are due (or whose sources
    /// were granted or taken back, or whose dashboard changed), and pushes
    /// what changed.
    pub fn refresh(&mut self, host: &mut impl Host) {
        if self.views.is_empty() {
            return;
        }
        let now = host.now_ms();
        let granted = granted(&host.data_sources());
        let due: Vec<String> = self
            .views
            .iter()
            .filter(|(_, view)| view.stale || now >= view.due || view.granted != granted)
            .map(|(surface, _)| surface.clone())
            .collect();
        for surface in due {
            if let Err(why) = self.show(host, &surface, true, Vec::new()) {
                host.log(Level::Warn, &format!("Couldn't update {surface}: {why}"));
                self.views.remove(&surface);
            }
        }
        self.schedule(host);
    }

    /// The next look: soon while a view waits for a source, else when the
    /// first is due.
    fn schedule(&mut self, host: &mut impl Host) {
        if self.views.is_empty() {
            host.cancel_timer(REFRESH);
            return;
        }
        let now = host.now_ms();
        let next = self
            .views
            .values()
            .map(|view| {
                let due = u64::try_from(view.due - now).unwrap_or(0).max(1);
                if view.ran.waiting { due.min(ASK_AGAIN_MS) } else { due }
            })
            .min()
            .unwrap_or(ASK_AGAIN_MS);
        host.set_timer(REFRESH, next);
    }

    /// A dashboard changed: every view runs again, from a timer right after
    /// this call.
    fn touch(&mut self, host: &mut impl Host) {
        if self.views.is_empty() {
            return;
        }
        for view in self.views.values_mut() {
            view.stale = true;
        }
        host.set_timer(REFRESH, 1);
    }

    /// Draws `surface` again (running its queries when `rerun`), pushing
    /// `data` (path, value) first, then whatever else changed.
    fn show(&mut self, host: &mut impl Host, surface: &str, rerun: bool, data: Vec<(String, Value)>) -> Result<(), String> {
        let ids = model::ids(host);
        let boards: Vec<(String, String)> =
            ids.iter().filter_map(|id| model::load(host, id).map(|dashboard| (id.clone(), dashboard.name))).collect();
        let Some(view) = self.views.get_mut(surface) else { return Ok(()) };
        let mut dashboard = model::load(host, &view.dashboard);
        if dashboard.is_none() {
            // Deleted meanwhile: the first there is.
            let first = ids.first().cloned().unwrap_or_default();
            if first != view.dashboard {
                *view = View {
                    dashboard: first,
                    created: view.created,
                    components: std::mem::take(&mut view.components),
                    panels: std::mem::take(&mut view.panels),
                    ..View::default()
                };
            }
            dashboard = model::load(host, &view.dashboard);
        }
        let sources = host.data_sources();
        let has_previous = dashboard.as_ref().is_some_and(|dashboard| model::previous(host, &dashboard.id).is_some());
        if rerun {
            let now = host.now_ms();
            view.ran = match &dashboard {
                Some(dashboard) => view::run(host, view, dashboard, &sources),
                None => view::Ran::default(),
            };
            view.granted = granted(&sources);
            view.stale = false;
            let refresh = dashboard.as_ref().map_or(30, |dashboard| dashboard.refresh_s).max(MIN_REFRESH_S);
            view.due = now + i64::try_from(refresh * 1000).unwrap_or(i64::MAX / 2);
        }
        let scene = Scene { boards: &boards, dashboard: dashboard.as_ref(), has_previous, sources: &sources, ran: &view.ran };
        let components = view::components(view, &scene);
        let panels = match &dashboard {
            Some(dashboard) if !view.ran.nothing_granted => view::panels_data(dashboard, &view.ran),
            _ => json!([]),
        };
        if !view.created {
            let mut model = json!({"panels": panels});
            if let Some(editor) = &view.editor {
                model["editor"] = editor.draft.data();
            }
            host.push_view(
                surface,
                &message("createSurface", surface, json!({"catalogId": CATALOG, "components": components, "dataModel": model})),
            )?;
            view.created = true;
            view.components =
                components.into_iter().map(|component| (component["id"].as_str().unwrap_or_default().to_owned(), component)).collect();
            view.panels = panels;
            return Ok(());
        }
        let mut messages = Vec::new();
        for (path, value) in data {
            messages.push(message("updateDataModel", surface, json!({"path": path, "value": value})));
        }
        if view.panels != panels {
            messages.push(message("updateDataModel", surface, json!({"path": "/panels", "value": panels})));
        }
        let changed: Vec<Value> = components
            .iter()
            .filter(|component| view.components.get(component["id"].as_str().unwrap_or_default()) != Some(*component))
            .cloned()
            .collect();
        if !changed.is_empty() {
            messages.push(message("updateComponents", surface, json!({"components": changed})));
        }
        for message in messages {
            host.push_view(surface, &message)?;
        }
        for component in components {
            view.components.insert(component["id"].as_str().unwrap_or_default().to_owned(), component);
        }
        view.panels = panels;
        Ok(())
    }

    /// Something the person did in `surface`.
    pub fn view_action(&mut self, host: &mut impl Host, surface: &str, message: &str) {
        let Ok(message) = serde_json::from_str::<Value>(message) else { return };
        let Some(action) = message.get("action") else {
            host.log(Level::Warn, &format!("{surface}: {message}"));
            return;
        };
        let name = action["name"].as_str().unwrap_or_default();
        let context = &action["context"];
        if !self.views.contains_key(surface) {
            return;
        }
        let shown = self.act(host, surface, name, context);
        if let Err(why) = shown {
            host.log(Level::Warn, &format!("Couldn't update {surface}: {why}"));
            self.views.remove(surface);
        }
        self.schedule(host);
    }

    fn act(&mut self, host: &mut impl Host, surface: &str, name: &str, context: &Value) -> Result<(), String> {
        let text = |key: &str| context[key].as_str().unwrap_or_default().to_owned();
        let Some(view) = self.views.get_mut(surface) else { return Ok(()) };
        view.notice.clear();
        let dashboard = model::load(host, &view.dashboard);
        let mut rerun = false;
        let mut touched = false;
        let mut data = Vec::new();
        match name {
            "edit_mode" => {
                view.editing = context["on"].as_bool().unwrap_or(false);
                if !view.editing {
                    view.editor = None;
                    view.form = None;
                    view.confirm_delete = false;
                }
            }
            "range" => {
                let from = text("from");
                if sql::time(&from, 0x7fff_ffff_ffff).is_some() {
                    view.range = Some(from);
                    rerun = true;
                }
            }
            "variable" => {
                view.values.insert(text("name"), text("value"));
                rerun = true;
            }
            "board" => {
                let id = text("id");
                if model::load(host, &id).is_some() {
                    *view = View {
                        dashboard: id,
                        created: true,
                        components: std::mem::take(&mut view.components),
                        panels: std::mem::take(&mut view.panels),
                        ..View::default()
                    };
                    rerun = true;
                }
            }
            "add_panel" => {
                let sources = host.data_sources();
                let source = sources
                    .iter()
                    .filter(|source| source.granted)
                    .map(|source| source.id.clone())
                    .min_by_key(|id| *id != ACTIVITY)
                    .unwrap_or_default();
                let draft = Draft { source, kind: "timeseries".into(), width: 6, height: 4, ..Draft::default() };
                data.push(("/editor".to_owned(), draft.data()));
                view.editor = Some(Editor { panel: None, draft, message: String::new(), preview: None });
                view.form = None;
                view.confirm_delete = false;
            }
            "edit_panel" => {
                if let Some(panel) = dashboard.as_ref().and_then(|dashboard| dashboard.panel(&text("panel"))) {
                    let draft = Draft::of(panel);
                    data.push(("/editor".to_owned(), draft.data()));
                    view.editor = Some(Editor { panel: Some(panel.id.clone()), draft, message: String::new(), preview: None });
                    view.form = None;
                    view.confirm_delete = false;
                }
            }
            "cancel_panel" => {
                view.editor = None;
                data.push(("/editor".to_owned(), Value::Null));
            }
            "run" => {
                if let Some(dashboard) = &dashboard
                    && view.editor.is_some()
                {
                    let draft = Draft::from_context(context);
                    let problem = view::draft_problem(&draft, dashboard);
                    let preview = match problem {
                        Some(_) => None,
                        None => {
                            let sources = host.data_sources();
                            let now = host.now_ms();
                            let values = view::values(view, dashboard, now);
                            let query = model::Query { source: draft.source.clone(), sql: draft.sql.clone() };
                            Some(view::query(host, &query, &values, &sources))
                        }
                    };
                    let editor = view.editor.as_mut().expect("editing");
                    editor.draft = draft;
                    editor.message = problem.unwrap_or_default();
                    editor.preview = preview;
                    data.push(("/editor/preview".to_owned(), view::preview_data(editor)));
                }
            }
            "save_panel" => {
                if let (Some(editor), Some(dashboard)) = (view.editor.as_mut(), &dashboard) {
                    editor.draft = Draft::from_context(context);
                    let problem = if editor.draft.title.is_empty() {
                        Some("Enter a title.".to_owned())
                    } else {
                        view::draft_problem(&editor.draft, dashboard)
                    };
                    let next = view::with_draft(dashboard, editor);
                    let problem = problem.or_else(|| next.problems(&host.data_sources()).first().map(|why| plain(why)));
                    match problem {
                        Some(why) => editor.message = why,
                        None => match model::save(host, next, None, "") {
                            Ok(_) => {
                                view.editor = None;
                                data.push(("/editor".to_owned(), Value::Null));
                                touched = true;
                                rerun = true;
                            }
                            Err(why) => editor.message = format!("Couldn't save: {why}."),
                        },
                    }
                }
            }
            "remove_panel" => {
                if let Some(dashboard) = &dashboard {
                    let id = text("panel");
                    if let Some(panel) = dashboard.panel(&id) {
                        let title = panel.title.clone();
                        let mut next = dashboard.clone();
                        next.panels.retain(|panel| panel.id != id);
                        match model::save(host, next, None, "") {
                            Ok(_) => {
                                if view.editor.as_ref().is_some_and(|editor| editor.panel.as_deref() == Some(id.as_str())) {
                                    view.editor = None;
                                }
                                view.notice = format!("Removed {title}. Restore previous version brings it back.");
                                touched = true;
                                rerun = true;
                            }
                            Err(why) => view.notice = format!("Couldn't remove \"{title}\": {why}."),
                        }
                    }
                }
            }
            "new_board" | "rename_board" => {
                let rename = name == "rename_board" && dashboard.is_some();
                let name =
                    if rename { dashboard.as_ref().map(|dashboard| dashboard.name.clone()).unwrap_or_default() } else { String::new() };
                data.push(("/form".to_owned(), json!({"name": name})));
                view.form = Some(Form { rename, message: String::new() });
                view.editor = None;
                view.confirm_delete = false;
            }
            "cancel_board" => view.form = None,
            "save_board" => {
                let name = text("name").trim().to_owned();
                if let Some(form) = view.form.as_mut() {
                    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
                        form.message = "Enter a name of up to 60 characters.".into();
                    } else {
                        let next = match (&dashboard, form.rename) {
                            (Some(dashboard), true) => Dashboard { name, ..dashboard.clone() },
                            _ => view::new_dashboard(&name, &model::ids(host)),
                        };
                        match model::save(host, next, None, "") {
                            Ok(saved) => {
                                if !form.rename {
                                    view.dashboard = saved.id;
                                    view.range = None;
                                    view.values.clear();
                                    view.editing = true;
                                }
                                view.form = None;
                                touched = true;
                                rerun = true;
                            }
                            Err(why) => form.message = format!("Couldn't save: {why}."),
                        }
                    }
                }
            }
            "delete_board" => {
                view.confirm_delete = true;
                view.editor = None;
                view.form = None;
            }
            "cancel_delete" => view.confirm_delete = false,
            "confirm_delete" => {
                view.confirm_delete = false;
                match model::delete(host, &view.dashboard) {
                    Ok(_) => {
                        view.editing = false;
                        touched = true;
                        rerun = true;
                    }
                    Err(why) => view.notice = format!("Couldn't delete it: {why}."),
                }
            }
            "restore" => match model::restore(host, &view.dashboard, "") {
                Ok(_) => {
                    view.notice = "Restored the previous version.".into();
                    view.editor = None;
                    touched = true;
                    rerun = true;
                }
                Err(why) => view.notice = format!("Couldn't restore it: {why}."),
            },
            other => host.log(Level::Debug, &format!("{surface}: no action \"{other}\"")),
        }
        if touched {
            self.touch(host);
        }
        self.show(host, surface, rerun, data)
    }
}

/// The ids of the sources granted.
fn granted(sources: &[DataSource]) -> Vec<String> {
    sources.iter().filter(|source| source.granted).map(|source| source.id.clone()).collect()
}

/// A problem with a panel, as the editor says it: without where it is.
fn plain(problem: &str) -> String {
    let why = problem.split_once(": ").map_or(problem, |(_, why)| why);
    let mut why = why.to_owned();
    if let Some(first) = why.get(..1) {
        why.replace_range(..1, &first.to_uppercase());
    }
    format!("{why}.")
}
