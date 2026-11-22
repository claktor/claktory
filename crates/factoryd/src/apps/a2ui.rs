//! What an app's view shows: A2UI v1.0 messages
//! (https://a2ui.org/specification/v1.0-a2ui/) checked against Claktory's
//! own catalog before any app sees them, and folded into the surface's
//! content (its components and data model as last applied), which a device
//! opening the view gets whole.
//!
//! The catalog is a subset of A2UI's basic one, with its property names,
//! plus ProgressBar and EmptyState. Dynamic values are literals or
//! `{"@path": …}` bindings; functions (`@call`), checks, function-call
//! actions, media, Modal, `sendDataModel` and `theme` aren't in v1.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{Map, Value, json};

/// Claktory's catalog: the only one surfaces use.
pub(crate) const CATALOG: &str = "https://claktory.com/a2ui/catalog/v1";
/// The largest message an app sends.
pub(crate) const MAX_MESSAGE: usize = 1 << 20;
/// The most a surface's components and data model weigh together, as JSON.
pub(crate) const MAX_CONTENT: usize = 4 << 20;
pub(crate) const MAX_COMPONENTS: usize = 1000;
/// The largest message a device sends about a surface.
pub(crate) const MAX_RENDERER_MESSAGE: usize = 64 << 10;
/// How deep JSON nests in a message.
const MAX_DEPTH: usize = 32;
const MAX_CHILDREN: usize = 500;
const MAX_OPTIONS: usize = 200;
const MAX_TABS: usize = 20;
const MAX_ID: usize = 64;
const MAX_PATH: usize = 1024;

/// Why a message can't be used, as A2UI says it: a code, where (a JSON
/// pointer into the message) and a sentence.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Invalid {
    pub code: &'static str,
    pub path: String,
    pub message: String,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.code, self.message)
        } else {
            write!(f, "{} at {}: {}", self.code, self.path, self.message)
        }
    }
}

fn invalid(path: impl Into<String>, message: impl Into<String>) -> Invalid {
    Invalid { code: "VALIDATION_FAILED", path: path.into(), message: message.into() }
}

fn integrity(message: impl Into<String>) -> Invalid {
    Invalid { code: "INTEGRITY_ERROR", path: String::new(), message: message.into() }
}

/// What applying a message did.
#[derive(Debug, PartialEq)]
pub(crate) enum Applied {
    /// createSurface: the content is new; the snapshot says it all.
    Created,
    /// A change, as clients get it (the message, compact).
    Changed(String),
    /// deleteSurface: the app closed it.
    Deleted,
}

/// A surface's content as last applied.
#[derive(Debug, Default, Clone)]
pub(crate) struct Content {
    created: bool,
    components: BTreeMap<String, Value>,
    data: Value,
}

impl Content {
    pub(crate) fn created(&self) -> bool {
        self.created
    }

    /// Checks one of the app's messages for `surface` and applies it; an
    /// invalid one changes nothing.
    pub(crate) fn apply(&mut self, surface: &str, text: &str) -> Result<Applied, Invalid> {
        if text.len() > MAX_MESSAGE {
            return Err(invalid("", format!("the message is {} bytes; at most {MAX_MESSAGE}", text.len())));
        }
        let message: Value = serde_json::from_str(text).map_err(|error| invalid("", format!("it isn't JSON: {error}")))?;
        depth(&message, "")?;
        let (kind, body) = envelope(&message, &["createSurface", "updateComponents", "updateDataModel", "deleteSurface"])?;
        let at = format!("/{kind}");
        match body.get("surfaceId") {
            Some(Value::String(id)) if id == surface => {}
            Some(Value::String(id)) => {
                return Err(invalid(format!("{at}/surfaceId"), format!("\"{id}\" isn't this surface, \"{surface}\"")));
            }
            _ => return Err(invalid(format!("{at}/surfaceId"), "surfaceId is required")),
        }
        if kind != "createSurface" && !self.created {
            return Err(integrity("createSurface comes first"));
        }
        let mut next = self.clone();
        let applied = match kind {
            "createSurface" => {
                if self.created {
                    return Err(integrity("the surface was created already"));
                }
                only(body, &at, &["surfaceId", "catalogId", "components", "dataModel", "sendDataModel", "metadata"])?;
                match body.get("catalogId") {
                    None => {}
                    Some(Value::String(catalog)) if catalog == CATALOG => {}
                    Some(_) => return Err(invalid(format!("{at}/catalogId"), format!("the catalog is \"{CATALOG}\""))),
                }
                if body.get("sendDataModel").is_some_and(|send| send != &Value::Bool(false)) {
                    return Err(invalid(
                        format!("{at}/sendDataModel"),
                        "sendDataModel isn't supported: put the values an action needs in its context",
                    ));
                }
                if body.get("metadata").is_some_and(|metadata| !metadata.is_object()) {
                    return Err(invalid(format!("{at}/metadata"), "metadata is an object"));
                }
                next.created = true;
                next.data = match body.get("dataModel") {
                    None => json!({}),
                    Some(data @ Value::Object(_)) => data.clone(),
                    Some(_) => return Err(invalid(format!("{at}/dataModel"), "the data model is an object")),
                };
                if let Some(components) = body.get("components") {
                    next.put_components(components, &format!("{at}/components"))?;
                }
                Applied::Created
            }
            "updateComponents" => {
                only(body, &at, &["surfaceId", "components"])?;
                let components = body.get("components").ok_or_else(|| invalid(format!("{at}/components"), "components is required"))?;
                next.put_components(components, &format!("{at}/components"))?;
                Applied::Changed(message.to_string())
            }
            "updateDataModel" => {
                only(body, &at, &["surfaceId", "path", "value"])?;
                let path = match body.get("path") {
                    None => "/",
                    Some(Value::String(path)) => path.as_str(),
                    Some(_) => return Err(invalid(format!("{at}/path"), "path is a JSON pointer")),
                };
                let value = body.get("value").ok_or_else(|| invalid(format!("{at}/value"), "value is required (null deletes)"))?;
                set(&mut next.data, path, value.clone()).map_err(|why| invalid(format!("{at}/path"), why))?;
                Applied::Changed(message.to_string())
            }
            _ => {
                only(body, &at, &["surfaceId"])?;
                Applied::Deleted
            }
        };
        if applied != Applied::Deleted {
            let weight: usize =
                next.components.values().map(|component| component.to_string().len()).sum::<usize>() + next.data.to_string().len();
            if weight > MAX_CONTENT {
                return Err(invalid("", format!("the surface would hold {weight} bytes; at most {MAX_CONTENT}")));
            }
        }
        *self = next;
        Ok(applied)
    }

    fn put_components(&mut self, components: &Value, at: &str) -> Result<(), Invalid> {
        let Value::Array(components) = components else { return Err(invalid(at, "components is a list")) };
        let mut checked = Vec::with_capacity(components.len());
        for (index, component) in components.iter().enumerate() {
            checked.push(component_id(component, &format!("{at}/{index}"))?);
        }
        for (id, component) in checked.into_iter().zip(components) {
            self.components.insert(id, component.clone());
        }
        if self.components.len() > MAX_COMPONENTS {
            return Err(invalid(at, format!("a surface has at most {MAX_COMPONENTS} components")));
        }
        Ok(())
    }

    /// All it shows, as one createSurface message.
    pub(crate) fn snapshot(&self, surface: &str) -> String {
        if !self.created {
            return String::new();
        }
        let components: Vec<&Value> = self.components.values().collect();
        json!({"version": "1.0", "createSurface": {
            "surfaceId": surface, "catalogId": CATALOG, "components": components, "dataModel": self.data,
        }})
        .to_string()
    }

    /// Checks a device's message about `surface`: an action from one of
    /// its components whose action has that name, or a renderer's error.
    pub(crate) fn check_renderer(&self, surface: &str, text: &str) -> Result<(), Invalid> {
        if text.len() > MAX_RENDERER_MESSAGE {
            return Err(invalid("", format!("the message is {} bytes; at most {MAX_RENDERER_MESSAGE}", text.len())));
        }
        let message: Value = serde_json::from_str(text).map_err(|error| invalid("", format!("it isn't JSON: {error}")))?;
        depth(&message, "")?;
        let (kind, body) = envelope(&message, &["action", "error"])?;
        let at = format!("/{kind}");
        let string = |key: &str, required: bool| -> Result<Option<&str>, Invalid> {
            match body.get(key) {
                Some(Value::String(text)) => Ok(Some(text)),
                None if !required => Ok(None),
                _ => Err(invalid(format!("{at}/{key}"), format!("{key} is a string{}", if required { ", and required" } else { "" }))),
            }
        };
        if let Some(id) = string("surfaceId", kind == "action")?
            && id != surface
        {
            return Err(invalid(format!("{at}/surfaceId"), format!("\"{id}\" isn't this surface, \"{surface}\"")));
        }
        if kind == "error" {
            only(body, &at, &["code", "message", "surfaceId"])?;
            string("code", true)?;
            string("message", true)?;
            return Ok(());
        }
        only(body, &at, &["name", "surfaceId", "sourceComponentId", "timestamp", "userMessage", "context", "metadata"])?;
        let name = string("name", true)?.unwrap_or_default();
        let source = string("sourceComponentId", true)?.unwrap_or_default();
        string("timestamp", true)?;
        string("userMessage", false)?;
        if !body.get("context").is_some_and(Value::is_object) {
            return Err(invalid(format!("{at}/context"), "context is an object, and required"));
        }
        let declared = self.components.get(source).and_then(|component| component.pointer("/action/event/name")).and_then(Value::as_str);
        match declared {
            Some(declared) if declared == name => Ok(()),
            Some(_) => Err(invalid(format!("{at}/name"), format!("\"{source}\"'s action isn't \"{name}\""))),
            None => Err(invalid(format!("{at}/sourceComponentId"), format!("the surface has no component \"{source}\" with an action"))),
        }
    }
}

/// The message's kind and body: `{"version": "1.0", <kind>: {…}}`.
fn envelope<'a>(message: &'a Value, kinds: &[&'static str]) -> Result<(&'static str, &'a Map<String, Value>), Invalid> {
    let Value::Object(fields) = message else { return Err(invalid("", "a message is a JSON object")) };
    if fields.get("version") != Some(&Value::String("1.0".into())) {
        return Err(invalid("/version", "version is \"1.0\""));
    }
    let mut found = None;
    for key in fields.keys().filter(|key| *key != "version") {
        let Some(kind) = kinds.iter().find(|kind| *kind == key) else {
            return Err(invalid(format!("/{key}"), format!("\"{key}\" isn't supported; a message has one of {}", kinds.join(", "))));
        };
        if found.is_some() {
            return Err(invalid("", "a message has exactly one kind"));
        }
        found = Some(*kind);
    }
    let kind = found.ok_or_else(|| invalid("", format!("a message has one of {}", kinds.join(", "))))?;
    match &fields[kind] {
        Value::Object(body) => Ok((kind, body)),
        _ => Err(invalid(format!("/{kind}"), format!("{kind} is an object"))),
    }
}

/// Refuses keys besides `allowed`.
fn only(object: &Map<String, Value>, at: &str, allowed: &[&str]) -> Result<(), Invalid> {
    match object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Some(key) => Err(invalid(format!("{at}/{}", escape(key)), format!("\"{key}\" isn't allowed here"))),
        None => Ok(()),
    }
}

fn escape(key: &str) -> String {
    key.replace('~', "~0").replace('/', "~1")
}

fn depth(value: &Value, at: &str) -> Result<(), Invalid> {
    fn walk(value: &Value, level: usize) -> bool {
        if level > MAX_DEPTH {
            return false;
        }
        match value {
            Value::Array(items) => items.iter().all(|item| walk(item, level + 1)),
            Value::Object(fields) => fields.values().all(|item| walk(item, level + 1)),
            _ => true,
        }
    }
    if walk(value, 0) { Ok(()) } else { Err(invalid(at, format!("it nests deeper than {MAX_DEPTH}"))) }
}

/// An identifier: letters, digits and _, not starting with a digit (UAX
/// #31, roughly).
fn identifier(text: &str) -> bool {
    let mut chars = text.chars();
    text.chars().count() <= MAX_ID
        && chars.next().is_some_and(|first| first.is_alphabetic() || first == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_')
}

// ---- The catalog ----

#[derive(Clone, Copy)]
enum Kind {
    DynString,
    DynNumber,
    DynBool,
    DynStringList,
    Number,
    /// A component's id.
    Id,
    Children,
    Action,
    Choice(&'static [&'static str]),
    /// A Lucide icon name, or a binding.
    Icon,
    /// A plain string.
    Text,
    Options,
    Tabs,
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
    /// "signal", "muted" or "agent:<id>".
    StatColor,
    /// A Table's columns: [{name, title?, format?}].
    Columns,
}

/// A Table value's most columns, a Chart's most series and a Stat's most
/// spark points.
const MAX_COLUMNS: usize = 64;
const MAX_SERIES: usize = 12;
const MAX_SPARK: usize = 200;

const JUSTIFY: Kind = Kind::Choice(&["start", "center", "end", "spaceBetween", "spaceAround", "spaceEvenly", "stretch"]);
const ALIGN: Kind = Kind::Choice(&["start", "center", "end", "stretch"]);

/// Each component's properties: (name, kind, required).
type Props = &'static [(&'static str, Kind, bool)];

fn catalog(component: &str) -> Option<Props> {
    use Kind::*;
    Some(match component {
        "Text" => &[("text", DynString, true), ("variant", Choice(&["heading", "body", "caption"]), false)],
        "Row" | "Column" => &[("children", Children, true), ("justify", JUSTIFY, false), ("align", ALIGN, false)],
        "List" => &[("children", Children, true), ("direction", Choice(&["vertical", "horizontal"]), false), ("align", ALIGN, false)],
        "Card" => &[("child", Id, true)],
        "Divider" => &[("axis", Choice(&["horizontal", "vertical"]), false)],
        "Icon" => &[("name", Icon, true)],
        "Button" => &[("child", Id, true), ("action", Action, true), ("variant", Choice(&["default", "primary", "borderless"]), false)],
        "TextField" => &[
            ("label", DynString, true),
            ("value", DynString, true),
            ("placeholder", DynString, false),
            // No "obscured": a password-style field would let an app pass
            // itself off as Claktory asking for the vault's passphrase.
            ("variant", Choice(&["shortText", "longText", "number"]), false),
        ],
        "CheckBox" => &[("label", DynString, true), ("value", DynBool, true)],
        "ChoicePicker" => &[
            ("label", DynString, false),
            ("options", Options, true),
            ("value", DynStringList, true),
            ("variant", Choice(&["mutuallyExclusive", "multipleSelection"]), false),
            ("displayStyle", Choice(&["checkbox", "chips"]), false),
        ],
        "Slider" => &[("label", DynString, false), ("min", Number, false), ("max", Number, true), ("value", DynNumber, true)],
        "Tabs" => &[("tabs", Tabs, true)],
        "ProgressBar" => &[("value", DynNumber, true), ("label", DynString, false)],
        "EmptyState" => &[("title", DynString, true), ("body", DynString, false), ("icon", Text, false)],
        "Chart" => &[
            ("kind", Choice(&["timeseries", "bars", "stackedBars", "pie", "scatter"]), true),
            ("data", Table, true),
            ("x", Text, false),
            ("y", Series, false),
            ("seriesBy", Text, false),
            ("color", Choice(&["palette", "agent", "signal", "muted"]), false),
            ("label", DynString, false),
            ("unit", DynString, false),
            ("height", Range(120.0, 480.0), false),
        ],
        "Stat" => &[
            ("label", DynString, true),
            ("value", DynScalar, true),
            ("unit", DynString, false),
            ("spark", Spark, false),
            ("color", StatColor, false),
        ],
        "Table" => &[("data", Table, true), ("columns", Columns, false)],
        _ => return None,
    })
}

/// Checks a component; its id.
fn component_id(component: &Value, at: &str) -> Result<String, Invalid> {
    let Value::Object(fields) = component else { return Err(invalid(at, "a component is an object")) };
    let id = match fields.get("id") {
        Some(Value::String(id)) if identifier(id) => id.clone(),
        Some(Value::String(id)) => {
            return Err(invalid(format!("{at}/id"), format!("\"{id}\" isn't an id: letters, digits and _, not starting with a digit")));
        }
        _ => return Err(invalid(format!("{at}/id"), "id is required")),
    };
    let name = match fields.get("component") {
        Some(Value::String(name)) => name.as_str(),
        _ => return Err(invalid(format!("{at}/component"), "component is required")),
    };
    let Some(props) = catalog(name) else {
        return Err(invalid(format!("{at}/component"), format!("\"{name}\" isn't in Claktory's catalog")));
    };
    for (key, value) in fields {
        let here = format!("{at}/{}", escape(key));
        match key.as_str() {
            "id" | "component" => {}
            "weight" => {
                if !value.as_f64().is_some_and(|weight| weight > 0.0) {
                    return Err(invalid(here, "weight is a number above 0"));
                }
            }
            "@accessibility" => accessibility(value, &here)?,
            _ => match props.iter().find(|(prop, _, _)| prop == key) {
                Some((_, kind, _)) => check(*kind, value, &here)?,
                None => return Err(invalid(here, format!("{name} has no property \"{key}\""))),
            },
        }
    }
    if let Some((missing, _, _)) = props.iter().find(|(prop, _, required)| *required && !fields.contains_key(*prop)) {
        return Err(invalid(format!("{at}/{missing}"), format!("{name} needs {missing}")));
    }
    if name == "Chart" && fields.contains_key("seriesBy") && fields.get("y").and_then(Value::as_array).is_some_and(|y| y.len() > 1) {
        return Err(invalid(format!("{at}/y"), "with seriesBy, y names one column at most"));
    }
    Ok(id)
}

fn accessibility(value: &Value, at: &str) -> Result<(), Invalid> {
    let Value::Object(fields) = value else { return Err(invalid(at, "@accessibility is an object")) };
    for (key, value) in fields {
        let kind = match key.as_str() {
            "label" | "description" => Kind::DynString,
            "hidden" => Kind::DynBool,
            "live" => Kind::Choice(&["off", "polite", "assertive"]),
            _ => return Err(invalid(format!("{at}/{}", escape(key)), format!("\"{key}\" isn't allowed here"))),
        };
        check(kind, value, &format!("{at}/{key}"))?;
    }
    Ok(())
}

/// A `{"@path": …}` binding, when `value` is one.
fn binding(value: &Value, at: &str) -> Option<Result<(), Invalid>> {
    let Value::Object(fields) = value else { return None };
    Some(match (fields.len(), fields.get("@path")) {
        (1, Some(Value::String(path))) if !path.is_empty() && path.len() <= MAX_PATH => Ok(()),
        (_, Some(_)) => Err(invalid(format!("{at}/@path"), "a binding is {\"@path\": <a JSON pointer>} alone")),
        _ if fields.contains_key("@call") => Err(invalid(format!("{at}/@call"), "functions (@call) aren't supported")),
        _ => Err(invalid(at, "this is a value or a {\"@path\": …} binding")),
    })
}

fn check(kind: Kind, value: &Value, at: &str) -> Result<(), Invalid> {
    let literal =
        |ok: bool, what: &str| if ok { Ok(()) } else { Err(invalid(at, format!("this is {what}, or a {{\"@path\": …}} binding"))) };
    match kind {
        Kind::DynString => binding(value, at).unwrap_or_else(|| literal(value.is_string(), "a string")),
        Kind::DynNumber => binding(value, at).unwrap_or_else(|| literal(value.is_number(), "a number")),
        Kind::DynBool => binding(value, at).unwrap_or_else(|| literal(value.is_boolean(), "true or false")),
        Kind::DynStringList => binding(value, at)
            .unwrap_or_else(|| literal(value.as_array().is_some_and(|items| items.iter().all(Value::is_string)), "a list of strings")),
        Kind::Icon => binding(value, at).unwrap_or_else(|| literal(value.as_str().is_some_and(|name| name.len() <= 64), "an icon's name")),
        Kind::Number => {
            if value.is_number() {
                Ok(())
            } else {
                Err(invalid(at, "this is a number"))
            }
        }
        Kind::Text => {
            if value.is_string() {
                Ok(())
            } else {
                Err(invalid(at, "this is a string"))
            }
        }
        Kind::Choice(choices) => match value.as_str() {
            Some(choice) if choices.contains(&choice) => Ok(()),
            _ => Err(invalid(at, format!("this is one of {}", choices.join(", ")))),
        },
        Kind::Id => match value.as_str() {
            Some(id) if identifier(id) => Ok(()),
            _ => Err(invalid(at, "this is a component's id")),
        },
        Kind::Children => match value {
            Value::Array(ids) if ids.len() <= MAX_CHILDREN => {
                ids.iter().enumerate().try_for_each(|(index, id)| check(Kind::Id, id, &format!("{at}/{index}")))
            }
            Value::Array(_) => Err(invalid(at, format!("at most {MAX_CHILDREN} children"))),
            Value::Object(template) => {
                only(template, at, &["componentId", "path"])?;
                check(Kind::Id, template.get("componentId").unwrap_or(&Value::Null), &format!("{at}/componentId"))?;
                match template.get("path") {
                    Some(Value::String(path)) if !path.is_empty() && path.len() <= MAX_PATH => Ok(()),
                    _ => Err(invalid(format!("{at}/path"), "a template's path is a JSON pointer to a list")),
                }
            }
            _ => Err(invalid(at, "children are a list of ids, or a template {componentId, path}")),
        },
        Kind::Action => {
            let Value::Object(action) = value else { return Err(invalid(at, "an action is an object")) };
            only(action, at, &["event"])?;
            let Some(Value::Object(event)) = action.get("event") else {
                return Err(invalid(format!("{at}/event"), "an action is {\"event\": {name, context}}"));
            };
            only(event, &format!("{at}/event"), &["name", "context"])?;
            match event.get("name") {
                Some(Value::String(name)) if identifier(name) => {}
                _ => return Err(invalid(format!("{at}/event/name"), "an action's name is an identifier")),
            }
            match event.get("context") {
                None => Ok(()),
                Some(Value::Object(context)) => context.iter().try_for_each(|(key, value)| {
                    let here = format!("{at}/event/context/{}", escape(key));
                    binding(value, &here).unwrap_or_else(|| no_functions(value, &here))
                }),
                Some(_) => Err(invalid(format!("{at}/event/context"), "context is an object")),
            }
        }
        Kind::Options => match value {
            Value::Array(options) if options.len() <= MAX_OPTIONS => options.iter().enumerate().try_for_each(|(index, option)| {
                let here = format!("{at}/{index}");
                let Value::Object(option) = option else { return Err(invalid(&here, "an option is {label, value}")) };
                only(option, &here, &["label", "value"])?;
                check(Kind::DynString, option.get("label").unwrap_or(&Value::Null), &format!("{here}/label"))?;
                check(Kind::Text, option.get("value").unwrap_or(&Value::Null), &format!("{here}/value"))
            }),
            _ => Err(invalid(at, format!("options are a list of at most {MAX_OPTIONS}"))),
        },
        Kind::Tabs => match value {
            Value::Array(tabs) if !tabs.is_empty() && tabs.len() <= MAX_TABS => tabs.iter().enumerate().try_for_each(|(index, tab)| {
                let here = format!("{at}/{index}");
                let Value::Object(tab) = tab else { return Err(invalid(&here, "a tab is {title, child}")) };
                only(tab, &here, &["title", "child"])?;
                check(Kind::DynString, tab.get("title").unwrap_or(&Value::Null), &format!("{here}/title"))?;
                check(Kind::Id, tab.get("child").unwrap_or(&Value::Null), &format!("{here}/child"))
            }),
            _ => Err(invalid(at, format!("tabs are a list of 1 to {MAX_TABS}"))),
        },
        Kind::Table => match value.as_object() {
            // A literal table is an object too: a binding only when it says so.
            Some(fields) if fields.keys().any(|key| key.starts_with('@')) => binding(value, at).expect("an object"),
            _ => table(value, at),
        },
        Kind::Series => match value {
            Value::Array(names) if !names.is_empty() && names.len() <= MAX_SERIES && names.iter().all(Value::is_string) => Ok(()),
            _ => Err(invalid(at, format!("this is a list of 1 to {MAX_SERIES} column names"))),
        },
        Kind::DynScalar => binding(value, at).unwrap_or_else(|| literal(value.is_string() || value.is_number(), "a string or a number")),
        Kind::Spark => binding(value, at).unwrap_or_else(|| {
            let numbers = value.as_array().is_some_and(|items| items.len() <= MAX_SPARK && items.iter().all(Value::is_number));
            literal(numbers, &format!("a list of at most {MAX_SPARK} numbers"))
        }),
        Kind::Range(low, high) => match value.as_f64() {
            Some(number) if (low..=high).contains(&number) => Ok(()),
            _ => Err(invalid(at, format!("this is a number from {low} to {high}"))),
        },
        Kind::StatColor => match value.as_str() {
            Some("signal" | "muted") => Ok(()),
            Some(color) if color.strip_prefix("agent:").is_some_and(agent_id) => Ok(()),
            _ => Err(invalid(at, "this is signal, muted or agent:<an agent's id>")),
        },
        Kind::Columns => match value {
            Value::Array(columns) if !columns.is_empty() && columns.len() <= MAX_COLUMNS => {
                columns.iter().enumerate().try_for_each(|(index, column)| {
                    let here = format!("{at}/{index}");
                    let Value::Object(column) = column else { return Err(invalid(&here, "a column is {name, title?, format?}")) };
                    only(column, &here, &["name", "title", "format"])?;
                    check(Kind::Text, column.get("name").unwrap_or(&Value::Null), &format!("{here}/name"))?;
                    if let Some(title) = column.get("title") {
                        check(Kind::DynString, title, &format!("{here}/title"))?;
                    }
                    match column.get("format") {
                        Some(format) => check(Kind::Choice(&["text", "number", "time"]), format, &format!("{here}/format")),
                        None => Ok(()),
                    }
                })
            }
            _ => Err(invalid(at, format!("columns are a list of 1 to {MAX_COLUMNS}"))),
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

/// A literal in an action's context: plain JSON, no `@` keys.
fn no_functions(value: &Value, at: &str) -> Result<(), Invalid> {
    match value {
        Value::Object(fields) => fields.iter().try_for_each(|(key, value)| {
            if key.starts_with('@') {
                return Err(invalid(format!("{at}/{}", escape(key)), format!("\"{key}\" isn't supported here")));
            }
            no_functions(value, &format!("{at}/{}", escape(key)))
        }),
        Value::Array(items) => items.iter().enumerate().try_for_each(|(index, item)| no_functions(item, &format!("{at}/{index}"))),
        _ => Ok(()),
    }
}

// ---- The data model ----

/// Sets `path` (a JSON pointer) in `data` to `value`, making the objects on
/// the way; null deletes it. The root stays an object.
fn set(data: &mut Value, path: &str, value: Value) -> Result<(), String> {
    if path.is_empty() || path == "/" {
        *data = match value {
            Value::Null => json!({}),
            Value::Object(_) => value,
            _ => return Err("the whole data model is an object".to_owned()),
        };
        return Ok(());
    }
    let Some(rest) = path.strip_prefix('/') else { return Err(format!("\"{path}\" isn't a JSON pointer")) };
    if path.len() > MAX_PATH {
        return Err(format!("a path is at most {MAX_PATH} bytes"));
    }
    let tokens: Vec<String> = rest.split('/').map(|token| token.replace("~1", "/").replace("~0", "~")).collect();
    let (last, parents) = tokens.split_last().expect("one token at least");
    let mut node = data;
    for token in parents {
        node = match node {
            Value::Object(fields) => fields.entry(token.clone()).or_insert_with(|| json!({})),
            Value::Array(items) => {
                let index = index(token, items.len(), false)?;
                &mut items[index]
            }
            _ => return Err(format!("\"{token}\" is inside something that's neither an object nor a list")),
        };
        if node.is_null() {
            *node = json!({});
        }
    }
    match node {
        Value::Object(fields) => {
            if value.is_null() {
                fields.remove(last);
            } else {
                fields.insert(last.clone(), value);
            }
        }
        Value::Array(items) => {
            if last == "-" && !value.is_null() {
                items.push(value);
                return Ok(());
            }
            let at = index(last, items.len(), !value.is_null())?;
            if value.is_null() {
                if at < items.len() {
                    items.remove(at);
                }
            } else if at == items.len() {
                items.push(value);
            } else {
                items[at] = value;
            }
        }
        _ => return Err(format!("\"{last}\" is inside something that's neither an object nor a list")),
    }
    Ok(())
}

/// A list's index token: within it, or just past its end when `append`.
fn index(token: &str, len: usize, append: bool) -> Result<usize, String> {
    let at: usize = token.parse().map_err(|_| format!("\"{token}\" isn't an index into a list"))?;
    if at < len || (append && at == len) { Ok(at) } else { Err(format!("{at} is past the list's end ({len})")) }
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "charts-1";

    fn created() -> Content {
        let mut content = Content::default();
        let create = json!({"version": "1.0", "createSurface": {"surfaceId": S, "catalogId": CATALOG,
            "components": [
                {"id": "root", "component": "Column", "children": ["title", "go"]},
                {"id": "title", "component": "Text", "text": {"@path": "/title"}, "variant": "heading"},
                {"id": "go_label", "component": "Text", "text": "Go"},
                {"id": "go", "component": "Button", "child": "go_label", "action": {"event": {"name": "go", "context": {"q": {"@path": "/q"}, "n": 1}}}},
            ],
            "dataModel": {"title": "Tokens"}}});
        assert_eq!(content.apply(S, &create.to_string()), Ok(Applied::Created));
        content
    }

    fn apply(content: &mut Content, message: Value) -> Result<Applied, String> {
        content.apply(S, &message.to_string()).map_err(|invalid| invalid.to_string())
    }

    #[test]
    fn a_surface_is_created_then_changed_and_its_snapshot_says_it_all() {
        let mut content = created();
        assert!(content.created());
        apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/rows", "value": []}})).unwrap();
        let past =
            apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/rows/5", "value": {"n": 3}}}));
        assert!(past.unwrap_err().contains("past the list's end"));
        apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/rows/-", "value": {"n": 3}}})).unwrap();
        apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/a/b~1c", "value": true}})).unwrap();
        apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/title", "value": null}})).unwrap();
        let replaced = json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "title", "component": "Text", "text": "Runs"}]}});
        assert_eq!(apply(&mut content, replaced.clone()), Ok(Applied::Changed(replaced.to_string())));
        let snapshot: Value = serde_json::from_str(&content.snapshot(S)).unwrap();
        assert_eq!(snapshot["createSurface"]["dataModel"], json!({"rows": [{"n": 3}], "a": {"b/c": true}}));
        let components = snapshot["createSurface"]["components"].as_array().unwrap();
        assert_eq!(components.len(), 4);
        assert!(components.contains(&json!({"id": "title", "component": "Text", "text": "Runs"})));
        assert_eq!(apply(&mut content, json!({"version": "1.0", "deleteSurface": {"surfaceId": S}})), Ok(Applied::Deleted));
        assert_eq!(Content::default().snapshot(S), "", "nothing before it's created");
    }

    #[test]
    fn what_isnt_in_the_catalog_or_the_protocol_is_refused_and_changes_nothing() {
        let mut fresh = Content::default();
        let early = apply(&mut fresh, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "value": {}}}));
        assert_eq!(early, Err("INTEGRITY_ERROR: createSurface comes first".into()));
        let cases = [
            (json!({"version": "0.9", "deleteSurface": {"surfaceId": S}}), "at /version"),
            (json!({"version": "1.0", "callRendererFunction": {}}), "\"callRendererFunction\" isn't supported"),
            (json!({"version": "1.0", "deleteSurface": {"surfaceId": "other"}}), "isn't this surface"),
            (json!({"version": "1.0", "createSurface": {"surfaceId": S}}), "INTEGRITY_ERROR: the surface was created already"),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Video", "url": "u"}]}}),
                "isn't in Claktory's catalog",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "1x", "component": "Divider"}]}}),
                "isn't an id",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Text"}]}}),
                "Text needs text",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Text", "text": "a", "markdown": true}]}}),
                "has no property \"markdown\"",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Text", "text": {"@call": "formatString", "args": {}}}]}}),
                "functions (@call) aren't supported",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Text", "text": "a", "variant": "huge"}]}}),
                "one of heading, body, caption",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Button", "child": "y", "action": {"functionCall": {}}}]}}),
                "\"functionCall\" isn't allowed",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "Text", "text": "a", "@checks": []}]}}),
                "has no property \"@checks\"",
            ),
            (
                json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": [{"id": "x", "component": "TextField", "label": "Passphrase", "value": "", "variant": "obscured"}]}}),
                "one of shortText, longText, number",
            ),
            (
                json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/", "value": [1]}}),
                "the whole data model is an object",
            ),
            (json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "title", "value": 1}}), "isn't a JSON pointer"),
        ];
        for (message, expected) in cases {
            let mut content = created();
            let before = content.snapshot(S);
            let refused = apply(&mut content, message.clone()).expect_err(&message.to_string());
            assert!(refused.contains(expected), "{message}: {refused}");
            assert_eq!(content.snapshot(S), before, "{message} changed nothing");
        }
        let mut fresh = Content::default();
        let theme = json!({"version": "1.0", "createSurface": {"surfaceId": S, "theme": {}}});
        assert!(apply(&mut fresh, theme).unwrap_err().contains("\"theme\" isn't allowed"));
        let send = json!({"version": "1.0", "createSurface": {"surfaceId": S, "sendDataModel": true}});
        assert!(apply(&mut fresh, send).unwrap_err().contains("sendDataModel isn't supported"));
        let catalog = json!({"version": "1.0", "createSurface": {"surfaceId": S, "catalogId": "https://a2ui.org/basic"}});
        assert!(apply(&mut fresh, catalog).unwrap_err().contains(CATALOG));
        assert!(!fresh.created());
    }

    #[test]
    fn bounds_hold() {
        let mut content = created();
        let deep = (0..40).fold(json!(1), |inner, _| json!([inner]));
        let refused = apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/d", "value": deep}}));
        assert!(refused.unwrap_err().contains("nests deeper than 32"));
        let many: Vec<Value> = (0..1001).map(|n| json!({"id": format!("c{n}"), "component": "Divider"})).collect();
        let refused = apply(&mut content, json!({"version": "1.0", "updateComponents": {"surfaceId": S, "components": many}}));
        assert!(refused.unwrap_err().contains("at most 1000 components"));
        let big = "x".repeat(MAX_MESSAGE + 1);
        assert!(content.apply(S, &big).unwrap_err().to_string().contains("at most 1048576"));
        let chunk = "x".repeat(900_000);
        for n in 0..4 {
            apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": format!("/b{n}"), "value": chunk}}))
                .unwrap();
        }
        let refused = apply(&mut content, json!({"version": "1.0", "updateDataModel": {"surfaceId": S, "path": "/b4", "value": chunk}}));
        assert!(refused.unwrap_err().contains("at most 4194304"));
    }

    #[test]
    fn actions_come_only_from_a_component_with_that_action() {
        let content = created();
        let action = |name: &str, source: &str| {
            json!({"version": "1.0", "action": {"name": name, "surfaceId": S, "sourceComponentId": source,
                "timestamp": "2026-11-22T10:00:00Z", "context": {"q": "x"}}})
            .to_string()
        };
        assert_eq!(content.check_renderer(S, &action("go", "go")), Ok(()));
        assert!(content.check_renderer(S, &action("delete", "go")).unwrap_err().to_string().contains("isn't \"delete\""));
        assert!(
            content.check_renderer(S, &action("go", "title")).unwrap_err().to_string().contains("no component \"title\" with an action")
        );
        assert!(content.check_renderer("charts-2", &action("go", "go")).unwrap_err().to_string().contains("isn't this surface"));
        let error = json!({"version": "1.0", "error": {"code": "EXPRESSION_ERROR", "message": "bad", "surfaceId": S}});
        assert_eq!(content.check_renderer(S, &error.to_string()), Ok(()));
        let call = json!({"version": "1.0", "callAgentFunction": {}});
        assert!(content.check_renderer(S, &call.to_string()).is_err());
        assert!(content.check_renderer(S, &"x".repeat(MAX_RENDERER_MESSAGE + 1)).is_err());
    }

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
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": rows, "theme": "x"}), "has no property \"theme\""),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [[1, 2]]}}), "a list of 1 strings"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [[{"x": 1}]]}}), "rows/0"),
            (json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": [], "rows": []}}), "1 to 64 names"),
            (
                json!({"id": "c", "component": "Chart", "kind": "bars", "data": {"columns": ["a"], "rows": [], "x": 1}}),
                "isn't allowed here",
            ),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": true}), "a string or a number"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "color": "#ff0000"}), "signal, muted or agent"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "color": "agent:Lea Smith"}), "signal, muted or agent"),
            (json!({"id": "s", "component": "Stat", "label": "x", "value": 1, "spark": ["a"]}), "at most 200 numbers"),
            (json!({"id": "t", "component": "Table", "data": rows, "columns": [{"name": "day", "format": "date"}]}), "one of text"),
            (json!({"id": "t", "component": "Table", "data": rows, "columns": [{"title": "Day"}]}), "this is a string"),
        ] {
            let error = apply(&mut content, component(wrong.clone())).unwrap_err();
            assert!(error.contains(why), "{wrong}: {error}");
        }
    }
}
