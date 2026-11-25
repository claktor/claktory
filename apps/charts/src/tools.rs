//! The tools agents use, as the person grants them: `view` gives
//! charts_list and charts_get; `edit` also charts_put, charts_delete and
//! charts_query, which reads whatever Charts can read.

use serde_json::{Value, json};

use crate::model::{self, Dashboard};
use crate::view::{self, Outcome, View};
use crate::{App, DataError, Host, sql};

/// The most rows charts_query answers.
pub const QUERY_ROWS: usize = 200;
/// The longest a cell reads in charts_query's text.
const CELL_CHARS: usize = 200;

/// What a tool answers: text for the agent, data (JSON) beside it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub data: Option<Value>,
    pub is_error: bool,
}

fn answer(text: impl Into<String>, data: Value) -> ToolOutput {
    ToolOutput { text: text.into(), data: Some(data), is_error: false }
}

fn refuse(text: impl Into<String>) -> ToolOutput {
    ToolOutput { text: text.into(), data: None, is_error: true }
}

impl App {
    /// An agent (`caller`, its id) called `tool` with `args` (JSON).
    pub fn call(&mut self, host: &mut impl Host, tool: &str, args: &str, caller: &str) -> ToolOutput {
        let args: Value = match serde_json::from_str(if args.trim().is_empty() { "{}" } else { args }) {
            Ok(args @ Value::Object(_)) => args,
            _ => return refuse("The arguments aren't a JSON object."),
        };
        match tool {
            "charts_list" => list(host),
            "charts_get" => get(host, &args),
            "charts_put" => {
                let output = put(host, &args, caller);
                if !output.is_error {
                    self.touch(host);
                }
                output
            }
            "charts_delete" => {
                let output = delete(host, &args);
                if !output.is_error {
                    self.touch(host);
                }
                output
            }
            "charts_query" => query(host, &args),
            other => refuse(format!("Charts has no tool \"{other}\".")),
        }
    }
}

/// charts_list: the dashboards and the sources.
fn list(host: &mut impl Host) -> ToolOutput {
    let mut lines = vec!["Dashboards:".to_owned()];
    let mut dashboards = Vec::new();
    for id in model::ids(host) {
        let Some(dashboard) = model::load(host, &id) else { continue };
        let variables: Vec<&str> = dashboard.variables.iter().map(|variable| variable.name.as_str()).collect();
        lines.push(format!(
            "- {} \"{}\": {} panels{}, _rev {}",
            dashboard.id,
            dashboard.name,
            dashboard.panels.len(),
            if variables.is_empty() { String::new() } else { format!(", variables ${}", variables.join(", $")) },
            dashboard.rev
        ));
        dashboards.push(json!({
            "id": dashboard.id, "name": dashboard.name, "panels": dashboard.panels.len(),
            "variables": variables, "_rev": dashboard.rev,
        }));
    }
    if dashboards.is_empty() {
        lines.push("(none)".into());
    } else {
        lines.push(arrangement(host));
    }
    lines.push("Sources (only granted ones can be read):".into());
    let sources: Vec<Value> = host
        .data_sources()
        .into_iter()
        .map(|source| {
            lines.push(format!(
                "- {} ({}): {}",
                source.id,
                source.label,
                if source.granted { "granted" } else { "not granted: the person gives it to Charts (Rivet proposes it with apps_propose_access)" }
            ));
            json!({"id": source.id, "label": source.label, "granted": source.granted})
        })
        .collect();
    let prefs = model::prefs(host);
    let opens = model::opening(host);
    answer(lines.join("\n"), json!({"dashboards": dashboards, "sources": sources, "home": opens, "starred": prefs.starred}))
}

/// charts_get: one dashboard's definition, or its previous version.
fn get(host: &mut impl Host, args: &Value) -> ToolOutput {
    let Some(id) = args["id"].as_str() else { return refuse("Say which dashboard: {\"id\": \"…\"} (charts_list lists them).") };
    let previous = args["previous"].as_bool().unwrap_or(false);
    let found = if previous { model::previous(host, id) } else { model::load(host, id) };
    let Some(dashboard) = found else {
        return refuse(if previous {
            format!("\"{id}\" has no previous version.")
        } else {
            format!("There's no dashboard \"{id}\"; charts_list lists them.")
        });
    };
    let mut data = serde_json::to_value(&dashboard).unwrap_or_default();
    // How it's kept is Charts' own business.
    if let Some(fields) = data.as_object_mut() {
        fields.remove("_v");
    }
    answer(serde_json::to_string_pretty(&data).unwrap_or_default(), data)
}

/// charts_put: a whole dashboard, new or changed, checked first. Its
/// panels' queries run once, so the agent hears which fail.
fn put(host: &mut impl Host, args: &Value, caller: &str) -> ToolOutput {
    let arranging = ARRANGE.iter().any(|key| args.get(*key).is_some());
    // {"dashboard": {…}}, or the dashboard itself; or only how they're
    // arranged.
    let given = match args.get("dashboard") {
        Some(dashboard) => dashboard.clone(),
        None if args.get("id").is_some() => args.clone(),
        None if arranging => {
            return match arrange(host, args) {
                Ok(text) => answer(text, json!({"arranged": true})),
                Err(why) => refuse(format!("Nothing was changed: {why}.")),
            };
        }
        None => return refuse("Send the whole dashboard: {\"dashboard\": {\"id\", \"name\", \"panels\", …}}, or home, starred or order."),
    };
    let base = given.get("_rev").and_then(Value::as_u64);
    let mut fields = given.clone();
    if let Some(fields) = fields.as_object_mut() {
        fields.remove("_by");
    }
    let mut dashboard: Dashboard = match serde_json::from_value(fields) {
        Ok(dashboard) => dashboard,
        Err(error) => return refuse(format!("The dashboard can't be read: {error}. charts_get shows one as it should be.")),
    };
    // A stat that doesn't say shows what it showed when kept (current
    // version first, then the one before, as when it's brought back): a
    // stat kept before reduce existed its total; a new one its last number.
    let kept: Vec<(String, Option<String>)> = [model::load(host, &dashboard.id), model::previous(host, &dashboard.id)]
        .into_iter()
        .flatten()
        .flat_map(|kept| kept.panels.into_iter().filter(|panel| panel.viz.kind == "stat").map(|panel| (panel.id, panel.viz.reduce)))
        .collect();
    for panel in &mut dashboard.panels {
        if panel.viz.kind == "stat" && panel.viz.reduce.is_none() {
            panel.viz.reduce = match kept.iter().find(|(id, _)| *id == panel.id) {
                Some((_, reduce)) => reduce.clone(),
                None => Some(model::NEW_REDUCE.into()),
            };
        }
    }
    let sources = host.data_sources();
    let problems = dashboard.problems(&sources);
    if !problems.is_empty() {
        return refuse(format!("Nothing was kept. Fix these, then put it again:\n- {}", problems.join("\n- ")));
    }
    let saved = match model::save(host, dashboard, base, caller) {
        Ok(saved) => saved,
        Err(error) => return refuse(format!("Nothing was kept: {error}.")),
    };
    // Each panel's query once, as the view would run it first.
    let now = host.now_ms();
    let values = view::values(&View::default(), &saved, now);
    let mut failed = Vec::new();
    let mut waiting = Vec::new();
    for panel in &saved.panels {
        match view::query(host, &panel.query, &values, &sources) {
            Outcome::Table(_) => {}
            Outcome::Failed(why) => failed.push(json!({"panel": panel.id, "error": why})),
            Outcome::NotGranted(label) => waiting.push(json!({"panel": panel.id, "source": label})),
        }
    }
    let mut text = format!("Kept \"{}\" ({}) as _rev {}", saved.name, saved.id, saved.rev);
    // A new dashboard has no version before it to speak of.
    text.push_str(if model::previous(host, &saved.id).is_some() { "; the version before it is kept too." } else { "." });
    for failure in &failed {
        text.push_str(&format!(
            "\nPanel {}'s query failed: {}",
            failure["panel"].as_str().unwrap_or_default(),
            failure["error"].as_str().unwrap_or_default()
        ));
    }
    for wait in &waiting {
        text.push_str(&format!(
            "\nPanel {} shows nothing until the person gives Charts {} (Rivet proposes it with apps_propose_access).",
            wait["panel"].as_str().unwrap_or_default(),
            wait["source"].as_str().unwrap_or_default()
        ));
    }
    if arranging {
        match arrange(host, args) {
            Ok(arranged) => text.push_str(&format!("\n{arranged}")),
            Err(why) => text.push_str(&format!("\nThe arrangement wasn't changed: {why}.")),
        }
    }
    answer(text, json!({"id": saved.id, "_rev": saved.rev, "failed": failed, "not_granted": waiting}))
}

/// What charts_put takes besides a dashboard: how the person's dashboards
/// are arranged.
const ARRANGE: [&str; 3] = ["home", "starred", "order"];

/// Sets the home dashboard ("" for none: the first), the starred ones (all
/// of them) and the order (those named first), each only when given.
fn arrange(host: &mut impl Host, args: &Value) -> Result<String, String> {
    let ids = model::ids(host);
    let list = |key: &str| -> Result<Option<Vec<String>>, String> {
        let Some(value) = args.get(key) else { return Ok(None) };
        let names: Option<Vec<String>> = value.as_array().and_then(|items| items.iter().map(|item| item.as_str().map(str::to_owned)).collect());
        let names = names.ok_or_else(|| format!("{key} is a list of dashboard ids"))?;
        match names.iter().find(|id| !ids.contains(id)) {
            Some(id) => Err(format!("there's no dashboard \"{id}\" (charts_list lists them)")),
            None => Ok(Some(names)),
        }
    };
    let (starred, order) = (list("starred")?, list("order")?);
    let home = match args.get("home") {
        None => None,
        Some(Value::String(id)) if id.is_empty() => Some(None),
        Some(Value::String(id)) if ids.contains(id) => Some(Some(id.clone())),
        Some(Value::String(id)) => return Err(format!("there's no dashboard \"{id}\" (charts_list lists them)")),
        Some(_) => return Err("home is a dashboard's id, or \"\" for none".into()),
    };
    let mut prefs = model::prefs(host);
    if let Some(home) = home {
        prefs.home = home;
    }
    if let Some(starred) = starred {
        prefs.starred = Vec::new();
        for id in starred {
            if !prefs.starred.contains(&id) {
                prefs.starred.push(id);
            }
        }
    }
    model::save_prefs(host, &prefs)?;
    if let Some(order) = order {
        model::set_order(host, &order)?;
    }
    Ok(arrangement(host))
}

/// How the dashboards are arranged, in a line.
fn arrangement(host: &mut impl Host) -> String {
    let prefs = model::prefs(host);
    let order = model::menu_order(&model::ids(host), &prefs);
    let opens = model::opening(host);
    let starred = if prefs.starred.is_empty() { "none".to_owned() } else { prefs.starred.join(", ") };
    format!("Charts opens on {opens}; starred: {starred}; the dashboards' menu lists {}.", order.join(", "))
}

/// charts_delete: a dashboard, its last version kept.
fn delete(host: &mut impl Host, args: &Value) -> ToolOutput {
    let Some(id) = args["id"].as_str() else { return refuse("Say which dashboard: {\"id\": \"…\"}.") };
    match model::delete(host, id) {
        Ok(gone) => answer(
            format!(
                "Deleted \"{}\" ({id}). Its last version is kept: charts_get with previous: true, then charts_put, brings it back.",
                gone.name
            ),
            json!({"id": id, "_rev": gone.rev}),
        ),
        Err(why) => refuse(format!("Nothing was deleted: {why}.")),
    }
}

/// charts_query: one read-only statement against a granted source, bound
/// like a panel's, at most [`QUERY_ROWS`] rows.
fn query(host: &mut impl Host, args: &Value) -> ToolOutput {
    let (Some(source), Some(text)) = (args["source"].as_str(), args["sql"].as_str()) else {
        return refuse("Send {\"source\": \"…\", \"sql\": \"SELECT …\"} (charts_list lists the sources).");
    };
    let now = host.now_ms();
    let from_text = args["from"].as_str().unwrap_or("now-7d");
    let to_text = args["to"].as_str().unwrap_or("now");
    let (Some(from), Some(to)) = (sql::time(from_text, now), sql::time(to_text, now)) else {
        return refuse("from and to are \"now\", \"now-<n><m|h|d|w|y>\" (such as \"now-7d\") or milliseconds.");
    };
    if from >= to {
        return refuse("from is before to.");
    }
    let mut values = sql::builtins(from, to, now);
    let mut known: Vec<String> = sql::BUILTINS.iter().map(|name| (*name).to_owned()).collect();
    // A dashboard's variables, at their defaults, so a panel's SQL runs as
    // it is.
    if let Some(id) = args["dashboard"].as_str() {
        let Some(dashboard) = model::load(host, id) else {
            return refuse(format!("There's no dashboard \"{id}\"; charts_list lists them."));
        };
        for variable in dashboard.variables {
            values.insert(variable.name.clone(), json!(variable.default));
            known.push(variable.name);
        }
    }
    if let Some(variables) = args["variables"].as_object() {
        for (name, value) in variables {
            if sql::BUILTINS.contains(&name.as_str()) {
                return refuse(format!("${name} is built in; it can't be a variable."));
            }
            let value = match value {
                Value::String(_) | Value::Number(_) | Value::Bool(_) | Value::Null => value.clone(),
                _ => return refuse(format!("${name}'s value is a string, a number, true/false or null.")),
            };
            values.insert(name.clone(), value);
            known.push(name.clone());
        }
    }
    let known: Vec<&str> = known.iter().map(String::as_str).collect();
    if let Err(why) = sql::check(text, &known) {
        return refuse(format!(
            "The query: {why}. Give a dashboard's variables their values with \"dashboard\": \"<its id>\" (each at its default, \"\" for All) or \"variables\": {{\"name\": value}}."
        ));
    }
    let bound = match sql::bind(text, &values) {
        Ok(bound) => bound,
        Err(why) => return refuse(format!("The query: {why}.")),
    };
    let sources = host.data_sources();
    let source = crate::source_for(source, &sources);
    match sources.iter().find(|known| known.id == source) {
        None => {
            let ids: Vec<&str> = sources.iter().map(|known| known.id.as_str()).collect();
            return refuse(format!("Charts doesn't read \"{source}\"; it reads {}.", ids.join(", ")));
        }
        Some(found) if !found.granted => {
            return refuse(format!("The person hasn't given Charts {}, so it can't be read (Rivet proposes it with apps_propose_access).", found.label));
        }
        Some(_) => {}
    }
    let table = match host.data_query(source, &bound.sql, &Value::Array(bound.params)) {
        Ok(table) => table,
        Err(DataError::NotGranted) => return refuse("The person hasn't given Charts that source."),
        Err(DataError::UnknownSource) => return refuse(format!("Charts can't read \"{source}\".")),
        Err(DataError::Invalid(why) | DataError::Failed(why)) => return refuse(format!("The query failed: {why}")),
    };
    let table = view::truncate(&table, QUERY_ROWS);
    answer(tsv(&table), table)
}

/// A table as tab-separated lines, its columns first.
fn tsv(table: &Value) -> String {
    let cell = |value: &Value| {
        let text = match value {
            Value::String(text) => text.clone(),
            Value::Null => String::new(),
            other => other.to_string(),
        };
        let text: String = text.chars().map(|c| if c == '\t' || c == '\n' || c == '\r' { ' ' } else { c }).collect();
        if text.chars().count() > CELL_CHARS { format!("{}…", text.chars().take(CELL_CHARS).collect::<String>()) } else { text }
    };
    let mut lines = Vec::new();
    let columns: Vec<String> = table["columns"].as_array().into_iter().flatten().map(cell).collect();
    lines.push(columns.join("\t"));
    let rows = table["rows"].as_array().cloned().unwrap_or_default();
    for row in &rows {
        lines.push(row.as_array().into_iter().flatten().map(cell).collect::<Vec<_>>().join("\t"));
    }
    let count = rows.len();
    if table["more"].as_bool().unwrap_or(false) {
        lines.push(format!("(the first {count} rows; there are more)"));
    } else {
        lines.push(if count == 1 { "(1 row)".to_owned() } else { format!("({count} rows)") });
    }
    lines.join("\n")
}
