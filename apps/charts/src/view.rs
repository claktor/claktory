//! The view: what one open surface shows (which dashboard, its range and
//! variables, the panel editor and the forms), running its queries, and
//! the components and data it's drawn from.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::model::{self, CHART_KINDS, COLUMNS, Dashboard, Grid, Panel, Query, Threshold, Viz};
use crate::sql;
use crate::{DataError, DataSource, Host};

/// The ranges offered, besides the dashboard's own.
pub const RANGES: [&str; 13] =
    ["now-5m", "now-15m", "now-30m", "now-1h", "now-3h", "now-6h", "now-12h", "now-24h", "now-2d", "now-7d", "now-30d", "now-90d", "now-1y"];

/// The width, in points, below which a row's panels go one under another.
const STACK_BELOW: u32 = 768;

/// A chart's most rows, downsampled past it; a table's most rows.
pub const CHART_ROWS: usize = 2_000;
pub const TABLE_ROWS: usize = 200;
/// The most rows of all a view's panels together, so a refresh stays well
/// under a message's megabyte.
const VIEW_ROWS: usize = 16_000;
/// A variable's most options shown: 50 agents and more (Scolippi 27155:
/// at 40, the last ten couldn't be chosen), within a view's 1,000
/// components while its menu is open.
pub const MAX_OPTIONS: usize = 200;

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
    /// The menu open under its button: "board", "range" or "var<index>".
    pub menu: Option<String>,
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
#[derive(Debug, Clone, PartialEq)]
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
    /// How a chart draws, as its fields have it: numbers as text, "" for
    /// unset (min, max, decimals).
    pub line_width: String,
    pub fill: String,
    pub points: String,
    pub legend: bool,
    pub min: String,
    pub max: String,
    pub decimals: String,
    /// What a stat shows (`model::REDUCERS`).
    pub reduce: String,
    /// A stat's thresholds as written: "green, 80 red".
    pub thresholds: String,
    pub color_mode: String,
    /// A stat's spark line shows.
    pub graph: bool,
    /// A map's column of countries, and its dots' sizes ("" for unset).
    pub location: String,
    pub size_min: String,
    pub size_max: String,
}

impl Default for Draft {
    fn default() -> Self {
        Self {
            title: String::new(),
            source: String::new(),
            sql: String::new(),
            kind: String::new(),
            x: String::new(),
            series_by: String::new(),
            unit: String::new(),
            width: COLUMNS,
            height: model::HEIGHT,
            line_width: number_text(LINE_WIDTH),
            fill: number_text(FILL),
            points: POINTS_AUTO.into(),
            legend: true,
            min: String::new(),
            max: String::new(),
            decimals: String::new(),
            reduce: model::NEW_REDUCE.into(),
            thresholds: String::new(),
            color_mode: COLOR_MODE.into(),
            graph: true,
            location: String::new(),
            size_min: String::new(),
            size_max: String::new(),
        }
    }
}

const COLOR_MODE: &str = "value";

/// Thresholds as the editor writes them: the base's color, then each
/// step's value and color, "green, 80 red".
pub fn thresholds_text(steps: &[Threshold]) -> String {
    steps
        .iter()
        .map(|step| match step.value {
            Some(value) => format!("{} {}", number_text(value), step.color),
            None => step.color.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Thresholds from how the editor writes them; None when they don't read
/// so.
pub fn read_thresholds(text: &str) -> Option<Vec<Threshold>> {
    if text.trim().is_empty() {
        return Some(Vec::new());
    }
    text.split(',')
        .enumerate()
        .map(|(index, step)| {
            let words: Vec<String> = step.split_whitespace().map(str::to_lowercase).collect();
            match (index, words.as_slice()) {
                (0, [color]) => Some(Threshold { color: color.clone(), value: None }),
                (1.., [value, color]) => {
                    let value = value.parse::<f64>().ok().filter(|value| value.is_finite())?;
                    Some(Threshold { color: color.clone(), value: Some(value) })
                }
                _ => None,
            }
        })
        .collect()
}

/// The renderer's defaults (Grafana's), which a panel leaves unset.
const LINE_WIDTH: f64 = 1.0;
const FILL: f64 = 0.0;
const POINTS_AUTO: &str = "auto";

/// A number as a field shows it: "2", "0.5".
pub fn number_text(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9e15 { format!("{}", value as i64) } else { value.to_string() }
}

impl Draft {
    pub fn of(panel: &Panel) -> Self {
        let viz = &panel.viz;
        let legacy = legacy_thresholds();
        let optional = |value: Option<f64>| value.map(number_text).unwrap_or_default();
        Self {
            title: panel.title.clone(),
            source: panel.query.source.clone(),
            sql: panel.query.sql.clone(),
            kind: viz.kind.clone(),
            x: viz.x.clone().unwrap_or_default(),
            series_by: viz.series_by.clone().unwrap_or_default(),
            unit: viz.unit.clone(),
            width: panel.grid.w,
            height: panel.grid.h,
            line_width: number_text(viz.line_width.unwrap_or(LINE_WIDTH)),
            fill: number_text(viz.fill.unwrap_or(FILL)),
            points: viz.points.clone().unwrap_or_else(|| POINTS_AUTO.into()),
            legend: viz.legend.unwrap_or(true),
            min: optional(viz.min),
            max: optional(viz.max),
            decimals: viz.decimals.map(|decimals| decimals.to_string()).unwrap_or_default(),
            // Unset is what stats showed before there was a choice.
            reduce: shown_reduce(viz),
            // A stat from before keeps its one color once edited.
            thresholds: thresholds_text(if legacy_stat(viz) { &legacy } else { &viz.thresholds }),
            color_mode: viz.color_mode.clone().unwrap_or_else(|| COLOR_MODE.into()),
            graph: viz.graph.as_deref() != Some("none"),
            location: viz.location.clone().unwrap_or_default(),
            size_min: optional(viz.size_min),
            size_max: optional(viz.size_max),
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
        let or = |key: &str, default: String| Some(text(key).trim().to_owned()).filter(|text| !text.is_empty()).unwrap_or(default);
        let defaults = Self::default();
        Self {
            title: text("title").trim().to_owned(),
            source: text("source"),
            sql: text("sql"),
            kind: text("kind"),
            x: text("x").trim().to_owned(),
            series_by: text("series_by").trim().to_owned(),
            unit: text("unit").trim().to_owned(),
            width: number("width", COLUMNS),
            height: number("height", model::HEIGHT),
            line_width: or("line_width", defaults.line_width),
            fill: or("fill", defaults.fill),
            points: or("points", defaults.points),
            legend: context["legend"].as_bool().unwrap_or(true),
            min: text("min").trim().to_owned(),
            max: text("max").trim().to_owned(),
            decimals: text("decimals").trim().to_owned(),
            reduce: or("reduce", defaults.reduce),
            thresholds: text("thresholds").trim().to_owned(),
            color_mode: or("color_mode", defaults.color_mode),
            graph: context["graph"].as_bool().unwrap_or(true),
            location: text("location").trim().to_owned(),
            size_min: text("size_min").trim().to_owned(),
            size_max: text("size_max").trim().to_owned(),
        }
    }

    /// The editor's data model.
    pub fn data(&self) -> Value {
        json!({
            "title": self.title, "source": [self.source], "sql": self.sql, "kind": [self.kind],
            "x": self.x, "series_by": self.series_by, "unit": self.unit,
            "width": [self.width.to_string()], "height": [self.height.to_string()],
            "line_width": [self.line_width], "fill": [self.fill], "points": [self.points], "legend": self.legend,
            "min": self.min, "max": self.max, "decimals": self.decimals,
            "reduce": [self.reduce], "thresholds": self.thresholds, "color_mode": [self.color_mode], "graph": self.graph,
            "location": self.location, "size_min": self.size_min, "size_max": self.size_max,
        })
    }

    /// Why its style fields can't be kept, as the editor says it.
    pub fn style_problem(&self) -> Option<String> {
        let number = |text: &str| text.is_empty() || text.parse::<f64>().is_ok_and(f64::is_finite);
        if !number(&self.min) || !number(&self.max) {
            return Some("Enter numbers for min and max, or leave them empty to fit the data.".into());
        }
        let size = |text: &str| text.is_empty() || text.parse::<f64>().is_ok_and(|size| (1.0..=40.0).contains(&size));
        if !size(&self.size_min) || !size(&self.size_max) {
            return Some("Enter dot sizes from 1 to 40, or leave them empty.".into());
        }
        if !self.decimals.is_empty() && !self.decimals.parse::<u32>().is_ok_and(|decimals| decimals <= 10) {
            return Some("Enter a whole number from 0 to 10 for decimals, or leave it empty.".into());
        }
        let Some(steps) = read_thresholds(&self.thresholds) else {
            return Some("Enter a color, then a value and a color for each step, such as \u{201c}green, 80 red\u{201d}.".into());
        };
        if let Some(step) = steps.iter().find(|step| !model::COLOR_NAMES.contains(&step.color.as_str())) {
            return Some(format!("\u{201c}{}\u{201d} isn't a color here. Use {}.", step.color, model::COLOR_NAMES.join(", ")));
        }
        if steps.windows(2).skip(1).any(|pair| pair[0].value >= pair[1].value) {
            return Some("Make each threshold value higher than the one before.".into());
        }
        if let Some(why) = model::thresholds_problem(&steps) {
            return Some(format!("Thresholds: {why}."));
        }
        let viz = self.viz(None);
        if let (Some(min), Some(max)) = (viz.min, viz.max)
            && min >= max
        {
            return Some("Make min lower than max.".into());
        }
        if let (Some(low), Some(high)) = (viz.size_min, viz.size_max)
            && low > high
        {
            return Some("Make the smallest dot no bigger than the largest.".into());
        }
        viz.style_problems().first().map(|(field, why)| format!("{field}: {why}."))
    }

    /// How the panel it makes shows (keeping what the editor doesn't show
    /// from `old`). What's at the renderer's default stays unset.
    pub fn viz(&self, old: Option<&Viz>) -> Viz {
        let optional = |text: &str| (!text.is_empty()).then(|| text.to_owned());
        let number = |text: &str| text.parse::<f64>().ok().filter(|value| value.is_finite());
        let unless = |value: Option<f64>, default: f64| value.filter(|value| *value != default);
        let kept = old.cloned().unwrap_or_default();
        Viz {
            kind: self.kind.clone(),
            unit: self.unit.clone(),
            x: optional(&self.x),
            series_by: optional(&self.series_by),
            y: if self.series_by.is_empty() || kept.y.len() <= 1 { kept.y } else { Vec::new() },
            color: kept.color,
            columns: kept.columns,
            line_width: unless(number(&self.line_width), LINE_WIDTH),
            fill: unless(number(&self.fill), FILL),
            points: optional(&self.points).filter(|points| points != POINTS_AUTO),
            point_size: kept.point_size,
            min: number(&self.min),
            max: number(&self.max),
            decimals: self.decimals.parse().ok(),
            legend: (!self.legend).then_some(false),
            // A stat's always says, so a new one's last stays last.
            // Kept as the person left it, as the other hidden settings are
            // (Abbacchio row 1050): changed while it was a number, then
            // saved as something else, it's what they chose.
            reduce: if self.kind == "stat" || old.is_some_and(|old| shown_reduce(old) != self.reduce) || (old.is_none() && self.reduce != model::NEW_REDUCE) {
                Some(self.reduce.clone())
            } else {
                kept.reduce
            },
            thresholds: read_thresholds(&self.thresholds).unwrap_or(kept.thresholds),
            color_mode: Some(self.color_mode.clone()).filter(|mode| !mode.is_empty() && mode != COLOR_MODE),
            graph: (!self.graph).then(|| "none".into()),
            location: optional(&self.location),
            size_min: number(&self.size_min),
            size_max: number(&self.size_max),
        }
    }

    /// The panel it makes, as `id` at `grid` (keeping what the editor
    /// doesn't show from `old`).
    pub fn panel(&self, id: &str, grid: Grid, old: Option<&Panel>) -> Panel {
        Panel {
            id: id.to_owned(),
            title: self.title.clone(),
            grid: Grid { w: self.width, h: self.height, x: grid.x.min(COLUMNS.saturating_sub(self.width)), ..grid },
            query: Query { source: self.source.clone(), sql: self.sql.clone() },
            viz: self.viz(old.map(|panel| &panel.viz)),
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
    /// The range it ran for, in milliseconds: where a time axis starts and
    /// ends.
    pub span: Option<(i64, i64)>,
    /// When it ran.
    pub now: i64,
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
    let asked = crate::source_for(&query.source, sources);
    let Some(source) = sources.iter().find(|source| source.id == asked) else {
        return Outcome::Failed(format!("Charts doesn't read \"{}\".", query.source));
    };
    if !source.granted {
        return Outcome::NotGranted(source.label.clone());
    }
    let bound = match sql::bind(&query.sql, values) {
        Ok(bound) => bound,
        Err(why) => return Outcome::Failed(why),
    };
    match host.data_query(asked, &bound.sql, &Value::Array(bound.params)) {
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
    let span = values["from"].as_i64().zip(values["to"].as_i64());
    let nothing_granted = !sources.iter().any(|source| source.granted);
    if nothing_granted {
        return Ran { nothing_granted, waiting: true, span, now, ..Ran::default() };
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
    Ran { panels, options, nothing_granted, waiting, span, now }
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
    /// The dashboards as their menu lists them: (id, name).
    pub boards: &'a [(String, String)],
    /// Their ids in the person's order.
    pub ids: &'a [String],
    pub prefs: &'a model::Prefs,
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

/// The icons Charts' menus use (Lucide names).
pub const STAR_ICON: &str = "star";
pub const HOME_ICON: &str = "house";

/// One of a menu's options: its words, the action choosing it sends, and
/// how it shows (chosen; under a section's heading; an icon after it).
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub words: String,
    pub action: &'static str,
    pub context: Value,
    pub chosen: bool,
    pub section: Option<&'static str>,
    pub icon: Option<&'static str>,
}

impl Choice {
    fn new(words: impl Into<String>, action: &'static str, context: Value, chosen: bool) -> Self {
        Self { words: words.into(), action, context, chosen, section: None, icon: None }
    }

    fn option(&self) -> Value {
        let mut option = json!({"label": self.words, "action": {"event": {"name": self.action, "context": self.context}}});
        if self.chosen {
            option["selected"] = json!(true);
        }
        if let Some(section) = self.section {
            option["section"] = json!(section);
        }
        if let Some(icon) = self.icon {
            option["icon"] = json!(icon);
        }
        option
    }
}

/// A button that opens a menu (`which`) over the page, `choices`, titled
/// `title`: its words (`<id>_t`, after `icon` if any) and a chevron. An
/// app without menus sends its action instead, and Charts opens the same
/// menu inline (`menu`).
fn menu_button(out: &mut Vec<Value>, id: &str, words: Value, which: &str, title: &str, choices: &[Choice], icon: Option<&str>) {
    let mut menu = json!({"options": choices.iter().map(Choice::option).collect::<Vec<_>>()});
    if !title.is_empty() {
        menu["title"] = json!(title);
    }
    out.push(json!({
        "id": id, "component": "Button", "child": format!("{id}_c"), "variant": "borderless",
        "action": {"event": {"name": "menu", "context": {"which": which}}}, "menu": menu,
    }));
    let mut parts = Vec::new();
    if let Some(icon) = icon {
        parts.push(format!("{id}_l"));
        out.push(json!({"id": format!("{id}_l"), "component": "Icon", "name": icon}));
    }
    parts.extend([format!("{id}_t"), format!("{id}_i")]);
    out.push(json!({"id": format!("{id}_c"), "component": "List", "direction": "horizontal", "align": "center", "children": parts}));
    out.push(text(&format!("{id}_t"), words, "body"));
    out.push(json!({"id": format!("{id}_i"), "component": "Icon", "name": "chevron-down"}));
}

/// A menu opened inline, for an app without menus over the page: its
/// options in a card under the bar, rows, as Grafana's picker has them,
/// each a Button over a List of its words alone, which a renderer draws as
/// a full-width row, the chosen one ("primary") tinted with a bar on its
/// left.
fn menu(out: &mut Vec<Value>, children: &mut Vec<String>, choices: Vec<Choice>) {
    let mut ids = Vec::new();
    for (index, choice) in choices.into_iter().enumerate() {
        let id = format!("menu_o{index}");
        // A starred dashboard's star, where icons don't show.
        let words = if choice.action == "board" && choice.icon == Some(STAR_ICON) { format!("\u{2605} {}", choice.words) } else { choice.words };
        out.push(json!({
            "id": id, "component": "Button", "child": format!("{id}_c"), "variant": if choice.chosen { "primary" } else { "borderless" },
            "action": {"event": {"name": choice.action, "context": choice.context}},
        }));
        out.push(json!({"id": format!("{id}_c"), "component": "List", "direction": "horizontal", "children": [format!("{id}_t")]}));
        out.push(text(&format!("{id}_t"), words, "body"));
        ids.push(id);
    }
    out.push(json!({"id": "menu", "component": "Card", "child": "menu_col"}));
    out.push(json!({"id": "menu_col", "component": "Column", "children": ids}));
    children.push("menu".into());
}

/// The dashboards: the starred ones, then all the others, each in the
/// person's order, the home one marked.
fn board_choices(scene: &Scene, dashboard: Option<&Dashboard>) -> Vec<Choice> {
    scene
        .boards
        .iter()
        .map(|(id, name)| {
            let starred = scene.prefs.starred.contains(id);
            let home = scene.prefs.home.as_deref() == Some(id.as_str());
            Choice {
                section: Some(if starred { "Starred" } else { "All dashboards" }),
                icon: if starred { Some(STAR_ICON) } else if home { Some(HOME_ICON) } else { None },
                ..Choice::new(name.clone(), "board", json!({"id": id}), dashboard.is_some_and(|d| d.id == *id))
            }
        })
        .collect()
}

/// The ranges: the dashboard's own first, when it isn't a quick one (""
/// goes back to it), then Grafana's quick ranges.
fn range_choices(view: &View, dashboard: &Dashboard) -> Vec<Choice> {
    let (from, to) = range(view, Some(dashboard));
    let own = &dashboard.time;
    let mut choices = Vec::new();
    if own.to != "now" || !RANGES.contains(&own.from.as_str()) {
        choices.push(Choice::new(shown_range(&own.from, &own.to), "range", json!({"from": ""}), view.range.is_none()));
    }
    choices.extend(RANGES.iter().map(|range| Choice::new(range_label(range), "range", json!({"from": range}), to == "now" && *range == from)));
    choices
}

/// A variable's values, All first, and the one chosen's words.
fn variable_choices(view: &View, scene: &Scene, index: usize, variable: &model::Variable) -> (String, Vec<Choice>) {
    let chosen = view.values.get(&variable.name).unwrap_or(&variable.default);
    let all: Vec<(String, String)> =
        std::iter::once((String::new(), "All".to_owned())).chain(scene.ran.options.get(index).cloned().unwrap_or_default()).collect();
    let shown = all.iter().find(|(value, _)| value == chosen).map_or(chosen.clone(), |(_, shown)| shown.clone());
    let choices = all
        .into_iter()
        .map(|(value, words)| {
            let is = value == *chosen;
            Choice::new(words, "variable", json!({"name": variable.name, "value": value}), is)
        })
        .collect();
    (shown, choices)
}

/// \u{2022}\u{2022}\u{2022}: editing, the star, home and the dashboard's place in the menu.
fn more_choices(view: &View, scene: &Scene, dashboard: &Dashboard, granted: bool) -> Vec<Choice> {
    let mut choices = Vec::new();
    if granted {
        let (words, on) = if view.editing { ("Done editing", false) } else { ("Edit", true) };
        choices.push(Choice::new(words, "edit_mode", json!({"on": on}), false));
    }
    let starred = scene.prefs.starred.contains(&dashboard.id);
    // The filled star once starred, as Grafana's toggle; none before
    // (Scolippi 27596: a star on "Star" read as starred already).
    choices.push(Choice {
        icon: starred.then_some(STAR_ICON),
        ..Choice::new(if starred { "Unstar" } else { "Star" }, "star_board", json!({"on": !starred}), false)
    });
    if scene.prefs.home.as_deref() != Some(dashboard.id.as_str()) {
        choices.push(Choice { icon: Some(HOME_ICON), ..Choice::new("Make home dashboard", "home_board", json!({}), false) });
    }
    if scene.boards.len() > 1 {
        let (up, down) = model::can_move(scene.ids, scene.prefs, &dashboard.id);
        if up {
            choices.push(Choice::new("Move up", "move_board", json!({"up": true}), false));
        }
        if down {
            choices.push(Choice::new("Move down", "move_board", json!({"up": false}), false));
        }
    }
    choices
}

/// The range on the bar, short for a phone: "7 days", or
/// a fixed one's `short_range`.
pub fn bar_range(from: &str, to: &str, now: i64) -> String {
    let words = short_range(from, to, now);
    match words.strip_prefix("Last ") {
        Some(rest) if to == "now" => rest.to_owned(),
        _ => words,
    }
}

/// How a range reads: "Last 7 days".
pub fn range_label(from: &str) -> String {
    let Some(ago) = from.strip_prefix("now-") else { return from.parse::<i64>().map_or_else(|_| from.to_owned(), date_time) };
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
        ("y", true) => "year",
        ("y", false) => "years",
        _ => return from.to_owned(),
    };
    format!("Last {count} {unit}")
}

/// A range as the bar shows it: "Last 7 days", or its ends ("2026-11-23
/// 08:00:00 to now"), in UTC when one is a fixed time, since an app can't
/// know the device's time zone.
pub fn shown_range(from: &str, to: &str) -> String {
    if to == "now" && from.starts_with("now-") {
        return range_label(from);
    }
    let fixed = [from, to].iter().any(|end| end.parse::<i64>().is_ok());
    format!("{} to {}{}", range_label(from), range_label(to), if fixed { " UTC" } else { "" })
}

/// A range short enough for the bar on a phone, with its chevron: "Last 7
/// days", or a fixed one as "23 Nov 08:00 – 14:00 UTC" (the second date
/// only when it's another day, the year only when it isn't `now`'s, seconds
/// only when there are some). The menu has it whole (`shown_range`).
pub fn short_range(from: &str, to: &str, now: i64) -> String {
    let (start, end) = (from.parse::<i64>().ok(), to.parse::<i64>().ok());
    if start.is_none() && end.is_none() {
        return shown_range(from, to);
    }
    let this_year = civil(now).0;
    let day = |ms: i64| {
        let (year, month, day, ..) = civil(ms);
        let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(month - 1) as usize];
        if year == this_year { format!("{day} {month}") } else { format!("{day} {month} {year}") }
    };
    let time = |ms: i64| {
        let (.., hour, minute, second) = civil(ms);
        if second == 0 { format!("{hour:02}:{minute:02}") } else { format!("{hour:02}:{minute:02}:{second:02}") }
    };
    let first = match start {
        Some(ms) => format!("{} {}", day(ms), time(ms)),
        None => range_label(from),
    };
    let last = match (start, end) {
        (Some(a), Some(b)) if a.div_euclid(86_400_000) == b.div_euclid(86_400_000) => time(b),
        (_, Some(ms)) => format!("{} {}", day(ms), time(ms)),
        (_, None) => range_label(to),
    };
    format!("{first} \u{2013} {last} UTC")
}

/// Milliseconds since 1970 as a UTC date and time, "2026-11-23 08:00:00".
fn date_time(ms: i64) -> String {
    let (year, month, day, hour, minute, second) = civil(ms);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
}

/// Milliseconds since 1970 in UTC: (year, month, day, hour, minute, second).
fn civil(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let (days, ms) = (ms.div_euclid(86_400_000), ms.rem_euclid(86_400_000));
    // Days since 1970 to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    let s = ms / 1000;
    (year, month, day, s / 3600, s / 60 % 60, s % 60)
}

/// What the editor's Number shows starts at for `viz`: what it says; a
/// stat kept before reduce existed, its total; anything else, as a new
/// number, the last.
fn shown_reduce(viz: &Viz) -> String {
    viz.reduce.clone().unwrap_or_else(|| if viz.kind == "stat" { "sum".into() } else { model::NEW_REDUCE.into() })
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

    // The bar, on one line as on a phone (Scolippi 27155): the
    // dashboard's name, the range, the variables (past one, folded into
    // Filters) and \u{2022}\u{2022}\u{2022}, each a menu over the page. An app without
    // menus sends the button's action instead, and the menu opens inline
    // under the bar.
    let granted = dashboard.is_some() && !scene.ran.nothing_granted;
    let mut bar = Vec::new();
    let mut ids = vec!["name".to_owned()];
    let mut inline: Option<Vec<Choice>> = None;
    let mut open = |which: &str, choices: &[Choice]| {
        if view.menu.as_deref() == Some(which) {
            inline = Some(choices.to_vec());
        }
    };
    let name = dashboard.map_or("Charts", |d| d.name.as_str());
    // A menu even of one dashboard, so every part of the bar is a button
    // (Luca 27397); the name takes what the others leave, ending in "…".
    if !scene.boards.is_empty() {
        let choices = board_choices(scene, dashboard);
        menu_button(&mut bar, "name", json!(name), "board", "Dashboards", &choices, None);
        bar.iter_mut().find(|component| component["id"] == "name_t").expect("its words")["variant"] = json!("heading");
        bar.iter_mut().find(|component| component["id"] == "name").expect("its button")["weight"] = json!(1);
        open("board", &choices);
    } else {
        bar.push(json!({"id": "name", "component": "Text", "text": name, "variant": "heading"}));
    }
    let mut filters = Vec::new();
    if let Some(dashboard) = dashboard.filter(|_| granted) {
        let (from, to) = range(view, Some(dashboard));
        let choices = range_choices(view, dashboard);
        // No clock: "7 days" reads as a range, and a phone's bar needs the
        // room (Scolippi 27596).
        menu_button(&mut bar, "range", json!(bar_range(&from, &to, scene.ran.now)), "range", "Time range", &choices, None);
        ids.push("range".into());
        open("range", &choices);
        let folded = dashboard.variables.len() > 1;
        for (index, variable) in dashboard.variables.iter().enumerate() {
            let id = format!("var{index}");
            let (shown, choices) = variable_choices(view, scene, index, variable);
            let label = if variable.label.is_empty() { &variable.name } else { &variable.label };
            menu_button(if folded { &mut filters } else { &mut bar }, &id, json!(format!("{label}: {shown}")), &id, label, &choices, None);
            if !folded {
                ids.push(id.clone());
            }
            open(&id, &choices);
        }
        if folded {
            let count = dashboard.variables.len();
            // Its words and a chevron, as the menus beside it, though it
            // opens their line rather than a menu.
            bar.push(json!({
                "id": "filters", "component": "Button", "child": "filters_c", "variant": "borderless",
                "action": {"event": {"name": "menu", "context": {"which": "filters"}}},
            }));
            bar.push(json!({"id": "filters_c", "component": "List", "direction": "horizontal", "align": "center", "children": ["filters_t", "filters_i"]}));
            bar.push(text("filters_t", format!("Filters ({count})"), "body"));
            bar.push(json!({"id": "filters_i", "component": "Icon", "name": "chevron-down"}));
            ids.push("filters".into());
        }
    }
    if let Some(dashboard) = dashboard {
        let choices = more_choices(view, scene, dashboard, granted);
        let mut more = Vec::new();
        // Titled with the dashboard's name: what its choices apply to.
        menu_button(&mut more, "more", json!(""), "more", &dashboard.name, &choices, None);
        // An ellipsis alone, no words or chevron.
        more.retain(|component| component["id"] != "more_t" && component["id"] != "more_i");
        more.iter_mut().find(|component| component["id"] == "more_c").expect("its child")["children"] = json!(["more_l"]);
        more.push(json!({"id": "more_l", "component": "Icon", "name": "ellipsis"}));
        bar.extend(more);
        ids.push("more".into());
        open("more", &choices);
    }
    // One line, never stacked (no stackBelow): Luca draws a Row of buttons
    // as a bar.
    bar.insert(0, json!({"id": "bar", "component": "Row", "align": "center", "children": ids}));
    add(&mut out, &mut children, "bar", bar);
    // The variables, folded: on a line of their own while Filters is open,
    // or one of them.
    let filters_open = view.menu.as_deref().is_some_and(|which| which == "filters" || which.starts_with("var"));
    if !filters.is_empty() && filters_open {
        let ids: Vec<Value> = filters.iter().filter(|c| c["component"] == "Button").map(|c| c["id"].clone()).collect();
        filters.insert(0, json!({"id": "filters_row", "component": "List", "direction": "horizontal", "align": "center", "children": ids}));
        add(&mut out, &mut children, "filters_row", filters);
    }
    if let Some(choices) = inline {
        menu(&mut out, &mut children, choices);
    }

    let Some(dashboard) = dashboard else {
        let mut built = vec![json!({
            "id": "empty", "component": "EmptyState", "icon": "chart-column",
            "title": "No dashboards yet", "body": "Tell Rivet what you'd like to chart, and it makes one for you.",
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
                "title": "Charts has nothing to show yet",
                "body": "Tell Rivet what you'd like to see, and it sets Charts up for you. It asks you first before Charts reads anything private, such as conversations.",
            })],
        );
        out.insert(0, json!({"id": "root", "component": "Column", "children": children}));
        return out;
    }

    // Changing it.
    if view.editing {
        let mut built = Vec::new();
        let mut ids = Vec::new();
        let mut tool = |built: &mut Vec<Value>, id: &str, label: &str, name: &str, context: Value| {
            button(built, id, label, name, context, "default");
            ids.push(id.to_owned());
        };
        tool(&mut built, "done", "Done", "edit_mode", json!({"on": false}));
        tool(&mut built, "add", "Add panel", "add_panel", json!({}));
        tool(&mut built, "rename", "Rename", "rename_board", json!({}));
        tool(&mut built, "new", "New dashboard", "new_board", json!({}));
        if scene.has_previous {
            tool(&mut built, "restore", "Restore previous version", "restore", json!({}));
        }
        tool(&mut built, "remove", "Delete dashboard", "delete_board", json!({}));
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
        out.extend(editor_components(editor, scene.sources, view.menu.as_deref() == Some("kind")));
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
        // Below a tablet's width, one panel under another, full width.
        built.insert(0, json!({"id": id, "component": "Row", "align": "start", "children": cells, "stackBelow": STACK_BELOW}));
        add(&mut out, &mut children, &id, built);
    }
    out.insert(0, json!({"id": "root", "component": "Column", "children": children}));
    // A time axis spans the range shown, as Grafana's does, rows or not.
    if let Some((from, to)) = scene.ran.span.filter(|(from, _)| *from >= 0) {
        for chart in out.iter_mut().filter(|component| component["component"] == "Chart" && component["kind"] == "timeseries") {
            chart["timeFrom"] = json!(from);
            chart["timeTo"] = json!(to);
        }
    }
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

/// A stat as kept before 0.5.0, untouched since: its total (no reduce) in
/// one color. It keeps one color, green, rather than the renderer's
/// default steps (red from 80), which would turn its totals red.
pub fn legacy_stat(viz: &Viz) -> bool {
    viz.kind == "stat" && viz.reduce.is_none() && viz.thresholds.is_empty() && viz.color.is_none()
}

fn legacy_thresholds() -> Vec<Threshold> {
    vec![Threshold { color: "green".into(), value: None }]
}

/// A stat's height in points: its panel's, so its number fills it, and at
/// least 140, Grafana's default stat's, below which its number would be too
/// small to read beside its spark line (the default dashboard's stats are 3
/// rows, 106).
fn stat_height(grid: Grid) -> u32 {
    model::points(grid.h).clamp(STAT_HEIGHT, 480)
}

const STAT_HEIGHT: u32 = 140;

/// A panel's height in a chart's points.
fn height(grid: Grid) -> u32 {
    model::points(grid.h).clamp(120, 480)
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
    kind == "stat" || kind == "table" || kind == "map" || CHART_KINDS.contains(&kind)
}

/// A panel's body as `id`, bound to its data at `at`: its Stat, Chart or
/// Table, or why it can't show.
pub fn panel_body(id: &str, at: &str, title: &str, viz: &Viz, grid: Grid, outcome: &Outcome) -> Vec<Value> {
    let path = |part: &str| json!({"@path": format!("{at}/{part}")});
    let titled = |second: Value| {
        let second_id = second["id"].as_str().unwrap_or_default().to_owned();
        vec![
            json!({"id": id, "component": "Column", "children": [format!("{id}_title"), second_id]}),
            // Drawn as every panel's title is, as Grafana's (T6).
            text(&format!("{id}_title"), title, "title"),
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
            let mut stat = json!({"id": id, "component": "Stat", "label": title, "value": path("value"), "spark": path("spark"), "height": stat_height(grid)});
            // One fixed color when it says so; else its thresholds'.
            if let Some(color) = viz.color.as_deref().filter(|color| *color != "palette" && *color != "agent") {
                stat["color"] = json!(color);
            }
            if !viz.thresholds.is_empty() {
                stat["thresholds"] = json!(viz.thresholds);
            } else if legacy_stat(viz) {
                stat["thresholds"] = json!(legacy_thresholds());
            }
            if let Some(mode) = &viz.color_mode {
                stat["colorMode"] = json!(mode);
            }
            if let Some(graph) = &viz.graph {
                stat["graph"] = json!(graph);
            }
            if let Some(decimals) = viz.decimals {
                stat["decimals"] = json!(decimals);
            }
            stat
        }
        "map" => {
            let mut map = json!({"id": id, "component": "Map", "data": path("data"), "label": title, "height": height(grid)});
            if let Some(location) = &viz.location {
                map["location"] = json!(location);
            }
            if let Some(value) = viz.y.first() {
                map["value"] = json!(value);
            }
            if let Some(color) = viz.color.as_deref().filter(|color| model::COLOR_NAMES.contains(color)) {
                map["color"] = json!(color);
            }
            if !viz.thresholds.is_empty() {
                map["thresholds"] = json!(viz.thresholds);
            }
            style(&mut map, viz);
            map
        }
        "table" => {
            // As tall as a chart of its height beside it, as Grafana's table
            // fills its panel: its rows scroll inside (Zucchero row 454).
            let mut table = json!({"id": format!("{id}_table"), "component": "Table", "data": path("data"), "height": height(grid)});
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
            style(&mut chart, viz);
            chart
        }
    };
    if !viz.unit.is_empty() {
        component["unit"] = json!(viz.unit);
    }
    vec![component]
}

/// How a Chart draws, the properties its panel sets (none for the
/// renderer's defaults, Grafana's).
fn style(chart: &mut Value, viz: &Viz) {
    let mut set = |key: &str, value: Option<Value>| {
        if let Some(value) = value {
            chart[key] = value;
        }
    };
    // Whole numbers as such: 2, not 2.0.
    let number = |value: f64| if value.fract() == 0.0 && value.abs() < 9e15 { json!(value as i64) } else { json!(value) };
    set("lineWidth", viz.line_width.map(number));
    set("fill", viz.fill.map(number));
    set("points", viz.points.clone().map(Value::from));
    set("pointSize", viz.point_size.map(number));
    set("min", viz.min.map(number));
    set("max", viz.max.map(number));
    set("decimals", viz.decimals.map(Value::from));
    set("legend", viz.legend.map(Value::from));
    set("sizeMin", viz.size_min.map(number));
    set("sizeMax", viz.size_max.map(number));
}

/// A panel's data, where its body looks for it.
pub fn panel_data(viz: &Viz, outcome: &Outcome, rows: usize) -> Value {
    match outcome {
        Outcome::Failed(why) => json!({"error": format!("Couldn't run this panel's query: {why}")}),
        Outcome::NotGranted(label) => {
            json!({"error": format!("To see this panel, Charts needs to read {label}. Ask Rivet to set it up.")})
        }
        Outcome::Table(_) if !drawable(&viz.kind) => json!({"error": format!("Charts can't draw \"{}\".", viz.kind)}),
        Outcome::Table(table) if viz.kind == "stat" => stat(table, viz.reduce.as_deref().unwrap_or("sum")),
        Outcome::Table(table) if viz.kind == "table" => json!({"data": truncate(table, TABLE_ROWS.min(rows))}),
        // A sample would leave holes in a grid or a map: the first rows.
        Outcome::Table(table) if viz.kind == "heatmap" || viz.kind == "map" => json!({"data": truncate(table, rows)}),
        Outcome::Table(table) => json!({"data": downsample(table, rows, viz.series_by.as_deref())}),
    }
}

/// The data of all a dashboard's panels, each held to its share of rows.
pub fn panels_data(dashboard: &Dashboard, ran: &Ran) -> Value {
    let share = (VIEW_ROWS / dashboard.panels.len().max(1)).clamp(TABLE_ROWS, CHART_ROWS);
    Value::Array(dashboard.panels.iter().zip(&ran.panels).map(|(panel, outcome)| panel_data(&panel.viz, outcome, share)).collect())
}

/// A Stat's value and spark line from a query's answer: the last column's
/// numbers, in order, and what `reduce` makes of them (`model::REDUCERS`):
/// a total of none is 0, the others of none null.
pub fn stat(table: &Value, reduce: &str) -> Value {
    let rows = table["rows"].as_array().cloned().unwrap_or_default();
    let spark: Vec<f64> = rows.iter().filter_map(|row| row.as_array()?.last()?.as_f64()).collect();
    let numbers = spark.iter().copied();
    let value = match reduce {
        "last" => spark.last().copied(),
        "mean" => (!spark.is_empty()).then(|| spark.iter().sum::<f64>() / spark.len() as f64),
        "max" => numbers.reduce(f64::max),
        "min" => numbers.reduce(f64::min),
        _ => Some(spark.iter().sum()),
    };
    let value = match value {
        Some(value) if value.fract() == 0.0 && value.abs() < 9e15 => json!(value as i64),
        Some(value) => json!(value),
        None => Value::Null,
    };
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

const KIND_LABELS: [(&str, &str); 9] = [
    ("stat", "Number"),
    ("timeseries", "Line"),
    ("bars", "Bars"),
    ("stackedBars", "Stacked bars"),
    ("pie", "Pie"),
    ("scatter", "Scatter"),
    ("heatmap", "Heatmap"),
    ("map", "World map"),
    ("table", "Table"),
];
const WIDTHS: [(u32, &str); 5] = [(3, "Quarter"), (4, "Third"), (6, "Half"), (8, "Two thirds"), (12, "Full")];
/// In Grafana's rows: 144, 296, 372 and 480 points.
const HEIGHTS: [(u32, &str); 4] = [(4, "Short"), (8, "Medium"), (10, "Tall"), (13, "Taller")];

const LINE_WIDTHS: [(&str, &str); 5] = [("0", "None"), ("1", "1"), ("2", "2"), ("3", "3"), ("5", "5")];
const FILLS: [(&str, &str); 5] = [("0", "None"), ("10", "10%"), ("25", "25%"), ("50", "50%"), ("100", "100%")];
const REDUCE_LABELS: [(&str, &str); 5] = [("last", "Last"), ("sum", "Total"), ("mean", "Mean"), ("max", "Max"), ("min", "Min")];
const COLOR_MODE_LABELS: [(&str, &str); 4] =
    [("value", "Value"), ("background", "Background gradient"), ("solid", "Background solid"), ("none", "None")];
const POINT_LABELS: [(&str, &str); 3] = [("auto", "Auto"), ("always", "Always"), ("never", "Never")];

/// Chips for `known`, and for `current` (as `current` and `unit`) when it
/// isn't one of them, such as an agent's choice.
fn choices(known: &[(&str, &str)], current: &str, unit: &str) -> Value {
    let mut options: Vec<Value> = known.iter().map(|(value, label)| json!({"label": label, "value": value})).collect();
    if !current.is_empty() && !known.iter().any(|(value, _)| *value == current) {
        options.push(json!({"label": format!("{current}{unit}"), "value": current}));
    }
    Value::Array(options)
}

fn sizes(known: &[(u32, &str)], current: u32, unit: &str) -> Value {
    let mut options: Vec<Value> = known.iter().map(|(value, label)| json!({"label": label, "value": value.to_string()})).collect();
    if !known.iter().any(|(value, _)| *value == current) {
        options.push(json!({"label": format!("{current} {unit}"), "value": current.to_string()}));
    }
    Value::Array(options)
}

/// The editor's fields, bound to `/editor`, its preview and its buttons.
pub fn editor_components(editor: &Editor, sources: &[DataSource], kind_open: bool) -> Vec<Value> {
    let draft = &editor.draft;
    // What Run and Save send: every field, shown or not.
    let context = json!({
        "title": {"@path": "/editor/title"}, "source": {"@path": "/editor/source"}, "sql": {"@path": "/editor/sql"},
        "kind": {"@path": "/editor/kind"}, "x": {"@path": "/editor/x"}, "series_by": {"@path": "/editor/series_by"},
        "unit": {"@path": "/editor/unit"}, "width": {"@path": "/editor/width"}, "height": {"@path": "/editor/height"},
        "line_width": {"@path": "/editor/line_width"}, "fill": {"@path": "/editor/fill"}, "points": {"@path": "/editor/points"},
        "min": {"@path": "/editor/min"}, "max": {"@path": "/editor/max"}, "decimals": {"@path": "/editor/decimals"},
        "legend": {"@path": "/editor/legend"}, "reduce": {"@path": "/editor/reduce"},
        "thresholds": {"@path": "/editor/thresholds"}, "color_mode": {"@path": "/editor/color_mode"}, "graph": {"@path": "/editor/graph"},
        "location": {"@path": "/editor/location"}, "size_min": {"@path": "/editor/size_min"}, "size_max": {"@path": "/editor/size_max"},
    });
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
        json!({"id": "ed_reduce", "component": "ChoicePicker", "label": "Number shows", "options": choices(&REDUCE_LABELS, &draft.reduce, ""), "value": {"@path": "/editor/reduce"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_thresholds", "component": "TextField", "label": "Thresholds", "value": {"@path": "/editor/thresholds"}, "placeholder": "green, 80 red"}),
        json!({"id": "ed_color_mode", "component": "ChoicePicker", "label": "Color mode", "options": choices(&COLOR_MODE_LABELS, &draft.color_mode, ""), "value": {"@path": "/editor/color_mode"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_graph", "component": "CheckBox", "label": "Sparkline", "value": {"@path": "/editor/graph"}}),
        json!({"id": "ed_columns", "component": "Row", "align": "start", "children": ["ed_x", "ed_series", "ed_unit"]}),
        json!({"id": "ed_x", "component": "TextField", "label": "Along the bottom", "value": {"@path": "/editor/x"}, "placeholder": "The first column", "weight": 1}),
        json!({"id": "ed_series", "component": "TextField", "label": "Split by", "value": {"@path": "/editor/series_by"}, "placeholder": "Nothing", "weight": 1}),
        json!({"id": "ed_unit", "component": "TextField", "label": "Unit", "value": {"@path": "/editor/unit"}, "placeholder": "None", "weight": 1}),
        json!({"id": "ed_width", "component": "ChoicePicker", "label": "Width", "options": sizes(&WIDTHS, draft.width, "columns"), "value": {"@path": "/editor/width"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_height", "component": "ChoicePicker", "label": "Height", "options": sizes(&HEIGHTS, draft.height, "units"), "value": {"@path": "/editor/height"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_line", "component": "ChoicePicker", "label": "Line width", "options": choices(&LINE_WIDTHS, &draft.line_width, ""), "value": {"@path": "/editor/line_width"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_fill", "component": "ChoicePicker", "label": "Fill", "options": choices(&FILLS, &draft.fill, "%"), "value": {"@path": "/editor/fill"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_points", "component": "ChoicePicker", "label": "Points", "options": choices(&POINT_LABELS, &draft.points, ""), "value": {"@path": "/editor/points"}, "variant": "mutuallyExclusive", "displayStyle": "chips"}),
        json!({"id": "ed_axis", "component": "Row", "align": "start", "children": ["ed_min", "ed_max", "ed_decimals"]}),
        json!({"id": "ed_min", "component": "TextField", "label": "Min", "value": {"@path": "/editor/min"}, "placeholder": "Auto", "variant": "number", "weight": 1}),
        json!({"id": "ed_max", "component": "TextField", "label": "Max", "value": {"@path": "/editor/max"}, "placeholder": "Auto", "variant": "number", "weight": 1}),
        json!({"id": "ed_decimals", "component": "TextField", "label": "Decimals", "value": {"@path": "/editor/decimals"}, "placeholder": "Auto", "variant": "number", "weight": 1}),
        json!({"id": "ed_legend", "component": "CheckBox", "label": "Legend", "value": {"@path": "/editor/legend"}}),
        json!({"id": "ed_location", "component": "TextField", "label": "Countries", "value": {"@path": "/editor/location"}, "placeholder": "The first text column", "weight": 1}),
        json!({"id": "ed_sizes", "component": "Row", "align": "start", "children": ["ed_size_min", "ed_size_max"]}),
        json!({"id": "ed_size_min", "component": "TextField", "label": "Smallest dot", "value": {"@path": "/editor/size_min"}, "placeholder": "2", "variant": "number", "weight": 1}),
        json!({"id": "ed_size_max", "component": "TextField", "label": "Largest dot", "value": {"@path": "/editor/size_max"}, "placeholder": "15", "variant": "number", "weight": 1}),
    ];
    // Show as: a menu, as Grafana's visualization picker; choosing one
    // draws the settings that kind has, and only those (Abbacchio, Bruno
    // 28309).
    let kinds: Vec<Choice> = KIND_LABELS
        .iter()
        .map(|(value, label)| {
            let mut context = context.clone();
            context["to"] = json!(value);
            Choice::new(*label, "set_kind", context, draft.kind == *value)
        })
        .collect();
    let shown_kind = KIND_LABELS.iter().find(|(value, _)| *value == draft.kind).map_or(draft.kind.as_str(), |(_, label)| label);
    menu_button(&mut out, "ed_kind", json!(format!("Show as: {shown_kind}")), "kind", "Show as", &kinds, None);
    let (before, after): (&[&str], &[&str]) = match draft.kind.as_str() {
        "stat" => (&["ed_reduce", "ed_thresholds", "ed_color_mode", "ed_graph", "ed_axis"], &[]),
        "timeseries" => (&[], &["ed_line", "ed_fill", "ed_points", "ed_axis", "ed_legend"]),
        "scatter" => (&[], &["ed_points", "ed_axis", "ed_legend"]),
        "bars" | "stackedBars" => (&[], &["ed_axis", "ed_legend"]),
        "pie" => (&[], &["ed_legend"]),
        "heatmap" => (&[], &["ed_axis", "ed_legend"]),
        "map" => (&[], &["ed_thresholds", "ed_sizes", "ed_axis", "ed_legend"]),
        _ => (&[], &[]),
    };
    // A number's, a grid's and a map's axis is its decimals alone.
    let decimals_only = matches!(draft.kind.as_str(), "stat" | "heatmap" | "map");
    if decimals_only {
        out.iter_mut().find(|component| component["id"] == "ed_axis").expect("the axis")["children"] = json!(["ed_decimals"]);
    }
    let find = |out: &mut Vec<Value>, id: &str| out.iter_mut().position(|component| component["id"] == id).expect(id);
    match draft.kind.as_str() {
        // A map is placed by its countries: no column along the bottom.
        "map" => {
            let at = find(&mut out, "ed_columns");
            out[at]["children"] = json!(["ed_location", "ed_unit"]);
            let at = find(&mut out, "ed_thresholds");
            out[at]["placeholder"] = json!("Green to red by value");
        }
        // A grid's rows are its series.
        "heatmap" => {
            let at = find(&mut out, "ed_series");
            out[at]["label"] = json!("Rows");
        }
        _ => {}
    }
    let mut col: Vec<String> = ["ed_heading", "ed_title", "ed_source", "ed_sql", "ed_help", "ed_kind"].map(String::from).to_vec();
    if kind_open {
        let mut inline = Vec::new();
        menu(&mut inline, &mut col, kinds);
        out.extend(inline);
    }
    col.extend(before.iter().map(|id| (*id).to_owned()));
    col.extend(["ed_columns", "ed_width", "ed_height"].map(String::from));
    col.extend(after.iter().map(|id| (*id).to_owned()));
    // What isn't shown isn't sent.
    let shown: Vec<&str> = before.iter().chain(after).copied().collect();
    let style = [
        "ed_reduce", "ed_thresholds", "ed_color_mode", "ed_graph", "ed_line", "ed_fill", "ed_points", "ed_axis", "ed_min", "ed_max", "ed_legend",
        "ed_sizes", "ed_size_min", "ed_size_max",
    ];
    out.retain(|component| {
        let id = component["id"].as_str().unwrap_or_default();
        let inside = match id {
            "ed_min" | "ed_max" => shown.contains(&"ed_axis") && !decimals_only,
            "ed_size_min" | "ed_size_max" => shown.contains(&"ed_sizes"),
            _ => false,
        };
        let map = draft.kind == "map";
        (!style.contains(&id) || shown.contains(&id) || inside) && (id != "ed_location" || map) && !(map && (id == "ed_x" || id == "ed_series"))
    });
    if let Some(preview) = &editor.preview {
        let viz = draft.viz(None);
        let grid = Grid { h: draft.height, ..Grid::default() };
        out.push(json!({"id": "ed_preview", "component": "Card", "child": "ed_preview_body"}));
        out.extend(panel_body("ed_preview_body", "/editor/preview", &draft.title, &viz, grid, preview));
        col.push("ed_preview".into());
    }
    if !editor.message.is_empty() {
        out.push(text("ed_message", editor.message.as_str(), "caption"));
        col.push("ed_message".into());
    }
    col.push("ed_buttons".into());
    out.push(json!({"id": "ed_col", "component": "Column", "children": col}));
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
            panel_data(&editor.draft.viz(None), outcome, CHART_ROWS)
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
    if let Some(why) = draft.style_problem() {
        return Some(why);
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
