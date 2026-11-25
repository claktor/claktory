//! Dashboards: their definition (what an agent's `charts_put` sends and
//! `charts_get` answers), how it's checked, the one Charts starts with, and
//! how they're kept in the app's store, each with the version before it.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::sql::{self, BUILTINS};
use crate::{ACTIVITY, DataSource, Host};

/// Where the list of dashboards is kept, in order.
pub const INDEX_KEY: &str = "dashboards";
/// C4a's one dashboard, given up for the list.
pub const LEGACY_KEY: &str = "dashboard/main";

pub const MIN_REFRESH_S: u64 = 10;
pub const MAX_REFRESH_S: u64 = 86_400;
pub const MAX_DASHBOARDS: usize = 20;
pub const MAX_PANELS: usize = 24;
pub const MAX_VARIABLES: usize = 4;
/// A grid row's columns.
pub const COLUMNS: u32 = 12;
/// The tallest a panel is, in Grafana's rows: 13 is 486 points, past the
/// most a chart draws (480).
pub const MAX_HEIGHT: u32 = 13;
/// What a kept dashboard's `_v` says: 1 since 0.5.0, its heights in
/// Grafana's rows; 0 (none) before, 50 points a unit.
pub const FORMAT: u32 = 1;

/// How a panel can show its answer.
pub const KINDS: [&str; 9] = ["stat", "timeseries", "bars", "stackedBars", "pie", "scatter", "heatmap", "map", "table"];
/// The Chart component's kinds, of those.
pub const CHART_KINDS: [&str; 6] = ["timeseries", "bars", "stackedBars", "pie", "scatter", "heatmap"];
const COLORS: [&str; 4] = ["palette", "agent", "signal", "muted"];
const FORMATS: [&str; 3] = ["text", "number", "time"];

/// A dashboard's definition.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Dashboard {
    pub id: String,
    pub name: String,
    /// The range it opens on.
    #[serde(default)]
    pub time: TimeRange,
    /// How often an open view runs its queries again.
    #[serde(default = "default_refresh")]
    pub refresh_s: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<Variable>,
    #[serde(default)]
    pub panels: Vec<Panel>,
    /// Its revision: one more at every save.
    #[serde(default, rename = "_rev")]
    pub rev: u64,
    /// Who saved it: an agent's id, or empty for the person.
    #[serde(default, rename = "_by", skip_serializing_if = "String::is_empty")]
    pub by: String,
    /// How it was kept (`FORMAT`); none before 0.5.0.
    #[serde(default, rename = "_v")]
    pub format: u32,
}

fn default_refresh() -> u64 {
    30
}

/// A range of time, each end "now", "now-<n><m|h|d|w|y>" or milliseconds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimeRange {
    pub from: String,
    #[serde(default = "now")]
    pub to: String,
}

fn now() -> String {
    "now".into()
}

impl Default for TimeRange {
    fn default() -> Self {
        Self { from: "now-7d".into(), to: now() }
    }
}

/// A variable: `$name` in the panels' queries, chosen in the view from its
/// query's answer (the first column the value, the second, if any, what
/// shows). "" means all, and is where it starts unless `default` says.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Variable {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,
    pub query: Query,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub default: String,
}

/// One panel: where it sits, its query and how its answer shows.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panel {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub grid: Grid,
    pub query: Query,
    pub viz: Viz,
}

/// A panel's place on a 12-column grid: rows by `y`, `x` and `w` in
/// columns, `h` its height in Grafana's rows (30 points each and 8 between:
/// 8 is 296 points).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grid {
    #[serde(default)]
    pub x: u32,
    #[serde(default)]
    pub y: u32,
    #[serde(default = "full")]
    pub w: u32,
    #[serde(default = "eight")]
    pub h: u32,
}

fn full() -> u32 {
    COLUMNS
}

/// Grafana's new panel's height.
pub const HEIGHT: u32 = 8;

fn eight() -> u32 {
    HEIGHT
}

impl Default for Grid {
    fn default() -> Self {
        Self { x: 0, y: 0, w: COLUMNS, h: HEIGHT }
    }
}

/// A grid's height in points: Grafana's rows, 30 points each and 8 between.
pub fn points(h: u32) -> u32 {
    (h * 38).saturating_sub(8)
}

/// One read-only statement against a source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Query {
    pub source: String,
    pub sql: String,
}

/// How a panel shows its answer.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Viz {
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub unit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub x: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub y: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// A table's columns, in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
    /// How a chart draws, each unset for the renderer's own default
    /// (Grafana's): the line's width in px (0–10, 0 for none; 1).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_width: Option<f64>,
    /// The area under a line, its opacity in percent (0–100; 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<f64>,
    /// Its points: auto (when far enough apart), always or never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub points: Option<String>,
    /// A point's diameter in px (1–40; 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub point_size: Option<f64>,
    /// The value axis's ends; unset, fitted to the data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    /// Decimals shown (0–10); unset, as many as the value needs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decimals: Option<u32>,
    /// The legend under a chart (shown unless false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub legend: Option<bool>,
    /// What a stat shows of its last column: last (the last number), sum,
    /// mean, max or min. Unset, the sum: what every stat showed before
    /// 0.5.0, so they keep it; new ones are given last.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reduce: Option<String>,
    /// A stat's colors by its value: the first step's (no value) up to the
    /// next one's value, and so on. Unset, green, red from 80.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub thresholds: Vec<Threshold>,
    /// What a stat's color colors: value, background, solid or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color_mode: Option<String>,
    /// A stat's spark line: area, or none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub graph: Option<String>,
    /// A map's column of countries (ISO codes); unset, its first text
    /// column. Its value is y's one column, else its last numeric one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    /// A map's dots' radius in px, by value (1–40; 2 and 15).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_max: Option<f64>,
}

/// One step of a stat's thresholds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Threshold {
    pub color: String,
    /// From where it starts; none for the first, the base.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<f64>,
}

/// The charts' colors by name, which the renderer shades for the theme;
/// "text" is the text's own.
pub const COLOR_NAMES: [&str; 14] = [
    "green",
    "yellow",
    "semi-dark-yellow",
    "blue",
    "orange",
    "red",
    "purple",
    "dark-green",
    "dark-yellow",
    "dark-blue",
    "dark-orange",
    "dark-red",
    "dark-purple",
    "text",
];
pub const COLOR_MODES: [&str; 4] = ["value", "background", "solid", "none"];
pub const GRAPHS: [&str; 2] = ["area", "none"];
pub const MAX_THRESHOLDS: usize = 10;

/// What a stat can show of its column, and what a new one shows.
pub const REDUCERS: [&str; 5] = ["last", "sum", "mean", "max", "min"];
pub const NEW_REDUCE: &str = "last";

/// What's wrong with a stat's thresholds, if anything.
pub fn thresholds_problem(steps: &[Threshold]) -> Option<String> {
    if steps.len() > MAX_THRESHOLDS {
        return Some(format!("at most {MAX_THRESHOLDS}"));
    }
    if let Some(step) = steps.iter().find(|step| !COLOR_NAMES.contains(&step.color.as_str())) {
        return Some(format!("\"{}\" isn't a color: use one of {}", step.color, COLOR_NAMES.join(", ")));
    }
    if steps.first().is_some_and(|base| base.value.is_some()) {
        return Some("the first is the base, with no value".into());
    }
    let values: Vec<Option<f64>> = steps.iter().skip(1).map(|step| step.value).collect();
    if values.iter().any(Option::is_none) {
        return Some("each after the first has a value".into());
    }
    if values.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Some("each value is above the one before".into());
    }
    None
}

/// A chart's points: when they show.
pub const POINTS: [&str; 3] = ["auto", "always", "never"];

impl Viz {
    /// What's wrong with how it draws, each as (its field, why).
    pub fn style_problems(&self) -> Vec<(&'static str, String)> {
        let mut problems = Vec::new();
        let within = |value: Option<f64>, low: f64, high: f64| value.is_some_and(|value| !(low..=high).contains(&value));
        if within(self.line_width, 0.0, 10.0) {
            problems.push(("line_width", "0 to 10".into()));
        }
        if within(self.fill, 0.0, 100.0) {
            problems.push(("fill", "0 to 100 (percent)".into()));
        }
        if self.points.as_deref().is_some_and(|points| !POINTS.contains(&points)) {
            problems.push(("points", format!("one of {}", POINTS.join(", "))));
        }
        if within(self.point_size, 1.0, 40.0) {
            problems.push(("point_size", "1 to 40".into()));
        }
        // serde_json reads no infinities or NaN: these are finite.
        if let (Some(min), Some(max)) = (self.min, self.max)
            && min >= max
        {
            problems.push(("min", "below max".into()));
        }
        if self.decimals.is_some_and(|decimals| decimals > 10) {
            problems.push(("decimals", "0 to 10".into()));
        }
        if self.reduce.as_deref().is_some_and(|reduce| !REDUCERS.contains(&reduce)) {
            problems.push(("reduce", format!("one of {}", REDUCERS.join(", "))));
        }
        if let Some(why) = thresholds_problem(&self.thresholds) {
            problems.push(("thresholds", why));
        }
        if self.color_mode.as_deref().is_some_and(|mode| !COLOR_MODES.contains(&mode)) {
            problems.push(("color_mode", format!("one of {}", COLOR_MODES.join(", "))));
        }
        if self.graph.as_deref().is_some_and(|graph| !GRAPHS.contains(&graph)) {
            problems.push(("graph", format!("one of {}", GRAPHS.join(", "))));
        }
        if within(self.size_min, 1.0, 40.0) {
            problems.push(("size_min", "1 to 40".into()));
        }
        if within(self.size_max, 1.0, 40.0) {
            problems.push(("size_max", "1 to 40".into()));
        }
        if let (Some(low), Some(high)) = (self.size_min, self.size_max)
            && low > high
        {
            problems.push(("size_min", "at most size_max".into()));
        }
        if self.location.as_deref().is_some_and(|location| location.is_empty() || location.chars().count() > 64) {
            problems.push(("location", "a column's name".into()));
        }
        problems
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Column {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
}

/// An id: lowercase letters, digits, - and _.
pub fn is_id(text: &str) -> bool {
    !text.is_empty() && text.len() <= 40 && text.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

fn is_name(text: &str) -> bool {
    let mut chars = text.chars();
    text.len() <= 32
        && chars.next().is_some_and(|c| c.is_ascii_lowercase() || c == '_')
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

fn words(text: &str, most: usize) -> bool {
    !text.trim().is_empty() && text.chars().count() <= most && !text.chars().any(char::is_control)
}

impl Dashboard {
    /// Everything wrong with it, each with where (`panels[2].viz.kind: …`);
    /// none when it can be kept. `sources` are those Charts asks for.
    pub fn problems(&self, sources: &[DataSource]) -> Vec<String> {
        let mut problems = Vec::new();
        let mut say = |at: &str, why: String| problems.push(if at.is_empty() { why } else { format!("{at}: {why}") });
        if !is_id(&self.id) {
            say("id", "use 1 to 40 lowercase letters, digits, - and _".into());
        }
        if !words(&self.name, 60) {
            say("name", "1 to 60 characters, on one line".into());
        }
        let now = 0x7fff_ffff_ffff;
        match (sql::time(&self.time.from, now), sql::time(&self.time.to, now)) {
            (Some(from), Some(to)) if from < to => {}
            (Some(_), Some(_)) => say("time", "from is before to".into()),
            _ => say("time", "each end is \"now\", \"now-<n><m|h|d|w|y>\" such as \"now-7d\", or milliseconds".into()),
        }
        if !(MIN_REFRESH_S..=MAX_REFRESH_S).contains(&self.refresh_s) {
            say("refresh_s", format!("{MIN_REFRESH_S} to {MAX_REFRESH_S} seconds"));
        }
        let source = |query: &Query| -> Option<String> {
            (!sources.iter().any(|known| known.id == query.source)).then(|| {
                let ids: Vec<&str> = sources.iter().map(|known| known.id.as_str()).collect();
                format!("\"{}\" isn't a source Charts reads; it reads {}", query.source, ids.join(", "))
            })
        };
        if self.variables.len() > MAX_VARIABLES {
            say("variables", format!("at most {MAX_VARIABLES}"));
        }
        let mut names = BTreeSet::new();
        for (index, variable) in self.variables.iter().enumerate() {
            let at = format!("variables[{index}]");
            if !is_name(&variable.name) || BUILTINS.contains(&variable.name.as_str()) {
                say(
                    &format!("{at}.name"),
                    format!("lowercase letters, digits and _, not starting with a digit, and none of {}", BUILTINS.join(", ")),
                );
            } else if !names.insert(variable.name.as_str()) {
                say(&format!("{at}.name"), format!("\"{}\" is there twice", variable.name));
            }
            if !variable.label.is_empty() && !words(&variable.label, 40) {
                say(&format!("{at}.label"), "at most 40 characters, on one line".into());
            }
            if let Some(why) = source(&variable.query) {
                say(&format!("{at}.query.source"), why);
            }
            if let Err(why) = sql::check(&variable.query.sql, &BUILTINS) {
                say(&format!("{at}.query.sql"), why);
            }
        }
        let known: Vec<&str> = BUILTINS.iter().copied().chain(self.variables.iter().map(|variable| variable.name.as_str())).collect();
        if self.panels.len() > MAX_PANELS {
            say("panels", format!("at most {MAX_PANELS}"));
        }
        let mut ids = BTreeSet::new();
        for (index, panel) in self.panels.iter().enumerate() {
            let at = format!("panels[{index}]");
            if !is_id(&panel.id) {
                say(&format!("{at}.id"), "use 1 to 40 lowercase letters, digits, - and _".into());
            } else if !ids.insert(panel.id.as_str()) {
                say(&format!("{at}.id"), format!("\"{}\" is there twice", panel.id));
            }
            if !words(&panel.title, 80) {
                say(&format!("{at}.title"), "1 to 80 characters, on one line".into());
            }
            let grid = panel.grid;
            if grid.w == 0 || grid.w > COLUMNS || grid.x + grid.w > COLUMNS {
                say(&format!("{at}.grid"), format!("w is 1 to {COLUMNS} columns, and x + w at most {COLUMNS}"));
            }
            if grid.h == 0 || grid.h > MAX_HEIGHT || grid.y > 100 {
                say(&format!("{at}.grid"), format!("h is 1 to {MAX_HEIGHT}, and y at most 100"));
            }
            if let Some(why) = source(&panel.query) {
                say(&format!("{at}.query.source"), why);
            }
            if let Err(why) = sql::check(&panel.query.sql, &known) {
                say(&format!("{at}.query.sql"), why);
            }
            let viz = &panel.viz;
            if !KINDS.contains(&viz.kind.as_str()) {
                say(&format!("{at}.viz.kind"), format!("one of {}", KINDS.join(", ")));
            }
            // A stat or a map takes one fixed color by name.
            let named = viz.kind == "stat" || viz.kind == "map";
            let stat_color = |color: &str| named && COLOR_NAMES.contains(&color);
            if viz.color.as_deref().is_some_and(|color| !COLORS.contains(&color) && !stat_color(color)) {
                let stat = if named { format!(", or for a {} {}", viz.kind, COLOR_NAMES.join(", ")) } else { String::new() };
                say(&format!("{at}.viz.color"), format!("one of {}{stat}", COLORS.join(", ")));
            }
            if viz.unit.chars().count() > 20 || viz.unit.chars().any(char::is_control) {
                say(&format!("{at}.viz.unit"), "at most 20 characters".into());
            }
            if viz.y.len() > 12 || (viz.series_by.is_some() && viz.y.len() > 1) {
                say(&format!("{at}.viz.y"), "at most 12 columns, and one with series_by".into());
            }
            if viz.columns.len() > 64 {
                say(&format!("{at}.viz.columns"), "at most 64".into());
            }
            for (field, why) in viz.style_problems() {
                say(&format!("{at}.viz.{field}"), why);
            }
            for (column_index, column) in viz.columns.iter().enumerate() {
                if column.format.as_deref().is_some_and(|format| !FORMATS.contains(&format)) {
                    say(&format!("{at}.viz.columns[{column_index}].format"), format!("one of {}", FORMATS.join(", ")));
                }
            }
        }
        problems.truncate(20);
        problems
    }

    pub fn panel(&self, id: &str) -> Option<&Panel> {
        self.panels.iter().find(|panel| panel.id == id)
    }

    /// The panels by row (`y`, then `x`), as they're drawn.
    pub fn rows(&self) -> Vec<Vec<&Panel>> {
        let mut panels: Vec<&Panel> = self.panels.iter().collect();
        panels.sort_by_key(|panel| (panel.grid.y, panel.grid.x));
        let mut rows: Vec<Vec<&Panel>> = Vec::new();
        for panel in panels {
            match rows.last_mut() {
                Some(row) if row[0].grid.y == panel.grid.y => row.push(panel),
                _ => rows.push(vec![panel]),
            }
        }
        rows
    }

    /// Where a new panel `w` wide goes: beside the last row's, if it fits,
    /// else under them.
    pub fn place(&self, w: u32, h: u32) -> Grid {
        let rows = self.rows();
        match rows.last() {
            None => Grid { x: 0, y: 0, w, h },
            Some(row) => {
                let end = row.iter().map(|panel| panel.grid.x + panel.grid.w).max().unwrap_or(0);
                let y = row[0].grid.y;
                if end + w <= COLUMNS { Grid { x: end, y, w, h } } else { Grid { x: 0, y: y + 1, w, h } }
            }
        }
    }

    /// An id for a new panel titled `title`, unlike the others.
    pub fn new_panel_id(&self, title: &str) -> String {
        let mut base: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_ascii_lowercase() || c.is_ascii_digit() { c } else { '_' })
            .collect::<String>()
            .split('_')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("_");
        base.truncate(32);
        if base.is_empty() {
            base = "panel".into();
        }
        let mut id = base.clone();
        let mut n = 2;
        while self.panel(&id).is_some() {
            id = format!("{base}_{n}");
            n += 1;
        }
        id
    }
}

/// Every bucket of the range, empty ones too, so spark lines are to scale.
const BUCKETS: &str =
    "WITH RECURSIVE t(at) AS (SELECT ($from / $step) * $step UNION ALL SELECT at + $step FROM t WHERE at + $step < $to)";

/// The bucket of an hour of agent_usage: its own, or the range's first when
/// the hour began before the range (a step under an hour), so tokens used
/// inside the range in that hour still count, as Tokens by agent counts
/// them.
const TOKENS_AT: &str = "MAX(((hour * 1000) / $step) * $step, ($from / $step) * $step)";

/// Before 0.2.1: an hour that began before the range fell outside every
/// bucket.
const TOKENS_AT_0_2_0: &str = "((hour * 1000) / $step) * $step";

/// The Tokens stat's query, each hour bucketed by `at`.
fn tokens_sql(at: &str) -> String {
    format!(
        "{BUCKETS}, c AS (SELECT {at} AS at, SUM(input + output) AS n FROM agent_usage \
         WHERE hour >= $from_s - 3599 AND hour < $to_s AND ($agent = '' OR agent = $agent) GROUP BY 1) \
         SELECT t.at AS time, COALESCE(c.n, 0) AS tokens FROM t LEFT JOIN c USING (at) ORDER BY t.at"
    )
}

/// The dashboard Charts starts with: what your agents did over the last
/// week, from your agents' activity.
pub fn default_dashboard() -> Dashboard {
    let buckets = BUCKETS;
    let counted = |table: &str, time: &str, filter: &str, column: &str| {
        format!(
            "{buckets}, c AS (SELECT ({time} / $step) * $step AS at, COUNT(*) AS n FROM {table} \
             WHERE {time} >= $from AND {time} < $to AND ($agent = '' OR agent = $agent){filter} GROUP BY 1) \
             SELECT t.at AS time, COALESCE(c.n, 0) AS {column} FROM t LEFT JOIN c USING (at) ORDER BY t.at"
        )
    };
    let records = |sql: String| Query { source: ACTIVITY.into(), sql };
    let stat = |id: &str, title: &str, x: u32, unit: &str, sql: String| Panel {
        id: id.into(),
        title: title.into(),
        grid: Grid { x, y: 0, w: 3, h: 3 },
        query: records(sql),
        viz: Viz { kind: "stat".into(), unit: unit.into(), ..Viz::default() },
    };
    // agent_usage is by the hour, in seconds.
    let hours = "MAX($step_s, 3600)";
    // By agent: agents only, not the person's chats; bars over time are
    // labelled with local times, which the chart reads as times.
    let agents = "AND agent != '' AND agent NOT LIKE 'chat:%' AND ($agent = '' OR agent = $agent)";
    Dashboard {
        id: "claktory".into(),
        name: "Claktory".into(),
        time: TimeRange::default(),
        refresh_s: default_refresh(),
        variables: vec![Variable {
            name: "agent".into(),
            label: "Agent".into(),
            query: records(
                "SELECT agent, MAX(agent_name) AS name FROM agent_usage WHERE hour >= $from_s - 3600 AND agent != '' \
                 AND agent NOT LIKE 'chat:%' GROUP BY agent ORDER BY name"
                    .into(),
            ),
            default: String::new(),
        }],
        panels: vec![
            stat("runs", "Runs", 0, "", counted("runs", "started", "", "runs")),
            stat("messages", "Messages", 3, "", counted("messages", "created", "", "messages")),
            stat("done", "Checklist items done", 6, "", counted("checklist_items", "updated", " AND status = 'done'", "done")),
            stat("tokens", "Tokens", 9, "", tokens_sql(TOKENS_AT)),
            Panel {
                id: "tokens_by_agent".into(),
                title: "Tokens by agent".into(),
                grid: Grid { x: 0, y: 1, w: 12, h: 7 },
                query: records(format!(
                    "SELECT strftime('%Y-%m-%d %H:%M', (hour / {hours}) * {hours}, 'unixepoch', 'localtime') AS time, agent, \
                     SUM(input + output) AS tokens FROM agent_usage WHERE hour >= $from_s - 3599 AND hour < $to_s {agents} \
                     GROUP BY 1, 2 ORDER BY 1"
                )),
                viz: Viz {
                    kind: "stackedBars".into(),
                    unit: "tokens".into(),
                    x: Some("time".into()),
                    series_by: Some("agent".into()),
                    color: Some("agent".into()),
                    ..Viz::default()
                },
            },
            Panel {
                id: "runs_by_agent".into(),
                title: "Runs by agent".into(),
                grid: Grid { x: 0, y: 2, w: 6, h: 5 },
                query: records(format!(
                    "SELECT strftime('%Y-%m-%d %H:%M', (started / $step) * $step / 1000, 'unixepoch', 'localtime') AS time, agent, \
                     COUNT(*) AS runs FROM runs WHERE started >= $from AND started < $to {agents} GROUP BY 1, 2 ORDER BY 1"
                )),
                viz: Viz {
                    kind: "stackedBars".into(),
                    unit: "runs".into(),
                    x: Some("time".into()),
                    series_by: Some("agent".into()),
                    color: Some("agent".into()),
                    ..Viz::default()
                },
            },
            Panel {
                id: "running_tasks".into(),
                title: "Background tasks running now".into(),
                grid: Grid { x: 6, y: 2, w: 6, h: 5 },
                query: records(
                    "SELECT agent, task, started FROM tasks WHERE ended IS NULL AND ($agent = '' OR agent = $agent) \
                     ORDER BY started DESC"
                        .into(),
                ),
                viz: Viz {
                    kind: "table".into(),
                    columns: vec![
                        Column { name: "agent".into(), title: Some("Agent".into()), format: None },
                        Column { name: "task".into(), title: Some("Task".into()), format: None },
                        Column { name: "started".into(), title: Some("Started".into()), format: Some("time".into()) },
                    ],
                    ..Viz::default()
                },
            },
        ],
        rev: 0,
        by: String::new(),
        format: FORMAT,
    }
}

// ---- The store ----

fn current_key(id: &str) -> String {
    format!("dashboard/{id}")
}

fn previous_key(id: &str) -> String {
    format!("previous/{id}")
}

fn read<T: for<'a> Deserialize<'a>>(host: &mut impl Host, key: &str) -> Option<T> {
    serde_json::from_slice(&host.kv_get(key)?).ok()
}

fn write<T: Serialize>(host: &mut impl Host, key: &str, value: &T) -> Result<(), String> {
    let bytes = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    host.kv_set(key, &bytes)
}

/// Where the person's home and starred dashboards are kept.
pub const PREFS_KEY: &str = "prefs";

/// How the person arranges the dashboards, the same on every device: the
/// one Charts opens on, and those starred, first in the dashboards' menu.
/// Their order is the index's.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub starred: Vec<String>,
}

/// The person's arrangement, without dashboards that are gone.
pub fn prefs(host: &mut impl Host) -> Prefs {
    let ids = ids(host);
    let mut prefs: Prefs = read(host, PREFS_KEY).unwrap_or_default();
    prefs.home = prefs.home.filter(|home| ids.contains(home));
    prefs.starred.retain(|id| ids.contains(id));
    prefs
}

pub fn save_prefs(host: &mut impl Host, prefs: &Prefs) -> Result<(), String> {
    write(host, PREFS_KEY, prefs)
}

/// The dashboard a view opens on: the home one, else the first.
pub fn opening(host: &mut impl Host) -> String {
    let home = prefs(host).home;
    home.or_else(|| ids(host).into_iter().next()).unwrap_or_default()
}

/// The dashboards as their menu lists them: the starred ones, then the
/// others, each in the person's order.
pub fn menu_order(ids: &[String], prefs: &Prefs) -> Vec<String> {
    let starred = |id: &&String| prefs.starred.contains(id);
    ids.iter().filter(starred).chain(ids.iter().filter(|id| !starred(id))).cloned().collect()
}

/// Where `id` can move in the menu: (up, down), within its group (starred
/// or not).
pub fn can_move(ids: &[String], prefs: &Prefs, id: &str) -> (bool, bool) {
    let group: Vec<&String> = ids.iter().filter(|other| prefs.starred.contains(other) == prefs.starred.iter().any(|s| s == id)).collect();
    match group.iter().position(|other| *other == id) {
        Some(at) => (at > 0, at + 1 < group.len()),
        None => (false, false),
    }
}

/// Moves `id` one place up or down in the menu, past the next dashboard
/// of its group.
pub fn move_board(host: &mut impl Host, id: &str, up: bool) -> Result<(), String> {
    let mut ids = ids(host);
    let prefs = prefs(host);
    let starred = prefs.starred.iter().any(|s| s == id);
    let at = ids.iter().position(|other| other == id).ok_or_else(|| format!("there's no dashboard \"{id}\""))?;
    let same = |other: &String| prefs.starred.contains(other) == starred;
    let next = if up { ids[..at].iter().rposition(same) } else { ids[at + 1..].iter().position(same).map(|n| at + 1 + n) };
    let Some(next) = next else { return Ok(()) };
    ids.swap(at, next);
    write(host, INDEX_KEY, &ids)
}

/// Puts the dashboards in `order` first, in that order, the others after
/// as they were.
pub fn set_order(host: &mut impl Host, order: &[String]) -> Result<(), String> {
    let ids = ids(host);
    let mut next: Vec<String> = Vec::new();
    for id in order.iter().filter(|id| ids.contains(id)) {
        if !next.contains(id) {
            next.push(id.clone());
        }
    }
    next.extend(ids.into_iter().filter(|id| !order.contains(id)));
    write(host, INDEX_KEY, &next)
}

/// The dashboards' ids, in order; None before the first start.
pub fn index(host: &mut impl Host) -> Option<Vec<String>> {
    read(host, INDEX_KEY)
}

pub fn ids(host: &mut impl Host) -> Vec<String> {
    index(host).unwrap_or_default()
}

pub fn load(host: &mut impl Host, id: &str) -> Option<Dashboard> {
    read(host, &current_key(id)).map(in_grafana_rows)
}

/// The version before the current one (or before it was deleted).
pub fn previous(host: &mut impl Host, id: &str) -> Option<Dashboard> {
    read(host, &previous_key(id)).map(in_grafana_rows)
}

/// A dashboard kept before 0.5.0, its heights 50 points a unit, in
/// Grafana's rows (38 points a row), so each panel stays as tall; kept so
/// at its next save.
fn in_grafana_rows(mut dashboard: Dashboard) -> Dashboard {
    if dashboard.format < FORMAT {
        for panel in &mut dashboard.panels {
            panel.grid.h = ((f64::from(panel.grid.h) * 50.0 / 38.0).round() as u32).clamp(1, MAX_HEIGHT);
        }
        dashboard.format = FORMAT;
    }
    dashboard
}

/// Why a save didn't happen.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveError {
    /// Someone saved it since `rev`: it's at this one now.
    Changed(u64),
    Full,
    Store(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Changed(rev) => write!(f, "it changed since you read it: it's at _rev {rev} now. Get it again, then put your change"),
            Self::Full => write!(f, "Charts keeps at most {MAX_DASHBOARDS} dashboards"),
            Self::Store(why) => write!(f, "couldn't keep it: {why}"),
        }
    }
}

/// Keeps `dashboard` as its id's next revision, the one it replaces as the
/// previous version. With `base`, only if it's still at that revision.
/// Returns the revision kept.
pub fn save(host: &mut impl Host, mut dashboard: Dashboard, base: Option<u64>, by: &str) -> Result<Dashboard, SaveError> {
    let mut ids = ids(host);
    let current = load(host, &dashboard.id);
    let at = current.as_ref().map_or(0, |current| current.rev);
    if let Some(base) = base
        && current.is_some()
        && base != at
    {
        return Err(SaveError::Changed(at));
    }
    if current.is_none() && ids.len() >= MAX_DASHBOARDS && !ids.contains(&dashboard.id) {
        return Err(SaveError::Full);
    }
    let last = previous(host, &dashboard.id).map_or(0, |previous| previous.rev);
    dashboard.rev = at.max(last) + 1;
    dashboard.by = by.to_owned();
    dashboard.format = FORMAT;
    if let Some(current) = &current {
        write(host, &previous_key(&dashboard.id), current).map_err(SaveError::Store)?;
    }
    write(host, &current_key(&dashboard.id), &dashboard).map_err(SaveError::Store)?;
    if !ids.contains(&dashboard.id) {
        ids.push(dashboard.id.clone());
        write(host, INDEX_KEY, &ids).map_err(SaveError::Store)?;
    }
    Ok(dashboard)
}

/// Deletes a dashboard, keeping it as the previous version, so putting it
/// back is one `charts_get` and `charts_put` away.
pub fn delete(host: &mut impl Host, id: &str) -> Result<Dashboard, String> {
    let mut ids = ids(host);
    let current = load(host, id).ok_or_else(|| format!("there's no dashboard \"{id}\""))?;
    write(host, &previous_key(id), &current)?;
    host.kv_delete(&current_key(id));
    ids.retain(|known| known != id);
    write(host, INDEX_KEY, &ids)?;
    Ok(current)
}

/// Puts the previous version back, as a new revision (so the one it
/// replaces becomes the previous version: restoring again undoes it).
pub fn restore(host: &mut impl Host, id: &str, by: &str) -> Result<Dashboard, String> {
    let previous = previous(host, id).ok_or_else(|| format!("\"{id}\" has no previous version"))?;
    save(host, previous, None, by).map_err(|error| error.to_string())
}

/// The first start: the default dashboard, in place of C4a's. Every
/// later one: the default dashboard's fixes, where it wasn't changed.
pub fn first_start(host: &mut impl Host) -> Result<(), String> {
    if index(host).is_some() {
        return upgrade(host);
    }
    host.kv_delete(LEGACY_KEY);
    write(host, INDEX_KEY, &Vec::<String>::new())?;
    save(host, default_dashboard(), None, "").map(|_| ()).map_err(|error| error.to_string())
}

/// The default dashboard's Tokens stat as 0.2.0 kept it, untouched since,
/// gets 0.2.1's query (a new revision, the old one its previous version);
/// one the person or an agent changed stays as it is.
fn upgrade(host: &mut impl Host) -> Result<(), String> {
    let Some(mut dashboard) = load(host, "claktory") else { return Ok(()) };
    let old = tokens_sql(TOKENS_AT_0_2_0);
    let Some(panel) = dashboard.panels.iter_mut().find(|panel| panel.id == "tokens" && panel.query.sql == old) else {
        return Ok(());
    };
    panel.query.sql = tokens_sql(TOKENS_AT);
    let rev = dashboard.rev;
    save(host, dashboard, Some(rev), "").map(|_| ()).map_err(|error| error.to_string())
}
