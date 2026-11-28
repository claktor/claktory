//! What a view shows: Claktory's Calendar component over the rows of the
//! days around its anchor, with the Agent, Show and ⋯ menus it places in
//! its bar. The component draws the days, moves between them and opens an
//! entry where it lives; each move comes back as the `calendar` action,
//! whose anchor and view the app keeps (the view per device) and draws
//! from.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::layers::{self, Layer, Look, Row};
use crate::store;
use crate::time::{self, DAY};
use crate::{CATALOG, Host, Level};

/// The rows a view gets: its anchor's days, this many each side.
pub const AROUND: i64 = 8 * DAY;

#[derive(Debug, Clone, Default)]
pub struct View {
    /// The device's stand-in, which keeps the view it chose.
    pub device: String,
    pub anchor: i64,
    /// day, week or agenda.
    pub view: String,
    /// Only this agent's entries; None for all.
    pub agent: Option<String>,
    pub created: bool,
    pub components: BTreeMap<String, Value>,
    pub data: Value,
    /// Nothing granted yet: it looks again soon.
    pub waiting: bool,
    /// Something every view shows changed.
    pub stale: bool,
}

impl View {
    pub fn new(device: String, view: String, now: i64) -> Self {
        Self { device, anchor: now, view, ..Self::default() }
    }
}

fn message(kind: &str, surface: &str, body: Value) -> Value {
    let mut body = body;
    body["surfaceId"] = json!(surface);
    json!({"version": "1.0", kind: body})
}

/// One of a menu's options.
fn option(label: &str, name: &str, context: Value, selected: bool, section: Option<&str>) -> Value {
    let mut option = json!({"label": label, "action": {"event": {"name": name, "context": context}}});
    if selected {
        option["selected"] = json!(true);
    }
    if let Some(section) = section {
        option["section"] = json!(section);
    }
    option
}

/// A button opening a menu: its words (or an icon alone) and a chevron.
fn menu_button(out: &mut Vec<Value>, id: &str, words: Option<&str>, icon: Option<&str>, title: &str, options: Vec<Value>) {
    out.push(json!({
        "id": id, "component": "Button", "child": format!("{id}_c"), "variant": "borderless",
        "action": {"event": {"name": "menu", "context": {"which": id}}},
        "menu": {"title": title, "options": options},
    }));
    let mut parts = Vec::new();
    if let Some(icon) = icon {
        parts.push(format!("{id}_l"));
        out.push(json!({"id": format!("{id}_l"), "component": "Icon", "name": icon}));
    }
    if let Some(words) = words {
        parts.extend([format!("{id}_t"), format!("{id}_i")]);
        out.push(json!({"id": format!("{id}_t"), "component": "Text", "text": words, "variant": "body"}));
        out.push(json!({"id": format!("{id}_i"), "component": "Icon", "name": "chevron-down"}));
    }
    out.push(json!({"id": format!("{id}_c"), "component": "List", "direction": "horizontal", "align": "center", "children": parts}));
}

/// The words over an empty range, for the view drawn: "today" and "this
/// week" only when the range holds today.
fn empty_title(view: &str, anchor_day: i64, today: i64, week_start: Option<&str>) -> String {
    let first = |day: i64| {
        let start = match week_start {
            Some("sunday") => 6,
            Some("saturday") => 5,
            _ => 0,
        };
        day - i64::from((time::weekday(day) + 7 - start) % 7)
    };
    match view {
        "day" if anchor_day == today => "Nothing on the calendar today".to_owned(),
        "day" => format!("Nothing on the calendar on {} {}", time::WEEKDAY_NAMES[time::weekday(anchor_day) as usize], time::short_date(anchor_day)),
        "agenda" if anchor_day == today => "Nothing on the calendar coming up".to_owned(),
        "agenda" => format!("Nothing on the calendar from {} on", time::short_date(anchor_day)),
        _ if first(anchor_day) == first(today) => "Nothing on the calendar this week".to_owned(),
        _ => "Nothing on the calendar that week".to_owned(),
    }
}

const HIDDEN_TITLE: &str = "Everything is hidden";
const HIDDEN_BODY: &str = "Turn something on in Show to see it here.";

const EMPTY_BODY: &str = "Your agents' monitors, deadlines and releases show here. Ask an agent to add something.";

/// The components and data of `open` now.
fn scene(host: &mut impl Host, open: &mut View) -> (Vec<Value>, Value) {
    let sources = host.data_sources();
    let Some(source) = layers::source(&sources) else {
        open.waiting = true;
        let components = vec![
            json!({"id": "root", "component": "Column", "children": ["ask"]}),
            json!({
                "id": "ask", "component": "EmptyState", "icon": "calendar",
                "title": "Calendar can't see anything yet",
                "body": "Let it read your agents' monitors, checklists and tasks to see them here. Ask Rivet, or choose what it reads in Marketplace → Apps.",
            }),
        ];
        return (components, json!({}));
    };
    open.waiting = false;
    let now = host.now_ms();
    let (from, to) = (open.anchor - AROUND, open.anchor + AROUND);
    let zone = layers::zone(host, Some(source), from, to).unwrap_or_default();
    let agents = layers::agents(host, source).unwrap_or_default();
    let prefs = store::prefs(host);
    let shown: Vec<Layer> = Layer::ALL.into_iter().filter(|layer| !prefs.hidden.iter().any(|hidden| hidden == layer.id())).collect();
    let events = store::in_range(host, from, to);
    let calendars = store::calendars(host);
    let look = Look { source, zone: &zone, agents: &agents, from, to, now };
    let rows: Vec<Row> = match layers::all(host, &look, &shown, &events, &calendars) {
        Ok(rows) => rows,
        Err(why) => {
            host.log(Level::Warn, &format!("Couldn't read the calendar: {why}"));
            layers::placed(&events, &calendars, &look)
        }
    };
    let rows: Vec<Value> = rows.iter().filter(|row| open.agent.as_ref().is_none_or(|agent| row.agent == *agent)).map(Row::cells).collect();

    let mut components = Vec::new();
    // Agent ▾: all, or one, under its project's heading when projects are known.
    let mut agent_options = vec![option("All agents", "agent", json!({"id": ""}), open.agent.is_none(), None)];
    let mut listed: Vec<(Option<String>, String, String)> = agents
        .names
        .iter()
        .map(|(id, name)| {
            let project = agents.projects.as_ref().and_then(|projects| projects.get(id).cloned().flatten());
            (project.map(|project| agents.project_names.get(&project).cloned().unwrap_or(project)), name.clone(), id.clone())
        })
        .collect();
    listed.sort_by(|a, b| (a.0.is_none(), &a.0, a.1.to_lowercase()).cmp(&(b.0.is_none(), &b.0, b.1.to_lowercase())));
    for (project, name, id) in &listed {
        let section = agents.projects.as_ref().map(|_| project.as_deref().unwrap_or("Workspace"));
        agent_options.push(option(name, "agent", json!({"id": id}), open.agent.as_deref() == Some(id), section));
    }
    let agent_words = match &open.agent {
        Some(id) => format!("Agent: {}", agents.name(id)),
        None => "Agent: All".to_owned(),
    };
    menu_button(&mut components, "agent_menu", Some(&agent_words), None, "Agent", agent_options);
    // Show ▾: each layer on or off, for every device.
    let show_options = Layer::ALL
        .iter()
        .map(|layer| {
            option(layer.words(), "layer", json!({"layer": layer.id(), "on": !shown.contains(layer)}), shown.contains(layer), None)
        })
        .collect();
    let show_words = match shown.len() {
        4 => "Show: All".to_owned(),
        0 => "Show: Nothing".to_owned(),
        _ => format!("Show: {}", shown.iter().map(|layer| layer.words()).collect::<Vec<_>>().join(", ")),
    };
    menu_button(&mut components, "show_menu", Some(&show_words), None, "Show", show_options);
    // ⋯: Today, what's shown (a narrow view has no Show ▾), and the
    // week's first day.
    let mut more = vec![option("Today", "today", json!({}), false, None)];
    for layer in &Layer::ALL {
        more.push(option(layer.words(), "layer", json!({"layer": layer.id(), "on": !shown.contains(layer)}), shown.contains(layer), Some("Show")));
    }
    for (day, words) in [("", "Same as this device"), ("monday", "Monday"), ("sunday", "Sunday"), ("saturday", "Saturday")] {
        let chosen = prefs.week_start.as_deref().unwrap_or_default() == day;
        more.push(option(words, "week_start", json!({"day": day}), chosen, Some("Week starts on")));
    }
    menu_button(&mut components, "more_menu", None, Some("ellipsis"), "Calendar", more);

    let mut calendar = json!({
        "id": "cal", "component": "Calendar",
        "data": {"@path": "/rows"},
        "view": open.view,
        "anchor": open.anchor,
        "tools": ["agent_menu", "show_menu", "more_menu"],
        "emptyTitle": if shown.is_empty() { HIDDEN_TITLE.to_owned() } else { empty_title(&open.view, zone.day(open.anchor), zone.day(now), prefs.week_start.as_deref()) },
        "agendaEmptyTitle": if shown.is_empty() { HIDDEN_TITLE.to_owned() } else { empty_title("agenda", zone.day(open.anchor), zone.day(now), None) },
        "emptyBody": if shown.is_empty() { HIDDEN_BODY } else { EMPTY_BODY },
        "action": {"event": {"name": "calendar"}},
    });
    if let Some(week_start) = &prefs.week_start {
        calendar["weekStart"] = json!(week_start);
    }
    if zone.is_known() {
        let span = match open.view.as_str() {
            "day" => (open.anchor - DAY / 2, open.anchor + DAY / 2),
            _ => (open.anchor - 7 * DAY, open.anchor + 7 * DAY),
        };
        calendar["macZone"] = json!({"start": zone.offset_at(span.0), "end": zone.offset_at(span.1)});
    }
    components.insert(0, calendar);
    components.insert(0, json!({"id": "root", "component": "Column", "children": ["cal"]}));
    (components, json!({"rows": {"columns": layers::COLUMNS, "rows": rows}}))
}

/// Draws `open` again, pushing only what changed, and keeps where its
/// device is.
pub fn show(host: &mut impl Host, surface: &str, open: &mut View) -> Result<(), String> {
    open.stale = false;
    let at = host.now_ms();
    let state = store::DeviceState { view: Some(open.view.clone()), agent: open.agent.clone(), anchor: open.anchor, at };
    if let Err(why) = store::save_device_state(host, &open.device, &state) {
        host.log(Level::Warn, &format!("Couldn't keep where this device is: {why}"));
    }
    let (components, data) = scene(host, open);
    if !open.created {
        host.push_view(
            surface,
            &message("createSurface", surface, json!({"catalogId": CATALOG, "components": components, "dataModel": data})),
        )?;
        open.created = true;
        open.components =
            components.into_iter().map(|component| (component["id"].as_str().unwrap_or_default().to_owned(), component)).collect();
        open.data = data;
        return Ok(());
    }
    if open.data.get("rows") != data.get("rows") {
        host.push_view(
            surface,
            &message("updateDataModel", surface, json!({"path": "/rows", "value": data.get("rows").cloned().unwrap_or(Value::Null)})),
        )?;
    }
    let changed: Vec<Value> = components
        .iter()
        .filter(|component| open.components.get(component["id"].as_str().unwrap_or_default()) != Some(*component))
        .cloned()
        .collect();
    if !changed.is_empty() {
        host.push_view(surface, &message("updateComponents", surface, json!({"components": changed})))?;
    }
    open.components =
        components.into_iter().map(|component| (component["id"].as_str().unwrap_or_default().to_owned(), component)).collect();
    open.data = data;
    Ok(())
}

/// What an action changed: nothing, this view, or what every view shows.
pub enum Acted {
    Nothing,
    This,
    Everyone,
}

/// Something the person did in `open`.
pub fn act(host: &mut impl Host, open: &mut View, name: &str, context: &Value, now: i64) -> Acted {
    let text = |key: &str| context[key].as_str().unwrap_or_default().to_owned();
    match name {
        // A move within the calendar: a swipe, ‹ ›, Today, the view switch.
        "calendar" => {
            if let Some(anchor) = context["anchor"].as_i64().filter(|anchor| *anchor >= 0) {
                open.anchor = anchor;
            }
            let view = text("view");
            if store::VIEWS.contains(&view.as_str()) {
                open.view = view;
            }
            Acted::This
        }
        "today" => {
            open.anchor = now;
            Acted::This
        }
        "agent" => {
            let id = text("id");
            open.agent = if id.is_empty() { None } else { Some(id) };
            Acted::This
        }
        "layer" => {
            let Some(layer) = Layer::of(&text("layer")) else { return Acted::Nothing };
            let mut prefs = store::prefs(host);
            prefs.hidden.retain(|hidden| hidden != layer.id());
            if !context["on"].as_bool().unwrap_or(true) {
                prefs.hidden.push(layer.id().to_owned());
            }
            keep(host, &prefs)
        }
        "week_start" => {
            let day = text("day");
            let mut prefs = store::prefs(host);
            prefs.week_start = store::WEEK_STARTS.contains(&day.as_str()).then_some(day);
            keep(host, &prefs)
        }
        // The menus open over the page; their buttons' own action is
        // only for a renderer without menus.
        _ => Acted::Nothing,
    }
}

fn keep(host: &mut impl Host, prefs: &store::Prefs) -> Acted {
    match store::save_prefs(host, prefs) {
        Ok(()) => Acted::Everyone,
        Err(why) => {
            host.log(Level::Warn, &format!("Couldn't keep that: {why}"));
            Acted::This
        }
    }
}
