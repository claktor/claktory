//! The view: what one open surface shows (which dashboard, its range and
//! variables, the panel editor and the forms), running its queries, and
//! the components and data it's drawn from.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{self, CHART_KINDS, COLUMNS, Dashboard, Grid, Panel, Query, Viz};
use crate::sql;
use crate::{DataError, DataSource, Host};

/// The ranges offered, besides the dashboard's own.
pub const RANGES: [&str; 5] = ["now-1h", "now-24h", "now-7d", "now-30d", "now-90d"];

/// A chart's most rows, downsampled past it; a table's most rows.
pub const CHART_ROWS: usize = 2_000;
pub const TABLE_ROWS: usize = 200;
/// The most rows of all a view's panels together, so a refresh stays well
/// under a message's megabyte.
const VIEW_ROWS: usize = 16_000;
/// A variable's most options shown.
pub const MAX_OPTIONS: usize = 40;

/// What one surface shows, besides the dashboard itself.
#[derive(Debug, Clone, Default)]
pub struct View {
    /// The dashboard it shows.
    pub dashboard: String,
    /// The range chosen, when not the dashboard's own.
    pub range: Option<String>,
    /// The variables' values chosen, by name.
    pub values: BTreeMap<String, String>,
    /// Showing the controls to change the dashboard.
    pub editing: bool,
    pub editor: Option<Editor>,
    pub form: Option<Form>,
    pub confirm_delete: bool,
    /// One line about what just happened ("Couldn't save: …").
    pub notice: String,
    /// What was last pushed.
    pub components: BTreeMap<String, Value>,
    pub panels: Value,
    /// When its queries run again (ms).
    pub due: i64,
    /// Its queries need running again now, whatever `due` says.
    pub stale: bool,
    /// The sources granted when it last ran.
    pub granted: Vec<String>,
    /// What its queries came to when they last ran.
    pub ran: Ran,
    /// Its createSurface was pushed.
    pub created: bool,
}

/// The panel editor.
#[derive(Debug, Clone, PartialEq)]
pub struct Editor {
    /// The panel changed, or None for a new one.
    pub panel: Option<String>,
    pub draft: Draft,
    /// Why the last Run or Save didn't work.
    pub message: String,
    /// What the last Run showed, when there was one.
    pub preview: Option<Outcome>,
}

/// A panel as the editor's fields have it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Draft {
    pub title: String,
    pub source: String,
    pub sql: String,
    pub kind: String,
    pub x: String,
    pub series_by: String,
    pub unit: String,
    pub width: u32,
    pub height: u32,
}

impl Draft {
    pub fn of(panel: &Panel) -> Self {
        Self {
            title: panel.title.clone(),
            source: panel.query.source.clone(),
            sql: panel.query.sql.clone(),
            kind: panel.viz.kind.clone(),
            x: panel.viz.x.clone().unwrap_or_default(),
            series_by: panel.viz.series_by.clone().unwrap_or_default(),
            unit: panel.viz.unit.clone(),
            width: panel.grid.w,
            height: panel.grid.h,
        }
    }

    /// The fields as an action's context sends them.
    pub fn from_context(context: &Value) -> Self {
        let text = |key: &str| match &context[key] {
            Value::String(text) => text.clone(),
            Value::Array(items) => items.first().and_then(Value::as_str).unwrap_or_default().to_owned(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        };
        let number = |key: &str, default: u32| text(key).trim().parse().unwrap_or(default);
        Self {
            title: text("title").trim().to_owned(),
            source: text("source"),
            sql: text("sql"),
            kind: text("kind"),
            x: text("x").trim().to_owned(),
            series_by: text("series_by").trim().to_owned(),
            unit: text("unit").trim().to_owned(),
            width: number("width", COLUMNS),
            height: number("height", 4),
        }
    }

    /// The editor's data model.
    pub fn data(&self) -> Value {
        json!({
            "title": self.title, "source": [self.source], "sql": self.sql, "kind": [self.kind],
            "x": self.x, "series_by": self.series_by, "unit": self.unit,
            "width": [self.width.to_string()], "height": [self.height.to_string()],
        })
    }

    /// The panel it makes, as `id` at `grid` (keeping what the editor
    /// doesn't show from `old`).
    pub fn panel(&self, id: &str, grid: Grid, old: Option<&Panel>) -> Panel {
        let optional = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        let kept = old.map(|panel| panel.viz.clone()).unwrap_or_default();
        Panel {
            id: id.to_owned(),
            title: self.title.clone(),
            grid: Grid { w: self.width, h: self.height, x: grid.x.min(COLUMNS.saturating_sub(self.width)), ..grid },
            query: Query { source: self.source.clone(), sql: self.sql.clone() },
            viz: Viz {
                kind: self.kind.clone(),
                unit: self.unit.clone(),
                x: optional(&self.x),
                series_by: optional(&self.series_by),
                y: if self.series_by.is_empty() || kept.y.len() <= 1 { kept.y } else { Vec::new() },
                color: kept.color,
                columns: kept.columns,
            },
        }
    }
}

/// The dashboard form: a new dashboard, or a new name for this one.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    pub rename: bool,
    pub message: String,
}

/// What a panel's query came to.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Table(Value),
    Failed(String),
    /// Its source isn't granted: its label.
    NotGranted(String),
}

/// One run of a view's queries.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Ran {
    pub panels: Vec<Outcome>,
    /// Each variable's options: (value, label).
    pub options: Vec<Vec<(String, String)>>,
    /// Nothing Charts reads is granted.
    pub nothing_granted: bool,
    /// Some panel waits for its source.
    pub waiting: bool,
}

/// The range a view shows, as written.
pub fn range(view: &View, dashboard: Option<&Dashboard>) -> (String, String) {
    match (view.range.as_ref(), dashboard) {
        (Some(from), _) => (from.clone(), "now".into()),
        (None, Some(dashboard)) => (dashboard.time.from.clone(), dashboard.time.to.clone()),
        (None, None) => ("now-7d".into(), "now".into()),
    }
}

/// The values a query of `dashboard` is bound with in `view`.
pub fn values(view: &View, dashboard: &Dashboard, now: i64) -> BTreeMap<String, Value> {
    let (from, to) = range(view, Some(dashboard));
    let to = sql::time(&to, now).unwrap_or(now);
    let from = sql::time(&from, now).filter(|from| *from < to).unwrap_or(to - 7 * 86_400_000);
    let mut values = sql::builtins(from, to, now);
    for variable in &dashboard.variables {
        let value = view.values.get(&variable.name).unwrap_or(&variable.default);
        values.insert(variable.name.clone(), json!(value));
    }
    values
}

/// Runs one query, bound with `values`.
pub fn query(host: &mut impl Host, query: &Query, values: &BTreeMap<String, Value>, sources: &[DataSource]) -> Outcome {
    let Some(source) = sources.iter().find(|source| source.id == query.source) else {
        return Outcome::Failed(format!("Charts doesn't read \"{}\".", query.source));
    };
    if !source.granted {
        return Outcome::NotGranted(source.label.clone());
    }
    let bound = match sql::bind(&query.sql, values) {
        Ok(bound) => bound,
        Err(why) => return Outcome::Failed(why),
    };
    match host.data_query(&query.source, &bound.sql, &Value::Array(bound.params)) {
        Ok(table) => Outcome::Table(table),
        Err(DataError::NotGranted) => Outcome::NotGranted(source.label.clone()),
        Err(DataError::UnknownSource) => Outcome::Failed("Charts can't read this source any more.".into()),
        Err(DataError::Invalid(why) | DataError::Failed(why)) => Outcome::Failed(why),
    }
}

/// Runs a dashboard's variables' and panels' queries for `view`.
pub fn run(host: &mut impl Host, view: &View, dashboard: &Dashboard, sources: &[DataSource]) -> Ran {
    let now = host.now_ms();
    let values = values(view, dashboard, now);
    let nothing_granted = !sources.iter().any(|source| source.granted);
    if nothing_granted {
        return Ran { nothing_granted, waiting: true, ..Ran::default() };
    }
    let options = dashboard
        .variables
        .iter()
        .map(|variable| match query(host, &variable.query, &values, sources) {
            Outcome::Table(table) => table["rows"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|row| {
                    let row = row.as_array()?;
                    let value = cell(row.first()?);
                    let label = row.get(1).map(cell).filter(|label| !label.is_empty()).unwrap_or_else(|| value.clone());
                    (!value.is_empty()).then_some((value, label))
                })
                .take(MAX_OPTIONS)
                .collect(),
            _ => Vec::new(),
        })
        .collect();
    let panels: Vec<Outcome> = dashboard.panels.iter().map(|panel| query(host, &panel.query, &values, sources)).collect();
    let waiting = panels.iter().any(|outcome| matches!(outcome, Outcome::NotGranted(_)));
    Ran { panels, options, nothing_granted, waiting }
}

fn cell(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// What the view shows besides its own state: the dashboards there are,
/// the one shown, the sources and what its queries came to.
pub struct Scene<'a> {
    pub boards: &'a [(String, String)],
    pub dashboard: Option<&'a Dashboard>,
    pub has_previous: bool,
    pub sources: &'a [DataSource],
    pub ran: &'a Ran,
}

// ---- Components ----

fn text(id: &str, text: impl Into<Value>, variant: &str) -> Value {
    json!({"id": id, "component": "Text", "text": text.into(), "variant": variant})
}

/// A button and its words.
fn button(out: &mut Vec<Value>, id: &str, label: &str, name: &str, context: Value, variant: &str) {
    out.push(json!({
        "id": id, "component": "Button", "child": format!("{id}_t"), "variant": variant,
        "action": {"event": {"name": name, "context": context}},
    }));
    out.push(text(&format!("{id}_t"), label, "body"));
}

/// How a range reads on its button: "7 days".
pub fn range_label(from: &str) -> String {
    let Some(ago) = from.strip_prefix("now-") else { return from.to_owned() };
    let (count, unit) = ago.split_at(ago.len().saturating_sub(1));
    let one = count == "1";
    let unit = match (unit, one) {
        ("m", true) => "minute",
        ("m", false) => "minutes",
        ("h", true) => "hour",
        ("h", false) => "hours",
        ("d", true) => "day",
        ("d", false) => "days",
        ("w", true) => "week",
        ("w", false) => "weeks",
        _ => return from.to_owned(),
    };
    format!("{count} {unit}")
}

/// The whole view's components, root first.
pub fn components(view: &View, scene: &Scene) -> Vec<Value> {
    let mut out = Vec::new();
    let mut children: Vec<String> = Vec::new();
    fn add(out: &mut Vec<Value>, children: &mut Vec<String>, id: &str, built: Vec<Value>) {
        children.push(id.to_owned());
        out.extend(built);
    }
    let dashboard = scene.dashboard;

    // The heading: the dashboard's name, and Edit or Done.
    let mut head = Vec::new();
    head.push(json!({"id": "name", "component": "Text", "text": dashboard.map_or("Charts", |d| d.name.as_str()), "variant": "heading"}));
    let mut head_children = vec!["name".to_owned()];
    if dashboard.is_some() && !scene.ran.nothing_granted {
        let (label, on) = if view.editing { ("Done", false) } else { ("Edit", true) };
        button(&mut head, "edit", label, "edit_mode", json!({"on": on}), "borderless");
        head_children.push("edit".into());
    }
    // Its natural width beside the name: a Row would give it half.
    head.insert(0, json!({"id": "head", "component": "List", "direction": "horizontal", "align": "center", "children": head_children}));
    add(&mut out, &mut children, "head", head);

    // The dashboards, when there's a choice.
    if scene.boards.len() > 1 {
        let mut built = Vec::new();
        let mut ids = Vec::new();
        for (index, (id, name)) in scene.boards.iter().enumerate() {
            let shown = dashboard.is_some_and(|d| d.id == *id);
            let component = format!("board{index}");
            button(&mut built, &component, name, "board", json!({"id": id}), if shown { "primary" } else { "default" });
            ids.push(component);
        }
        built.insert(0, json!({"id": "boards", "component": "List", "direction": "horizontal", "children": ids}));
        add(&mut out, &mut children, "boards", built);
    }

    let Some(dashboard) = dashboard else {
        let mut built = vec![json!({
            "id": "empty", "component": "EmptyState", "icon": "chart-column",
            "title": "No dashboards yet", "body": "Make one to chart what your agents do.",
        })];
        button(&mut built, "new", "New dashboard", "new_board", json!({}), "primary");
        add(&mut out, &mut children, "empty", built.drain(..1).collect());
        children.push("new".into());
        out.extend(built);
        dashboard_form(view, &mut out, &mut children);
        notice(view, &mut out, &mut children);
        out.insert(0, json!({"id": "root", "component": "Column", "children": children}));
        return out;
    };

    if scene.ran.nothing_granted {
        add(
            &mut out,
            &mut children,
            "ask",
            vec![json!({
                "id": "ask", "component": "EmptyState", "icon": "chart-column",
                "title": "Charts can't read anything yet",
                "body": "To show your charts, turn on Claktory's records for Charts in Settings → Apps.",
            })],
        );
        out.insert(0, json!({"id": "root", "component": "Column", "children": children}));
        return out;
    }

    // The range.
    let (from, _) = range(view, Some(dashboard));
    let mut ranges: Vec<String> = RANGES.iter().map(|range| (*range).to_owned()).collect();
    if !ranges.contains(&dashboard.time.from) && dashboard.time.to == "now" {
        ranges.insert(0, dashboard.time.from.clone());
    }
    let mut built = Vec::new();
    let mut ids = Vec::new();
    for (index, range) in ranges.iter().enumerate() {
        let id = format!("range{index}");
        button(&mut built, &id, &range_label(range), "range", json!({"from": range}), if *range == from { "primary" } else { "default" });
        ids.push(id);
    }
    built.insert(0, json!({"id": "ranges", "component": "List", "direction": "horizontal", "children": ids}));
    add(&mut out, &mut children, "ranges", built);

    // The variables: All, then each option.
    for (index, variable) in dashboard.variables.iter().enumerate() {
        let chosen = view.values.get(&variable.name).unwrap_or(&variable.default);
        let label = if variable.label.is_empty() { &variable.name } else { &variable.label };
        let id = format!("var{index}");
        let mut built = vec![text(&format!("{id}_label"), label.as_str(), "caption")];
        let mut ids = vec![format!("{id}_label")];
        let options = scene.ran.options.get(index).cloned().unwrap_or_default();
        let all = std::iter::once((String::new(), "All".to_owned())).chain(options);
        for (option, (value, shown)) in all.enumerate() {
            let component = format!("{id}_o{option}");
            let variant = if value == *chosen { "primary" } else { "default" };
            button(&mut built, &component, &shown, "variable", json!({"name": variable.name, "value": value}), variant);
            ids.push(component);
        }
        built.insert(0, json!({"id": id, "component": "List", "direction": "horizontal", "align": "center", "children": ids}));
        add(&mut out, &mut children, &id, built);
    }

    // Changing it.
    if view.editing {
        let mut built = Vec::new();
        let mut ids = Vec::new();
        let mut tool = |built: &mut Vec<Value>, id: &str, label: &str, name: &str| {
            button(built, id, label, name, json!({}), "default");
            ids.push(id.to_owned());
        };
        tool(&mut built, "add", "Add panel", "add_panel");
        tool(&mut built, "rename", "Rename", "rename_board");
        tool(&mut built, "new", "New dashboard", "new_board");
        if scene.has_previous {
            tool(&mut built, "restore", "Restore previous version", "restore");
        }
        tool(&mut built, "remove", "Delete dashboard", "delete_board");
        built.insert(0, json!({"id": "tools", "component": "List", "direction": "horizontal", "children": ids}));
        add(&mut out, &mut children, "tools", built);
    }
    if view.confirm_delete {
        let mut built = vec![
            json!({"id": "confirm", "component": "Card", "child": "confirm_col"}),
            json!({"id": "confirm_col", "component": "Column", "children": ["confirm_text", "confirm_row"]}),
            text("confirm_text", format!("Delete {} and its panels?", dashboard.name), "body"),
            json!({"id": "confirm_row", "component": "List", "direction": "horizontal", "children": ["confirm_yes", "confirm_no"]}),
        ];
        button(&mut built, "confirm_yes", "Delete", "confirm_delete", json!({}), "primary");
        button(&mut built, "confirm_no", "Cancel", "cancel_delete", json!({}), "borderless");
        add(&mut out, &mut children, "confirm", built);
    }
    dashboard_form(view, &mut out, &mut children);
    if let Some(editor) = &view.editor {
        children.push("ed".into());
        out.extend(editor_components(editor, scene.sources));
    }
    notice(view, &mut out, &mut children);

    // The panels, row by row.
    if dashboard.panels.is_empty() {
        let body = if view.editing { "Choose Add panel to chart something." } else { "Choose Edit, then Add panel." };
        add(
            &mut out,
            &mut children,
            "nopanels",
            vec![json!({"id": "nopanels", "component": "EmptyState", "icon": "chart-column", "title": "No panels yet", "body": body})],
        );
    }
    for (row, panels) in dashboard.rows().into_iter().enumerate() {
        let id = format!("row{row}");
        let mut cells = Vec::new();
        let mut built = Vec::new();
        let mut at = 0;
        let gap = |built: &mut Vec<Value>, cells: &mut Vec<String>, width: u32| {
            let id = format!("row{row}_gap{}", cells.len());
            built.push(json!({"id": id, "component": "Column", "children": [], "weight": width}));
            cells.push(id);
        };
        for panel in panels {
            if panel.grid.x > at {
                gap(&mut built, &mut cells, panel.grid.x - at);
            }
            let index = dashboard.panels.iter().position(|p| p.id == panel.id).unwrap_or(0);
            let outcome = scene.ran.panels.get(index).cloned().unwrap_or(Outcome::Table(json!({"columns": ["x"], "rows": []})));
            built.extend(panel_card(index, panel, &outcome, view.editing));
            cells.push(format!("p{index}"));
            at = at.max(panel.grid.x + panel.grid.w);
        }
        if at < COLUMNS {
            gap(&mut built, &mut cells, COLUMNS - at);
        }
        built.insert(0, json!({"id": id, "component": "Row", "align": "start", "children": cells}));
        add(&mut out, &mut children, &id, built);
    }
    out.insert(0, json!({"id": "root", "component": "Column", "children": children}));
    out
}

fn notice(view: &View, out: &mut Vec<Value>, children: &mut Vec<String>) {
    if !view.notice.is_empty() {
        out.push(text("notice", view.notice.as_str(), "caption"));
        children.push("notice".into());
    }
}

/// The form for a new dashboard's name, or this one's.
fn dashboard_form(view: &View, out: &mut Vec<Value>, children: &mut Vec<String>) {
    let Some(form) = &view.form else { return };
    let mut col = vec!["form_name".to_owned()];
    out.push(json!({"id": "form", "component": "Card", "child": "form_col"}));
    out.push(json!({"id": "form_name", "component": "TextField", "label": if form.rename { "Name" } else { "New dashboard's name" }, "value": {"@path": "/form/name"}}));
    if !form.message.is_empty() {
        out.push(text("form_message", form.message.as_str(), "caption"));
        col.push("form_message".into());
    }
    col.push("form_row".into());
    out.push(json!({"id": "form_col", "component": "Column", "children": col}));
    out.push(json!({"id": "form_row", "component": "List", "direction": "horizontal", "children": ["form_save", "form_cancel"]}));
    let save = if form.rename { "Save" } else { "Create" };
    button(out, "form_save", save, "save_board", json!({"name": {"@path": "/form/name"}}), "primary");
    button(out, "form_cancel", "Cancel", "cancel_board", json!({}), "borderless");
    children.push("form".into());
}

/// A panel's height in a chart's points.
fn height(grid: Grid) -> u32 {
    (grid.h * 50).clamp(120, 480)
}

/// A panel's card: its body, and in edit mode Edit and Remove.
fn panel_card(index: usize, panel: &Panel, outcome: &Outcome, editing: bool) -> Vec<Value> {
    let id = format!("p{index}");
    let body = format!("{id}_body");
    let mut out = panel_body(&body, &format!("/panels/{index}"), &panel.title, &panel.viz, panel.grid, outcome);
    if editing {
        out.push(json!({"id": id, "component": "Card", "child": format!("{id}_col"), "weight": panel.grid.w}));
        out.push(json!({"id": format!("{id}_col"), "component": "Column", "children": [body, format!("{id}_acts")]}));
        out.push(json!({"id": format!("{id}_acts"), "component": "List", "direction": "horizontal", "children": [format!("{id}_edit"), format!("{id}_remove")]}));
        button(&mut out, &format!("{id}_edit"), "Edit", "edit_panel", json!({"panel": panel.id}), "borderless");
        button(&mut out, &format!("{id}_remove"), "Remove", "remove_panel", json!({"panel": panel.id}), "borderless");
    } else {
        out.push(json!({"id": id, "component": "Card", "child": body, "weight": panel.grid.w}));
    }
    out
}

/// Whether `kind` is one Charts draws.
pub fn drawable(kind: &str) -> bool {
    kind == "stat" || kind == "table" || CHART_KINDS.contains(&kind)
}

/// A panel's body as `id`, bound to its data at `at`: its Stat, Chart or
/// Table, or why it can't show.
pub fn panel_body(id: &str, at: &str, title: &str, viz: &Viz, grid: Grid, outcome: &Outcome) -> Vec<Value> {
    let path = |part: &str| json!({"@path": format!("{at}/{part}")});
    let titled = |second: Value| {
        let second_id = second["id"].as_str().unwrap_or_default().to_owned();
        vec![
            json!({"id": id, "component": "Column", "children": [format!("{id}_title"), second_id]}),
            text(&format!("{id}_title"), title, "caption"),
            second,
        ]
    };
    match outcome {
        Outcome::Failed(_) | Outcome::NotGranted(_) => return titled(text(&format!("{id}_error"), path("error"), "body")),
        Outcome::Table(_) if !drawable(&viz.kind) => return titled(text(&format!("{id}_error"), path("error"), "body")),
        Outcome::Table(_) => {}
    }
    let mut component = match viz.kind.as_str() {
        "stat" => {
            let color = match viz.color.as_deref() {
                Some("muted") => "muted",
                _ => "signal",
            };
            json!({"id": id, "component": "Stat", "label": title, "value": path("value"), "spark": path("spark"), "color": color})
        }
        "table" => {
            let mut table = json!({"id": format!("{id}_table"), "component": "Table", "data": path("data")});
            if !viz.columns.is_empty() {
                table["columns"] = json!(viz.columns);
            }
            return titled(table);
        }
        kind => {
            let mut chart =
                json!({"id": id, "component": "Chart", "kind": kind, "data": path("data"), "label": title, "height": height(grid)});
            if let Some(x) = &viz.x {
                chart["x"] = json!(x);
            }
            if !viz.y.is_empty() {
                chart["y"] = json!(viz.y);
            }
            if let Some(series) = &viz.series_by {
                chart["seriesBy"] = json!(series);
            }
            if let Some(color) = &viz.color {
                chart["color"] = json!(color);
            }
            chart
        }
    };
    if !viz.unit.is_empty() {
        component["unit"] = json!(viz.unit);
    }
    vec![component]
}

/// A panel's data, where its body looks for it.
pub fn panel_data(viz: &Viz, outcome: &Outcome, rows: usize) -> Value {
    match outcome {
        Outcome::Failed(why) => json!({"error": format!("Couldn't run this panel's query: {why}")}),
        Outcome::NotGranted(label) => json!({"error": format!("To see this panel, turn on {label} for Charts in Settings → Apps.")}),
        Outcome::Table(_) if !drawable(&viz.kind) => json!({"error": format!("Charts can't draw \"{}\".", viz.kind)}),
        Outcome::Table(table) if viz.kind == "stat" => stat(table),
        Outcome::Table(table) if viz.kind == "table" => json!({"data": truncate(table, TABLE_ROWS.min(rows))}),
        Outcome::Table(table) => json!({"data": downsample(table, rows, viz.series_by.as_deref())}),
    }
}

/// The data of all a dashboard's panels, each held to its share of rows.
pub fn panels_data(dashboard: &Dashboard, ran: &Ran) -> Value {
    let share = (VIEW_ROWS / dashboard.panels.len().max(1)).clamp(TABLE_ROWS, CHART_ROWS);
    Value::Array(dashboard.panels.iter().zip(&ran.panels).map(|(panel, outcome)| panel_data(&panel.viz, outcome, share)).collect())
}

/// A Stat's value and spark line from a query's answer: the last column's
/// numbers, in order, and their total.
pub fn stat(table: &Value) -> Value {
    let rows = table["rows"].as_array().cloned().unwrap_or_default();
    let spark: Vec<f64> = rows.iter().filter_map(|row| row.as_array()?.last()?.as_f64()).collect();
    let total: f64 = spark.iter().sum();
    let value = if total.fract() == 0.0 && total.abs() < 9e15 { json!(total as i64) } else { json!(total) };
    let mut spark: Vec<f64> = spark.into_iter().rev().take(200).collect();
    spark.reverse();
    json!({"value": value, "spark": spark})
}

/// The first `most` rows, `more` when there were others.
pub fn truncate(table: &Value, most: usize) -> Value {
    let mut table = table.clone();
    if let Some(rows) = table["rows"].as_array_mut()
        && rows.len() > most
    {
        rows.truncate(most);
        table["more"] = json!(true);
    }
    table
}

/// At most about `most` rows, evenly spread over each series (the
/// `series_by` column's values, or all as one), first and last kept.
pub fn downsample(table: &Value, most: usize, series_by: Option<&str>) -> Value {
    let Some(rows) = table["rows"].as_array() else { return table.clone() };
    if rows.len() <= most {
        return table.clone();
    }
    let column = series_by.and_then(|name| table["columns"].as_array()?.iter().position(|column| column == name));
    let mut series: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        let key = column.and_then(|column| row.get(column)).map(cell).unwrap_or_default();
        series.entry(key).or_default().push(index);
    }
    let each = (most / series.len()).max(2);
    let mut kept: Vec<usize> = Vec::new();
    for indexes in series.values() {
        if indexes.len() <= each {
            kept.extend(indexes);
            continue;
        }
        let last = indexes.len() - 1;
        kept.extend((0..each).map(|n| indexes[n * last / (each - 1)]));
    }
    kept.sort_unstable();
    kept.dedup();
    let mut out = table.clone();
    out["rows"] = Value::Array(kept.into_iter().map(|index| rows[index].clone()).collect());
    out
}

// ---- The editor ----

const KIND_LABELS: [(&str, &str); 7] = [
    ("stat", "Number"),
    ("timeseries", "Line"),
    ("bars", "Bars"),
    ("stackedBars", "Stacked bars"),
    ("pie", "Pie"),
    ("scatter", "Scatter"),
    ("table", "Table"),
];
const WIDTHS: [(u32, &str); 5] = [(3, "Quarter"), (4, "Third"), (6, "Half"), (8, "Two thirds"), (12, "Full")];
const HEIGHTS: [(u32, &str); 4] = [(2, "Short"), (4, "Medium"), (6, "Tall"), (8, "Taller")];

fn sizes(known: &[(u32, &str)], current: u32, unit: &str) -> Value {
    let mut options: Vec<Value> = known.iter().map(|(value, label)| json!({"label": label, "value": value.to_string()})).collect();
    if !known.iter().any(|(value, _)| *value == current) {
        options.push(json!({"label": format!("{current} {unit}"), "value": current.to_string()}));
    }
    Value::Array(options)
}

/// The editor's fields, bound to `/editor`, its preview and its buttons.
pub fn editor_components(editor: &Editor, sources: &[DataSource]) -> Vec<Value> {
    let draft = &editor.draft;
    let field = |id: &str, label: &str, key: &str, placeholder: &str, variant: &str| json!({"id": id, "component": "TextField", "label": label, "value": {"@path": format!("/editor/{key}")}, "placeholder": placeholder, "variant": variant});
    let mut sources: Vec<Value> =
        sources.iter().filter(|source| source.granted).map(|source| json!({"label": source.label, "value": source.id})).collect();
    if !draft.source.is_empty() && !sources.iter().any(|source| source["value"] == draft.source) {
        sources.push(json!({"label": draft.source, "value": draft.source}));
    }
    let mut out = vec![
        json!({"id": "ed", "component": "Card", "child": "ed_col"}),
        text("ed_heading", if editor.panel.is_some() { "Edit panel" } else { "New panel" }, "heading"),
        field("ed_title", "Title", "title", "", "shortText"),
        json!({"id": "ed_source", "component": "ChoicePicker", "label": "Source", "options": sources, "value": {"@path": "/editor/source"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        field("ed_sql", "Query", "sql", "SELECT …", "longText"),
        text(
            "ed_help",
            "One read-only SQL statement. $from and $to are the range shown, in milliseconds ($from_s and $to_s in seconds); $step is a bucket that suits it; and each of the dashboard's variables is $ and its name.",
            "caption",
        ),
        json!({"id": "ed_kind", "component": "ChoicePicker", "label": "Show as", "options": KIND_LABELS.iter().map(|(value, label)| json!({"label": label, "value": value})).collect::<Vec<_>>(), "value": {"@path": "/editor/kind"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_columns", "component": "Row", "align": "start", "children": ["ed_x", "ed_series", "ed_unit"]}),
        json!({"id": "ed_x", "component": "TextField", "label": "Along the bottom", "value": {"@path": "/editor/x"}, "placeholder": "The first column", "weight": 1}),
        json!({"id": "ed_series", "component": "TextField", "label": "Split by", "value": {"@path": "/editor/series_by"}, "placeholder": "Nothing", "weight": 1}),
        json!({"id": "ed_unit", "component": "TextField", "label": "Unit", "value": {"@path": "/editor/unit"}, "placeholder": "None", "weight": 1}),
        json!({"id": "ed_width", "component": "ChoicePicker", "label": "Width", "options": sizes(&WIDTHS, draft.width, "columns"), "value": {"@path": "/editor/width"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_height", "component": "ChoicePicker", "label": "Height", "options": sizes(&HEIGHTS, draft.height, "units"), "value": {"@path": "/editor/height"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
    ];
    let mut col = vec!["ed_heading", "ed_title", "ed_source", "ed_sql", "ed_help", "ed_kind", "ed_columns", "ed_width", "ed_height"];
    if let Some(preview) = &editor.preview {
        let viz = Viz {
            kind: draft.kind.clone(),
            unit: draft.unit.clone(),
            x: (!draft.x.is_empty()).then(|| draft.x.clone()),
            series_by: (!draft.series_by.is_empty()).then(|| draft.series_by.clone()),
            ..Viz::default()
        };
        let grid = Grid { h: draft.height, ..Grid::default() };
        out.push(json!({"id": "ed_preview", "component": "Card", "child": "ed_preview_body"}));
        out.extend(panel_body("ed_preview_body", "/editor/preview", &draft.title, &viz, grid, preview));
        col.push("ed_preview");
    }
    if !editor.message.is_empty() {
        out.push(text("ed_message", editor.message.as_str(), "caption"));
        col.push("ed_message");
    }
    col.push("ed_buttons");
    out.push(json!({"id": "ed_col", "component": "Column", "children": col}));
    let context = json!({
        "title": {"@path": "/editor/title"}, "source": {"@path": "/editor/source"}, "sql": {"@path": "/editor/sql"},
        "kind": {"@path": "/editor/kind"}, "x": {"@path": "/editor/x"}, "series_by": {"@path": "/editor/series_by"},
        "unit": {"@path": "/editor/unit"}, "width": {"@path": "/editor/width"}, "height": {"@path": "/editor/height"},
    });
    let mut buttons = vec!["ed_run", "ed_save", "ed_cancel"];
    button(&mut out, "ed_run", "Run", "run", context.clone(), "default");
    button(&mut out, "ed_save", "Save", "save_panel", context, "primary");
    button(&mut out, "ed_cancel", "Cancel", "cancel_panel", json!({}), "borderless");
    if let Some(panel) = &editor.panel {
        button(&mut out, "ed_remove", "Remove panel", "remove_panel", json!({"panel": panel}), "borderless");
        buttons.push("ed_remove");
    }
    out.push(json!({"id": "ed_buttons", "component": "List", "direction": "horizontal", "children": buttons}));
    out
}

/// The editor's preview data.
pub fn preview_data(editor: &Editor) -> Value {
    match &editor.preview {
        None => Value::Null,
        Some(outcome) => {
            let viz = Viz {
                kind: editor.draft.kind.clone(),
                series_by: Some(editor.draft.series_by.clone()).filter(|s| !s.is_empty()),
                ..Viz::default()
            };
            panel_data(&viz, outcome, CHART_ROWS)
        }
    }
}

/// Checks a draft before it's run or kept: why not, if it can't be.
pub fn draft_problem(draft: &Draft, dashboard: &Dashboard) -> Option<String> {
    if draft.source.is_empty() {
        return Some("Choose a source.".into());
    }
    if draft.kind.is_empty() {
        return Some("Choose how to show it.".into());
    }
    let known: Vec<&str> = sql::BUILTINS.iter().copied().chain(dashboard.variables.iter().map(|v| v.name.as_str())).collect();
    sql::check(&draft.sql, &known).err().map(|why| format!("The query: {why}."))
}

/// The panel a draft makes in `dashboard` (a new one placed after the
/// others), and the dashboard with it.
pub fn with_draft(dashboard: &Dashboard, editor: &Editor) -> Dashboard {
    let mut next = dashboard.clone();
    match editor.panel.as_ref().and_then(|id| next.panels.iter().position(|panel| panel.id == *id)) {
        Some(index) => {
            let old = next.panels[index].clone();
            next.panels[index] = editor.draft.panel(&old.id, old.grid, Some(&old));
        }
        None => {
            let grid = dashboard.place(editor.draft.width.clamp(1, COLUMNS), editor.draft.height);
            let id = dashboard.new_panel_id(&editor.draft.title);
            next.panels.push(editor.draft.panel(&id, grid, None));
        }
    }
    next
}

/// A new, empty dashboard named `name`, with an id unlike the others'.
pub fn new_dashboard(name: &str, ids: &[String]) -> Dashboard {
    let mut base: String = name
        .to_lowercase()
        .chars()
        .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    base.truncate(32);
    if base.is_empty() {
        base = "dashboard".into();
    }
    let mut id = base.clone();
    let mut n = 2;
    while ids.contains(&id) {
        id = format!("{base}-{n}");
        n += 1;
    }
    let default = model::default_dashboard();
    Dashboard { id, name: name.to_owned(), panels: Vec::new(), variables: Vec::new(), ..default }
}
