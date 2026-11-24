//! The chart family of Claktory's A2UI catalog: Chart, Stat and Table, the
//! kinds of value only they take (a query's answer, series, a spark, a
//! Stat's color, a Table's columns) and their rules across properties.
//! `a2ui.rs` checks everything else and asks here for these.

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

/// The charts' palette by name (#380): the renderer gives each the
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
            ("kind", Choice(&["timeseries", "bars", "stackedBars", "pie", "scatter"]), true),
            ("data", TABLE, true),
            ("x", Text, false),
            ("y", SERIES, false),
            ("seriesBy", Text, false),
            ("color", Choice(&["palette", "agent", "signal", "muted"]), false),
            ("label", DynString, false),
            ("unit", DynString, false),
            ("height", super::Kind::Chart(Kind::Range(120.0, 480.0)), false),
            // How it draws (#380), each a plain value; left out, the
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
            // The time axis's ends, in milliseconds since 1970 (#380): the
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
            // How it draws (#380), each a plain value; left out, the
            // renderer's default: green, red from 80, the value colored,
            // a spark line under it, automatic decimals, its own height.
            ("thresholds", super::Kind::Chart(Kind::Thresholds), false),
            ("colorMode", Choice(&["value", "background", "solid", "none"]), false),
            ("graph", Choice(&["area", "none"]), false),
            ("decimals", super::Kind::Chart(Kind::Whole(0, 10)), false),
            ("height", super::Kind::Chart(Kind::Range(60.0, 480.0)), false),
        ],
        "Table" => &[("data", TABLE, true), ("columns", COLUMNS, false)],
        _ => return None,
    })
}

/// Whether a component's unknown plain keys are left out rather than
/// refused (#380): the chart family's presentation (and Row's), which the
/// Charts app may set for a newer catalog than this one, after a roll-back
/// say. What's left out never reaches a device; known keys are still checked.
pub(super) fn lenient(component: &str) -> bool {
    matches!(component, "Chart" | "Stat" | "Row")
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
    }
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
        ] {
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
        }
    }

    #[test]
    fn what_a_newer_catalog_adds_to_charts_and_rows_is_left_out_and_never_reaches_a_device() {
        let rows = json!({"columns": ["t", "n"], "rows": [[1, 2]]});
        let mut content = created();
        let chart = json!({"id": "chart", "component": "Chart", "kind": "timeseries", "data": rows, "lineWidth": 2, "glow": true, "gradient": {"x": 1}});
        let stat = json!({"id": "stat", "component": "Stat", "label": "Runs", "value": 3, "textMode": "auto", "percentChange": {"x": 1}});
        let row = json!({"id": "row", "component": "Row", "children": ["chart", "stat"], "stackBelow": 768, "wrap": "x"});
        let message = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [row, chart, stat]}});
        let Ok(Applied::Changed(sent)) = apply(&mut content, message) else { panic!("applied") };
        let sent: Value = serde_json::from_str(&sent).unwrap();
        let kept = json!([
            {"id": "row", "component": "Row", "children": ["chart", "stat"], "stackBelow": 768},
            {"id": "chart", "component": "Chart", "kind": "timeseries", "data": rows, "lineWidth": 2},
            {"id": "stat", "component": "Stat", "label": "Runs", "value": 3},
        ]);
        assert_eq!(sent, json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": kept}}), "devices get what was kept");
        let snapshot: Value = serde_json::from_str(&content.snapshot(S)).unwrap();
        let components = snapshot["createSurface"]["components"].as_array().unwrap();
        for component in kept.as_array().unwrap() {
            assert!(components.contains(component), "{component} in {components:?}");
        }
        let text = snapshot.to_string();
        for key in ["glow", "gradient", "textMode", "percentChange", "wrap"] {
            assert!(!text.contains(key), "{key} never reaches a device");
        }

        // A message with nothing left out goes on as it came.
        let plain = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "stat", "component": "Stat", "label": "Runs", "value": 4}]}});
        assert_eq!(apply(&mut content, plain.clone()), Ok(Applied::Changed(plain.to_string())));

        // Leaving out is only for plain keys on those three: known keys are
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
            (json!({"id": "t", "component": "Table", "data": rows, "zebra": true}), "has no property \"zebra\""),
        ] {
            let before = content.snapshot(S);
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
            assert_eq!(content.snapshot(S), before, "{wrong} changed nothing");
        }
    }
}
