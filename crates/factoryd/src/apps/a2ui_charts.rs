//! The chart family of Claktory's A2UI catalog: Chart, Stat, Table, Map and
//! Calendar, the kinds of value only they take (a query's answer, series, a
//! spark, a Stat's color, a Table's columns, a Calendar's zone and tools),
//! their rules across properties, and what a Calendar's rows open
//! (`open_cells`). `a2ui.rs` checks everything else and asks here for these.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use super::Kind::{Choice, DynString, Number, Text};
use super::{Invalid, Props, binding, invalid, literal, only};

/// A Table value's most columns, a Chart's most series and a Stat's most
/// spark points.
const MAX_COLUMNS: usize = 64;
const MAX_SERIES: usize = 12;
const MAX_SPARK: usize = 200;
/// A Stat's most thresholds.
const MAX_THRESHOLDS: usize = 10;
/// A Calendar's most tools (the menus in its bar).
const MAX_TOOLS: usize = 8;
/// A time zone's offset from UTC, in minutes: UTC-14:00 to UTC+14:00.
const MAX_ZONE_MINUTES: i64 = 840;

/// The charts' palette by name: the renderer gives each the
/// theme's shade; "text" is the text's own color. Never a value.
pub(super) const COLOR_NAMES: [&str; 14] = [
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

/// The kinds of property value only the chart family takes.
#[derive(Clone, Copy)]
pub(super) enum Kind {
    /// A query's answer, `{columns, rows, more?}`, or a binding to one.
    Table,
    /// A plain list of 1 to 12 column names.
    Series,
    /// A string or a number, or a binding.
    DynScalar,
    /// A list of at most `MAX_SPARK` numbers, or a binding.
    Spark,
    /// A number in a range.
    Range(f64, f64),
    /// "signal", "muted", "agent:<id>" or one of `COLOR_NAMES`.
    StatColor,
    /// A Stat's thresholds: [{color, value?}], the first without a value.
    Thresholds,
    /// A Table's columns: [{name, title?, format?}].
    Columns,
    /// A whole number in a range.
    Whole(u64, u64),
    /// true or false, never a binding.
    Flag,
    /// The Claktory's time zone, for a Calendar to say when it isn't the
    /// device's: {start, end, name?}, its offset from UTC in whole minutes
    /// at the start and end of what's shown (a change of summer time
    /// between them), and its name (at most 64 characters).
    MacZone,
    /// A list of at most `MAX_TOOLS` components' ids.
    Tools,
}

const TIME_MAX: u64 = 8_640_000_000_000_000;

const TABLE: super::Kind = super::Kind::Chart(Kind::Table);
const SERIES: super::Kind = super::Kind::Chart(Kind::Series);
const DYN_SCALAR: super::Kind = super::Kind::Chart(Kind::DynScalar);
const SPARK: super::Kind = super::Kind::Chart(Kind::Spark);
const STAT_COLOR: super::Kind = super::Kind::Chart(Kind::StatColor);
const COLUMNS: super::Kind = super::Kind::Chart(Kind::Columns);
const FLAG: super::Kind = super::Kind::Chart(Kind::Flag);

/// The chart family's components and their properties: (name, kind,
/// required).
pub(super) fn catalog(component: &str) -> Option<Props> {
    Some(match component {
        "Chart" => &[
            // heatmap: a grid of x's values by seriesBy's, each cell
            // colored by its y (an hour by weekday, say).
            ("kind", Choice(&["timeseries", "bars", "stackedBars", "pie", "scatter", "heatmap"]), true),
            ("data", TABLE, true),
            ("x", Text, false),
            ("y", SERIES, false),
            ("seriesBy", Text, false),
            ("color", Choice(&["palette", "agent", "signal", "muted"]), false),
            ("label", DynString, false),
            ("unit", DynString, false),
            ("height", super::Kind::Chart(Kind::Range(120.0, 480.0)), false),
            // How it draws, each a plain value; left out, the
            // renderer's default: a line 1 px wide, no fill, points when far
            // enough apart, 5 px across, the value axis fitted to the data,
            // automatic decimals and a legend.
            ("lineWidth", super::Kind::Chart(Kind::Range(0.0, 10.0)), false),
            ("fill", super::Kind::Chart(Kind::Range(0.0, 100.0)), false),
            ("points", Choice(&["auto", "always", "never"]), false),
            ("pointSize", super::Kind::Chart(Kind::Range(1.0, 40.0)), false),
            ("min", Number, false),
            ("max", Number, false),
            ("decimals", super::Kind::Chart(Kind::Whole(0, 10)), false),
            ("legend", FLAG, false),
            // The time axis's ends, in milliseconds since 1970: the
            // range shown, rows or not. The most a device's clock can
            // say is 8.64e15.
            ("timeFrom", super::Kind::Chart(Kind::Whole(0, TIME_MAX)), false),
            ("timeTo", super::Kind::Chart(Kind::Whole(0, TIME_MAX)), false),
        ],
        "Stat" => &[
            ("label", DynString, true),
            ("value", DYN_SCALAR, true),
            ("unit", DynString, false),
            ("spark", SPARK, false),
            ("color", STAT_COLOR, false),
            // How it draws, each a plain value; left out, the
            // renderer's default: green, red from 80, the value colored,
            // a spark line under it, automatic decimals, its own height.
            ("thresholds", super::Kind::Chart(Kind::Thresholds), false),
            ("colorMode", Choice(&["value", "background", "solid", "none"]), false),
            ("graph", Choice(&["area", "none"]), false),
            ("decimals", super::Kind::Chart(Kind::Whole(0, 10)), false),
            ("height", super::Kind::Chart(Kind::Range(60.0, 480.0)), false),
        ],
        "Table" => &[
            ("data", TABLE, true),
            ("columns", COLUMNS, false),
            // The whole table's height, as Grafana's table panel fills its
            // place: its heading stays and its rows scroll inside. Left out,
            // as tall as its rows.
            ("height", super::Kind::Chart(Kind::Range(120.0, 480.0)), false),
        ],
        // A world map, as Grafana's geomap with a country lookup: a dot at
        // each country (ISO 3166-1 alpha-2 or alpha-3 in `location`; the
        // first text column when left out) sized and colored by `value`
        // (the last numeric column), on a map the app bundles. Rows it
        // can't place are listed under it in words.
        "Map" => &[
            ("data", TABLE, true),
            ("location", Text, false),
            ("value", Text, false),
            ("label", DynString, false),
            ("unit", DynString, false),
            ("decimals", super::Kind::Chart(Kind::Whole(0, 10)), false),
            // Colored by thresholds, or one color; left out, green to
            // yellow to red by value.
            ("thresholds", super::Kind::Chart(Kind::Thresholds), false),
            ("color", STAT_COLOR, false),
            // A dot's radius from the least value to the most, in pixels.
            ("sizeMin", super::Kind::Chart(Kind::Range(1.0, 40.0)), false),
            ("sizeMax", super::Kind::Chart(Kind::Range(1.0, 40.0)), false),
            ("legend", FLAG, false),
            ("height", super::Kind::Chart(Kind::Range(120.0, 480.0)), false),
        ],
        // What agents have scheduled, as a day, a week or an agenda from
        // `anchor` (ms since 1970): the rows of `data`, by their columns'
        // names (id, start, end, title, agent, color, allDay, kind,
        // repeat, detail, open, live). Each row's `open` says where it
        // opens in Claktory, which the device follows itself
        // (`open_cells`). Its `action` goes with {anchor, view} added when
        // the person moves or changes the view.
        "Calendar" => &[
            ("data", TABLE, true),
            ("view", Choice(&["day", "week", "agenda"]), false),
            ("anchor", super::Kind::Chart(Kind::Whole(0, TIME_MAX)), false),
            ("weekStart", Choice(&["monday", "sunday", "saturday"]), false),
            ("macZone", super::Kind::Chart(Kind::MacZone), false),
            ("tools", super::Kind::Chart(Kind::Tools), false),
            ("emptyTitle", DynString, false),
            ("agendaEmptyTitle", DynString, false),
            ("emptyBody", DynString, false),
            ("label", DynString, false),
            ("action", super::Kind::Action, false),
            ("height", super::Kind::Chart(Kind::Range(240.0, 2000.0)), false),
        ],
        _ => return None,
    })
}

/// Whether a component's unknown plain keys are left out rather than
/// refused: the chart family's presentation (and Row's), which the
/// Charts app may set for a newer catalog than this one, after a roll-back
/// say. What's left out never reaches a device; known keys are still checked.
pub(super) fn lenient(component: &str) -> bool {
    matches!(component, "Chart" | "Stat" | "Row" | "Map" | "Table" | "Calendar")
}

/// Rules across a component's properties, once each property is checked on
/// its own.
pub(super) fn component(name: &str, fields: &Map<String, Value>, at: &str) -> Result<(), Invalid> {
    if name == "Chart" && fields.contains_key("seriesBy") && fields.get("y").and_then(Value::as_array).is_some_and(|y| y.len() > 1) {
        return Err(invalid(format!("{at}/y"), "with seriesBy, y names one column at most"));
    }
    if name == "Chart"
        && let (Some(min), Some(max)) = (fields.get("min").and_then(Value::as_f64), fields.get("max").and_then(Value::as_f64))
        && min >= max
    {
        return Err(invalid(format!("{at}/min"), "min is below max"));
    }
    if name == "Chart"
        && let (Some(from), Some(to)) = (fields.get("timeFrom").and_then(Value::as_u64), fields.get("timeTo").and_then(Value::as_u64))
        && from >= to
    {
        return Err(invalid(format!("{at}/timeFrom"), "timeFrom is before timeTo"));
    }
    if name == "Map"
        && let (Some(least), Some(most)) = (fields.get("sizeMin").and_then(Value::as_f64), fields.get("sizeMax").and_then(Value::as_f64))
        && least > most
    {
        return Err(invalid(format!("{at}/sizeMin"), "sizeMin is at most sizeMax"));
    }
    Ok(())
}

pub(super) fn check(kind: Kind, value: &Value, at: &str) -> Result<(), Invalid> {
    match kind {
        Kind::Table => match value.as_object() {
            // A literal table is an object too: a binding only when it says so.
            Some(fields) if fields.keys().any(|key| key.starts_with('@')) => binding(value, at).expect("an object"),
            _ => table(value, at),
        },
        Kind::Series => match value {
            Value::Array(names) if !names.is_empty() && names.len() <= MAX_SERIES && names.iter().all(Value::is_string) => Ok(()),
            _ => Err(invalid(at, format!("this is a list of 1 to {MAX_SERIES} column names"))),
        },
        Kind::DynScalar => {
            binding(value, at).unwrap_or_else(|| literal(value.is_string() || value.is_number(), "a string or a number", at))
        }
        Kind::Spark => binding(value, at).unwrap_or_else(|| {
            let numbers = value.as_array().is_some_and(|items| items.len() <= MAX_SPARK && items.iter().all(Value::is_number));
            literal(numbers, &format!("a list of at most {MAX_SPARK} numbers"), at)
        }),
        Kind::Range(low, high) => match value.as_f64() {
            Some(number) if (low..=high).contains(&number) => Ok(()),
            _ => Err(invalid(at, format!("this is a number from {low} to {high}"))),
        },
        Kind::StatColor => match value.as_str() {
            Some("signal" | "muted") => Ok(()),
            Some(color) if COLOR_NAMES.contains(&color) => Ok(()),
            Some(color) if color.strip_prefix("agent:").is_some_and(agent_id) => Ok(()),
            _ => Err(invalid(at, format!("this is signal, muted, agent:<an agent's id> or one of {}", COLOR_NAMES.join(", ")))),
        },
        Kind::Thresholds => {
            let Value::Array(steps) = value else { return Err(invalid(at, format!("thresholds are a list of 1 to {MAX_THRESHOLDS}"))) };
            if steps.is_empty() || steps.len() > MAX_THRESHOLDS {
                return Err(invalid(at, format!("thresholds are a list of 1 to {MAX_THRESHOLDS}")));
            }
            let mut last = None;
            for (index, step) in steps.iter().enumerate() {
                let here = format!("{at}/{index}");
                let Value::Object(step) = step else { return Err(invalid(&here, "a threshold is {color, value?}")) };
                only(step, &here, &["color", "value"])?;
                match step.get("color").and_then(Value::as_str) {
                    Some(color) if COLOR_NAMES.contains(&color) => {}
                    _ => return Err(invalid(format!("{here}/color"), format!("this is one of {}", COLOR_NAMES.join(", ")))),
                }
                match (index, step.get("value")) {
                    (0, None) => {}
                    (0, Some(_)) => return Err(invalid(format!("{here}/value"), "the first threshold is the base: it has no value")),
                    (_, Some(value)) => match value.as_f64() {
                        Some(number) if last.is_none_or(|last| number > last) => last = Some(number),
                        Some(_) => return Err(invalid(format!("{here}/value"), "each threshold's value is above the one before")),
                        None => return Err(invalid(format!("{here}/value"), "this is a number")),
                    },
                    (_, None) => return Err(invalid(format!("{here}/value"), "a threshold after the first has a value")),
                }
            }
            Ok(())
        }
        Kind::Columns => match value {
            Value::Array(columns) if !columns.is_empty() && columns.len() <= MAX_COLUMNS => {
                columns.iter().enumerate().try_for_each(|(index, column)| {
                    let here = format!("{at}/{index}");
                    let Value::Object(column) = column else { return Err(invalid(&here, "a column is {name, title?, format?}")) };
                    only(column, &here, &["name", "title", "format"])?;
                    super::check(Text, column.get("name").unwrap_or(&Value::Null), &format!("{here}/name"))?;
                    if let Some(title) = column.get("title") {
                        super::check(DynString, title, &format!("{here}/title"))?;
                    }
                    match column.get("format") {
                        Some(format) => super::check(Choice(&["text", "number", "time"]), format, &format!("{here}/format")),
                        None => Ok(()),
                    }
                })
            }
            _ => Err(invalid(at, format!("columns are a list of 1 to {MAX_COLUMNS}"))),
        },
        Kind::Whole(low, high) => match value.as_u64() {
            Some(number) if (low..=high).contains(&number) => Ok(()),
            _ => Err(invalid(at, format!("this is a whole number from {low} to {high}"))),
        },
        Kind::Flag => match value {
            Value::Bool(_) => Ok(()),
            _ => Err(invalid(at, "this is true or false")),
        },
        Kind::MacZone => {
            let Value::Object(zone) = value else { return Err(invalid(at, "macZone is {start, end, name?}")) };
            only(zone, at, &["start", "end", "name"])?;
            for key in ["start", "end"] {
                match zone.get(key).and_then(Value::as_i64) {
                    Some(minutes) if (-MAX_ZONE_MINUTES..=MAX_ZONE_MINUTES).contains(&minutes) => {}
                    _ => {
                        return Err(invalid(
                            format!("{at}/{key}"),
                            format!("this is a whole number of minutes from -{MAX_ZONE_MINUTES} to {MAX_ZONE_MINUTES}"),
                        ));
                    }
                }
            }
            match zone.get("name") {
                None => Ok(()),
                Some(Value::String(name)) if name.chars().count() <= 64 && !name.chars().any(char::is_control) => Ok(()),
                Some(_) => Err(invalid(format!("{at}/name"), "a zone's name is a line of at most 64 characters")),
            }
        }
        Kind::Tools => match value {
            Value::Array(ids) if ids.len() <= MAX_TOOLS => {
                ids.iter().enumerate().try_for_each(|(index, id)| super::check(super::Kind::Id, id, &format!("{at}/{index}")))
            }
            _ => Err(invalid(at, format!("tools are a list of at most {MAX_TOOLS} components' ids"))),
        },
    }
}

/// Checks what every Calendar's rows open, wherever the rows are (in the
/// component, or in `data` where it's bound): each `open` cell is null,
/// "agent:<id>", "item:<agent>:<n>", "monitor:<agent>:m<n>" (or a bare
/// <n>) or "task:<agent>:<task id>", ids being letters, digits, _ and -
/// up to 64; anything else refuses the message. A cell naming an agent
/// that's gone stays as it is, so a device can say the agent was deleted.
pub(super) fn open_cells(components: &BTreeMap<String, Value>, data: &Value) -> Result<(), Invalid> {
    for (id, component) in components {
        if component.get("component").and_then(Value::as_str) != Some("Calendar") {
            continue;
        }
        let table = match component.pointer("/data/@path").and_then(Value::as_str) {
            Some(path) => data.pointer(path),
            None => component.get("data"),
        };
        let Some(Value::Object(table)) = table else { continue };
        let Some(columns) = table.get("columns").and_then(Value::as_array) else { continue };
        if let Some(twice) = columns.iter().enumerate().find_map(|(at, name)| columns[..at].contains(name).then_some(name)) {
            return Err(invalid("", format!("Calendar \"{id}\"'s rows name the column {twice} twice")));
        }
        let Some(open) = columns.iter().position(|name| name == "open") else { continue };
        let Some(Value::Array(rows)) = table.get("rows") else { continue };
        for (index, row) in rows.iter().enumerate() {
            let Some(cell) = row.as_array().and_then(|cells| cells.get(open)) else { continue };
            let shaped = match cell {
                Value::Null => true,
                Value::String(target) => open_agent(target).is_some(),
                _ => false,
            };
            if !shaped {
                return Err(invalid(
                    "",
                    format!(
                        "Calendar \"{id}\"'s row {index} opens {cell}, which isn't agent:<id>, item:<agent>:<n>, monitor:<agent>:m<n> or task:<agent>:<task id>"
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// The agent an open cell names, when it's one of the four shapes.
fn open_agent(target: &str) -> Option<&str> {
    let id = |id: &str| !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    let number = |n: &str| !n.is_empty() && n.len() <= 18 && n.chars().all(|c| c.is_ascii_digit());
    let (kind, rest) = target.split_once(':')?;
    let (agent, place) = match kind {
        "agent" => (rest, None),
        _ => rest.split_once(':').map(|(agent, place)| (agent, Some(place)))?,
    };
    let shaped = match (kind, place) {
        ("agent", None) => true,
        ("item", Some(n)) => number(n),
        ("monitor", Some(m)) => number(m.strip_prefix('m').unwrap_or(m)),
        ("task", Some(task)) => id(task),
        _ => false,
    };
    (shaped && id(agent)).then_some(agent)
}

/// An agent's id as a color names it: lowercase letters, digits, - and _.
fn agent_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A literal Table value: {columns: [name…], rows: [[cell…]…], more?}, each
/// row as long as columns, each cell a string, number, true/false or null.
fn table(value: &Value, at: &str) -> Result<(), Invalid> {
    let Value::Object(table) = value else { return Err(invalid(at, "a table is {columns, rows, more?}, or a {\"@path\": …} binding")) };
    only(table, at, &["columns", "rows", "more"])?;
    let width = match table.get("columns") {
        Some(Value::Array(columns)) if !columns.is_empty() && columns.len() <= MAX_COLUMNS && columns.iter().all(Value::is_string) => {
            columns.len()
        }
        _ => return Err(invalid(format!("{at}/columns"), format!("columns are a list of 1 to {MAX_COLUMNS} names"))),
    };
    let Some(Value::Array(rows)) = table.get("rows") else { return Err(invalid(format!("{at}/rows"), "rows are a list of rows")) };
    for (index, row) in rows.iter().enumerate() {
        let scalar = |cell: &Value| !matches!(cell, Value::Array(_) | Value::Object(_));
        if !row.as_array().is_some_and(|cells| cells.len() == width && cells.iter().all(scalar)) {
            return Err(invalid(format!("{at}/rows/{index}"), format!("a row is a list of {width} strings, numbers, true/false or null")));
        }
    }
    match table.get("more") {
        None | Some(Value::Bool(_)) => Ok(()),
        Some(_) => Err(invalid(format!("{at}/more"), "more is true or false")),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::super::Applied;
    use super::super::tests::{S, apply, created};
    use super::TIME_MAX;

    #[test]
    fn charts_stats_and_tables_take_a_querys_answer() {
        let mut content = created();
        let rows = json!({"columns": ["day", "agent", "tokens"], "rows": [["2026-11-22", "lea", 120], ["2026-11-22", "rivet", null]], "more": false});
        let good = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "chart", "component": "Chart", "kind": "timeseries", "data": {"@path": "/usage"}, "x": "day", "y": ["tokens"],
             "seriesBy": "agent", "color": "agent", "label": "Tokens a day", "unit": "tokens", "height": 240},
            {"id": "bars", "component": "Chart", "kind": "bars", "data": rows},
            {"id": "stat", "component": "Stat", "label": "Today", "value": 1200, "unit": "tokens", "spark": [1, 2, 3], "color": "agent:lea"},
            {"id": "named", "component": "Stat", "label": {"@path": "/l"}, "value": "n/a", "spark": {"@path": "/s"}},
            {"id": "table", "component": "Table", "data": {"@path": "/usage"},
             "columns": [{"name": "day", "format": "time"}, {"name": "tokens", "title": "Tokens", "format": "number"}]},
            {"id": "tall_title", "component": "Text", "text": "Countries", "variant": "title"},
            {"id": "tall", "component": "Table", "data": rows, "height": 120},
            {"id": "taller", "component": "Table", "data": rows, "height": 480},
        ]}});
        apply(&mut content, good).unwrap();
        let styled = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "line", "component": "Chart", "kind": "timeseries", "data": rows, "lineWidth": 0, "fill": 100, "points": "always",
             "pointSize": 40, "min": -1.5, "max": 1e6, "decimals": 10, "legend": false,
             "timeFrom": 1_795_420_800_000u64, "timeTo": 1_795_442_400_000u64},
            {"id": "edges", "component": "Chart", "kind": "scatter", "data": rows, "lineWidth": 10, "fill": 0, "points": "never",
             "pointSize": 1, "min": 0, "decimals": 0, "legend": true},
        ]}});
        apply(&mut content, styled).unwrap();
        let stats = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "grafana", "component": "Stat", "label": "CPU", "value": 42.5, "unit": "percent",
             "thresholds": [{"color": "green"}, {"color": "semi-dark-yellow", "value": -1.5}, {"color": "dark-red", "value": 80}],
             "colorMode": "background", "graph": "none", "decimals": 1, "height": 60, "color": "text"},
            {"id": "plain", "component": "Stat", "label": "Runs", "value": {"@path": "/none"}, "thresholds": [{"color": "blue"}], "colorMode": "solid",
             "graph": "area", "decimals": 0, "height": 480, "color": "purple"},
            {"id": "none", "component": "Stat", "label": "Runs", "value": 1, "colorMode": "none", "color": "agent:lea"},
        ]}});
        apply(&mut content, stats).unwrap();
        let component = |component: Value| json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [component]}});
        for (wrong, why) in [
            (json!({"id": "c", "component": "Chart", "data": rows}), "Chart needs kind"),
            (json!({"id": "c", "component": "Chart", "kind": "radar", "data": rows}), "one of timeseries"),
            (json!({"id": "c", "component": "Chart", "kind": {"@path": "/k"}, "data": rows}), "one of timeseries"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "height": 900}), "from 120 to 480"),
            (
                json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "y": ["a", "b"], "seriesBy": "c"}),
                "y names one column",
            ),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "y": []}), "1 to 12 column names"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "@theme": "x"}), "has no property \"@theme\""),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [[1, 2]]}}), "a list of 1 strings"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [[{"x": 1}]]}}), "rows/0"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": [], "rows": []}}), "1 to 64 names"),
            (
                json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [], "x": 1}}),
                "isn't allowed here",
            ),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "lineWidth": 11}), "from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "lineWidth": {"@path": "/w"}}), "from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "fill": -1}), "from 0 to 100"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "points": "sometimes"}), "one of auto, always, never"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "pointSize": 0.5}), "from 1 to 40"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "min": "0"}), "this is a number"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "max": {"@path": "/m"}}), "this is a number"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "min": 5, "max": 5}), "min is below max"),
            (json!({"id": "c", "component": "Chart", "kind": "timeseries", "data": rows, "timeFrom": 9, "timeTo": 9}), "timeFrom is before timeTo"),
            (json!({"id": "c", "component": "Chart", "kind": "timeseries", "data": rows, "timeFrom": -1}), "whole number from 0"),
            (json!({"id": "c", "component": "Chart", "kind": "timeseries", "data": rows, "timeTo": 1.5}), "whole number from 0"),
            (json!({"id": "c", "component": "Chart", "kind": "timeseries", "data": rows, "timeTo": {"@path": "/t"}}), "whole number from 0"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "decimals": 1.5}), "whole number from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "decimals": 11}), "whole number from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "decimals": -1}), "whole number from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "legend": "no"}), "true or false"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "legend": {"@path": "/l"}}), "true or false"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": true}), "a string or a number"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": []}), "a list of 1 to 10"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": {"@path": "/t"}}), "a list of 1 to 10"),
            (
                json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": vec![json!({"color": "red"}); 11]}),
                "a list of 1 to 10",
            ),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": [{"color": "#ff0000"}]}), "one of green"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": [{"color": "red", "value": 1}]}), "the base"),
            (
                json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": [{"color": "red"}, {"color": "blue"}]}),
                "after the first has a value",
            ),
            (
                json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": [{"color": "red"}, {"color": "blue", "value": "80"}]}),
                "this is a number",
            ),
            (
                json!({"id": "s", "component": "Stat", "label": "x", "value": 1,
                       "thresholds": [{"color": "red"}, {"color": "blue", "value": 80}, {"color": "green", "value": 80}]}),
                "above the one before",
            ),
            (
                json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": [{"color": "red", "op": "gt"}]}),
                "isn't allowed here",
            ),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "thresholds": ["red"]}), "{color, value?}"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "colorMode": "gradient"}), "one of value, background"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "graph": "line"}), "one of area, none"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "decimals": 2.5}), "whole number from 0 to 10"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "height": 40}), "from 60 to 480"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "height": {"@path": "/h"}}), "from 60 to 480"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "color": "#ff0000"}), "signal, muted, agent"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "color": "agent:Lea Smith"}), "signal, muted, agent"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "spark": ["a"]}), "at most 200 numbers"),
            (json!({"id": "t", "component": "Table", "data": rows, "columns": [{"name": "day", "format": "date"}]}), "one of text"),
            (json!({"id": "t", "component": "Table", "data": rows, "columns": [{"title": "Day"}]}), "this is a string"),
            (json!({"id": "t", "component": "Table", "data": rows, "height": 100}), "from 120 to 480"),
            (json!({"id": "t", "component": "Table", "data": rows, "height": 481}), "from 120 to 480"),
            (json!({"id": "t", "component": "Table", "data": rows, "height": {"@path": "/h"}}), "from 120 to 480"),
        ] {
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
        }
    }

    #[test]
    fn maps_and_heatmaps_take_a_querys_answer() {
        let mut content = created();
        let users = json!({"columns": ["country", "users"], "rows": [["DE", 12], ["usa", 40], ["Hidden (fewer than 5)", 9], ["FR", null]]});
        let hours = json!({"columns": ["day", "hour", "active"], "rows": [["Mon", "14:00", 3], ["Tue", "14:00", null]]});
        let good = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "map", "component": "Map", "data": users},
            {"id": "styled", "component": "Map", "data": {"@path": "/users"}, "location": "country", "value": "users",
             "label": "Users by country", "unit": "users", "decimals": 0, "thresholds": [{"color": "green"}, {"color": "red", "value": 10}],
             "color": "blue", "sizeMin": 1, "sizeMax": 40, "legend": false, "height": 300},
            {"id": "same", "component": "Map", "data": users, "sizeMin": 8, "sizeMax": 8},
            {"id": "grid", "component": "Chart", "kind": "heatmap", "data": hours, "x": "hour", "y": ["active"], "seriesBy": "day"},
        ]}});
        apply(&mut content, good).unwrap();
        let component = |component: Value| json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [component]}});
        for (wrong, why) in [
            (json!({"id": "m", "component": "Map"}), "Map needs data"),
            (json!({"id": "m", "component": "Map", "data": users, "location": 1}), "this is a string"),
            (json!({"id": "m", "component": "Map", "data": users, "value": {"@path": "/v"}}), "this is a string"),
            (json!({"id": "m", "component": "Map", "data": users, "sizeMin": 0}), "from 1 to 40"),
            (json!({"id": "m", "component": "Map", "data": users, "sizeMax": 41}), "from 1 to 40"),
            (json!({"id": "m", "component": "Map", "data": users, "sizeMin": 20, "sizeMax": 10}), "sizeMin is at most sizeMax"),
            (json!({"id": "m", "component": "Map", "data": users, "color": "#00ff00"}), "signal, muted, agent"),
            (json!({"id": "m", "component": "Map", "data": users, "thresholds": [{"color": "red", "value": 1}]}), "the base"),
            (json!({"id": "m", "component": "Map", "data": users, "height": 100}), "from 120 to 480"),
            (json!({"id": "m", "component": "Map", "data": users, "legend": 1}), "true or false"),
            (json!({"id": "m", "component": "Map", "data": users, "@tiles": "https://x"}), "has no property \"@tiles\""),
            (
                json!({"id": "c", "component": "Chart", "kind": "heatmap", "data": hours, "y": ["a", "b"], "seriesBy": "day"}),
                "y names one column",
            ),
        ] {
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
        }
        // A newer catalog's settings on a Map are left out, as on a Chart.
        let newer = component(json!({"id": "m", "component": "Map", "data": users, "basemap": "satellite"}));
        let Ok(Applied::Changed(sent)) = apply(&mut content, newer) else { panic!("applied") };
        assert!(!sent.contains("basemap"));
    }

    #[test]
    fn a_calendar_takes_its_rows_view_zone_and_tools() {
        let mut content = created();
        let rows = json!({"columns": ["id", "start", "end", "title", "agent", "open"], "rows": [
            ["m1", 1_764_770_400_000u64, null, "Logs", "lea", "monitor:lea:m3"],
            ["m2", 1_764_770_400_000u64, null, "Disk", "lea", "monitor:lea:3"],
            ["i1", 1_764_770_400_000u64, null, "Ship", "lea", "item:lea:12"],
            ["t1", 1_764_770_400_000u64, null, "Build", "lea", "task:lea:task-7"],
            ["a1", 1_764_770_400_000u64, null, "Release", "lea", "agent:lea"],
            ["e1", 1_764_770_400_000u64, null, "Launch", "lea", null],
        ]});
        let good = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "cal", "component": "Calendar", "data": {"@path": "/rows"}, "view": "week", "anchor": 1_764_770_400_000u64,
             "weekStart": "monday", "macZone": {"start": 60, "end": 60}, "tools": ["agent_menu", "show_menu", "more_menu"],
             "emptyTitle": "Nothing on the calendar this week", "agendaEmptyTitle": "Nothing on the calendar coming up",
             "emptyBody": {"@path": "/empty"}, "label": "Calendar",
             "action": {"event": {"name": "calendar"}}, "height": 240},
            {"id": "day", "component": "Calendar", "data": rows, "view": "day", "anchor": 0, "weekStart": "sunday",
             "macZone": {"start": -840, "end": 840, "name": "Europe/Rome"}, "tools": [], "height": 2000},
            {"id": "agenda", "component": "Calendar", "data": rows, "view": "agenda", "anchor": TIME_MAX, "weekStart": "saturday"},
        ]}});
        let Ok(Applied::Changed(sent)) = apply(&mut content, good) else { panic!("applied") };
        assert!(sent.contains("Nothing on the calendar coming up"), "the agenda's own empty title reaches devices");
        let component = |component: Value| json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [component]}});
        let calendar = |extra: Value| {
            let mut calendar = json!({"id": "c", "component": "Calendar", "data": {"@path": "/rows"}});
            calendar.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            component(calendar)
        };
        for (wrong, why) in [
            (component(json!({"id": "c", "component": "Calendar"})), "Calendar needs data"),
            (calendar(json!({"view": "month"})), "one of day, week, agenda"),
            (calendar(json!({"weekStart": "tuesday"})), "one of monday, sunday, saturday"),
            (calendar(json!({"anchor": -1})), "whole number from 0"),
            (calendar(json!({"anchor": {"@path": "/a"}})), "whole number from 0"),
            (calendar(json!({"macZone": {"start": 841, "end": 0}})), "from -840 to 840"),
            (calendar(json!({"macZone": {"start": 0, "end": -841}})), "from -840 to 840"),
            (calendar(json!({"macZone": {"start": 1.5, "end": 0}})), "whole number of minutes"),
            (calendar(json!({"macZone": {"start": 0}})), "whole number of minutes"),
            (calendar(json!({"macZone": {"start": 0, "end": 0, "name": "x".repeat(65)}})), "at most 64 characters"),
            (calendar(json!({"macZone": {"start": 0, "end": 0, "name": "Rome\nfake"}})), "at most 64 characters"),
            (calendar(json!({"macZone": {"start": 0, "end": 0, "dst": true}})), "isn't allowed here"),
            (calendar(json!({"macZone": "UTC+1"})), "macZone is {start, end, name?}"),
            (calendar(json!({"tools": ["a", "b", "c", "d", "e", "f", "g", "h", "i"]})), "at most 8"),
            (calendar(json!({"tools": ["1menu"]})), "a component's id"),
            (calendar(json!({"tools": {"componentId": "m", "path": "/t"}})), "at most 8"),
            (calendar(json!({"height": 239})), "from 240 to 2000"),
            (calendar(json!({"height": 2001})), "from 240 to 2000"),
            (calendar(json!({"action": {"event": {"name": "go", "context": {"x": {"@call": "f"}}}}})), "aren't supported"),
            (calendar(json!({"label": 3})), "a string"),
            (calendar(json!({"agendaEmptyTitle": ["x"]})), "a string"),
            (calendar(json!({"@open": "x"})), "has no property \"@open\""),
        ] {
            let before = content.snapshot(S);
            let error = apply(&mut content, wrong.clone()).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
            assert_eq!(content.snapshot(S), before, "{wrong} changed nothing");
        }
        // A newer catalog's settings on a Calendar are left out, as on a Chart.
        let Ok(Applied::Changed(sent)) = apply(&mut content, calendar(json!({"monthGrid": true}))) else { panic!("applied") };
        assert!(!sent.contains("monthGrid"));
    }

    /// What a Calendar's rows open is checked at every push, bound or not:
    /// only the four shapes. One naming an agent that's gone reaches devices
    /// as it is, so they can say the agent was deleted.
    #[test]
    fn a_calendars_rows_open_only_known_places_and_keep_an_agent_that_is_gone() {
        let message = |body: Value| body.to_string();
        let mut content = super::super::Content::default();
        let rows = |open: Value| json!({"columns": ["id", "title", "open"], "rows": [["a", "Logs", "monitor:lea:m1"], ["b", "Old", open]]});
        let create = json!({"version": "1.0", "createSurface": {"surfaceId": S, "components": [
            {"id": "root", "component": "Calendar", "data": {"@path": "/rows"}},
        ], "dataModel": {"rows": rows(json!("item:gone:4"))}}});
        assert_eq!(content.apply(S, &message(create)), Ok(Applied::Created));
        let snapshot: Value = serde_json::from_str(&content.snapshot(S)).unwrap();
        assert_eq!(snapshot.pointer("/createSurface/dataModel/rows/rows/1/2"), Some(&json!("item:gone:4")));
        assert_eq!(snapshot.pointer("/createSurface/dataModel/rows/rows/0/2"), Some(&json!("monitor:lea:m1")));

        let update = |open: Value| message(json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/rows", "value": rows(open)}}));
        assert_eq!(content.apply(S, &update(json!("task:lea:task-2"))), Ok(Applied::Changed(update(json!("task:lea:task-2")))));
        assert_eq!(content.apply(S, &update(json!("agent:gone"))), Ok(Applied::Changed(update(json!("agent:gone")))));

        for wrong in [
            json!("agent:"),
            json!("agent:lea:4"),
            json!("item:lea:x"),
            json!("item:lea:-4"),
            json!("item:lea"),
            json!("monitor:lea:mx"),
            json!("monitor:lea:m"),
            json!("task:lea:a b"),
            json!("task:lea/../x:1"),
            json!("file:/etc/passwd"),
            json!("https://example.com"),
            json!(4),
            json!({"agent": "lea"}),
        ] {
            let before = content.snapshot(S);
            let error = content.apply(S, &update(wrong.clone())).unwrap_err().to_string();
            assert!(error.contains("row 1 opens") && error.contains("isn't agent:<id>"), "{wrong}: {error}");
            assert_eq!(content.snapshot(S), before, "{wrong} changed nothing");
        }

        // Rows in the component itself, too.
        let literal = |open: Value| {
            message(json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
                {"id": "root", "component": "Calendar", "data": rows(open)},
            ]}}))
        };
        assert!(matches!(content.apply(S, &literal(json!("item:gone:1"))), Ok(Applied::Changed(_))));
        let snapshot: Value = serde_json::from_str(&content.snapshot(S)).unwrap();
        let root = snapshot["createSurface"]["components"].as_array().unwrap().iter().find(|c| c["id"] == "root").unwrap().clone();
        assert_eq!(root.pointer("/data/rows/1/2"), Some(&json!("item:gone:1")));
        assert!(content.apply(S, &literal(json!("item:lea:one"))).is_err());
        // Without an open column, nothing to check.
        let plain = message(json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "root", "component": "Calendar", "data": {"columns": ["id"], "rows": [["x"]]}},
        ]}}));
        assert!(matches!(content.apply(S, &plain), Ok(Applied::Changed(_))));
        // A column named twice, bound or literal: no second "open" past the check.
        let rebind = message(json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "root", "component": "Calendar", "data": {"@path": "/rows"}},
        ]}}));
        assert!(content.apply(S, &rebind).is_ok());
        let before = content.snapshot(S);
        let twice = json!({"columns": ["id", "open", "open"], "rows": [["a", "agent:lea", "https://example.com"]]});
        let bound = message(json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/rows", "value": twice}}));
        let literal = message(json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [
            {"id": "root", "component": "Calendar", "data": {"columns": ["id", "title", "id"], "rows": []}},
        ]}}));
        for wrong in [bound, literal] {
            let error = content.apply(S, &wrong).unwrap_err().to_string();
            assert!(error.contains("twice"), "{wrong}: {error}");
            assert_eq!(content.snapshot(S), before);
        }
    }

    #[test]
    fn what_a_newer_catalog_adds_to_charts_and_rows_is_left_out_and_never_reaches_a_device() {
        let rows = json!({"columns": ["t", "n"], "rows": [[1, 2]]});
        let mut content = created();
        let chart = json!({"id": "chart", "component": "Chart", "kind": "timeseries", "data": rows, "lineWidth": 2, "glow": true, "gradient": {"x": 1}});
        let stat = json!({"id": "stat", "component": "Stat", "label": "Runs", "value": 3, "textMode": "auto", "percentChange": {"x": 1}});
        let row = json!({"id": "row", "component": "Row", "children": ["chart", "stat"], "stackBelow": 768, "wrap": "x"});
        let table = json!({"id": "table", "component": "Table", "data": rows, "height": 200, "zebra": true});
        let message = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [row, chart, stat, table]}});
        let Ok(Applied::Changed(sent)) = apply(&mut content, message) else { panic!("applied") };
        let sent: Value = serde_json::from_str(&sent).unwrap();
        let kept = json!([
            {"id": "row", "component": "Row", "children": ["chart", "stat"], "stackBelow": 768},
            {"id": "chart", "component": "Chart", "kind": "timeseries", "data": rows, "lineWidth": 2},
            {"id": "stat", "component": "Stat", "label": "Runs", "value": 3},
            {"id": "table", "component": "Table", "data": rows, "height": 200},
        ]);
        assert_eq!(sent, json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": kept}}), "devices get what was kept");
        let snapshot: Value = serde_json::from_str(&content.snapshot(S)).unwrap();
        let components = snapshot["createSurface"]["components"].as_array().unwrap();
        for component in kept.as_array().unwrap() {
            assert!(components.contains(component), "{component} in {components:?}");
        }
        let text = snapshot.to_string();
        for key in ["glow", "gradient", "textMode", "percentChange", "wrap", "zebra"] {
            assert!(!text.contains(key), "{key} never reaches a device");
        }

        // A message with nothing left out goes on as it came.
        let plain = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "stat", "component": "Stat", "label": "Runs", "value": 4}]}});
        assert_eq!(apply(&mut content, plain.clone()), Ok(Applied::Changed(plain.to_string())));

        // Leaving out is only for plain keys on those: known keys are
        // still checked, required ones still needed, and "@" keys and every
        // other component's unknown keys still refused, changing nothing.
        let component = |component: Value| json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [component]}});
        for (wrong, why) in [
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "@call": "x"}), "has no property \"@call\""),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "glow": 1, "lineWidth": 20}), "from 0 to 10"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "glow": 1}), "Chart needs data"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": [1], "colorMode": "value"}), "a string or a number"),
            (json!({"id": "r", "component": "Row", "children": [], "stackBelow": 100}), "from 320 to 1200"),
            (json!({"id": "r", "component": "Row", "children": [], "stackBelow": {"@path": "/w"}}), "from 320 to 1200"),
            (json!({"id": "r", "component": "Row", "children": [], "@foo": 1}), "has no property \"@foo\""),
            (json!({"id": "c", "component": "Column", "children": [], "stackBelow": 768}), "has no property \"stackBelow\""),
            (json!({"id": "t", "component": "Text", "text": "a", "theme": "dark"}), "has no property \"theme\""),
            (json!({"id": "t", "component": "Table", "data": rows, "@zebra": true}), "has no property \"@zebra\""),
        ] {
            let before = content.snapshot(S);
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
            assert_eq!(content.snapshot(S), before, "{wrong} changed nothing");
        }
    }
}
