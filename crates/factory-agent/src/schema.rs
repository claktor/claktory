use serde_json::{Map, Number, Value, json};

use crate::text::utf16_len;

pub fn parameters(tool: &str) -> Value {
    match tool {
        "read_file" => {
            json!({"type": "object", "required": ["path"], "properties": {"path": {"type": "string"}, "offset": {"anyOf": [{"type": "integer", "minimum": 1}, {"type": "null"}]}, "limit": {"anyOf": [{"type": "integer", "minimum": 1, "maximum": 2000}, {"type": "null"}]}}, "additionalProperties": false})
        }
        "write_file" => {
            json!({"type": "object", "required": ["path", "content"], "properties": {"path": {"type": "string"}, "content": {"type": "string"}}, "additionalProperties": false})
        }
        "edit_file" => {
            json!({"type": "object", "required": ["path", "edits"], "properties": {"path": {"type": "string"}, "edits": {"type": "array", "items": {"type": "object", "required": ["oldText", "newText"], "properties": {"oldText": {"type": "string", "minLength": 1}, "newText": {"type": "string"}, "replaceAll": {"anyOf": [{"type": "boolean"}, {"type": "null"}]}}, "additionalProperties": false}, "minItems": 1}}, "additionalProperties": false})
        }
        "bash" => {
            json!({"type": "object", "required": ["command"], "properties": {"command": {"type": "string"}, "timeout": {"anyOf": [{"type": "number", "exclusiveMinimum": 0, "maximum": 7200}, {"type": "null"}]}, "run_in_background": {"anyOf": [{"type": "boolean"}, {"type": "null"}]}}, "additionalProperties": false})
        }
        _ => Value::Null,
    }
}

/// Strict sampling: every property is required, since the optional ones are nullable.
pub fn strict_parameters(schema: &Value) -> Value {
    let mut strict = schema.clone();
    if let Some(object) = strict.as_object_mut() {
        if let Some(properties) = object.get("properties").and_then(Value::as_object) {
            let keys: Vec<Value> = properties.keys().map(|key| Value::String(key.clone())).collect();
            let nested: Map<String, Value> = properties.iter().map(|(key, property)| (key.clone(), strict_parameters(property))).collect();
            object.insert("properties".into(), Value::Object(nested));
            object.insert("required".into(), Value::Array(keys));
        }
        if let Some(items) = object.get("items") {
            let items = strict_parameters(items);
            object.insert("items".into(), items);
        }
    }
    strict
}

fn schema_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(Value::as_str)
}

/// `Number.prototype.toString`.
pub fn js_number(n: f64) -> String {
    if n == 0.0 {
        return "0".into();
    }
    if n.is_nan() {
        return "NaN".into();
    }
    if n.is_infinite() {
        return if n > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let abs = n.abs();
    if !(1e-6..1e21).contains(&abs) {
        let text = format!("{n:e}");
        return match text.split_once('e') {
            Some((mantissa, exponent)) if !exponent.starts_with('-') => format!("{mantissa}e+{exponent}"),
            _ => text,
        };
    }
    if n.fract() == 0.0 { format!("{}", n as i128) } else { n.to_string() }
}

fn radix_digits(digits: &str, base: u32) -> f64 {
    if digits.is_empty() || !digits.chars().all(|c| c.is_digit(base)) {
        return f64::NAN;
    }
    digits.chars().fold(0.0, |acc, c| acc * f64::from(base) + f64::from(c.to_digit(base).unwrap_or(0)))
}

fn is_decimal_literal(body: &str) -> bool {
    let digits = |s: &str| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit());
    let (mantissa, exponent) = body.split_once(['e', 'E']).map_or((body, None), |(m, e)| (m, Some(e)));
    let mantissa_ok = match mantissa.split_once('.') {
        Some((int, frac)) => (digits(int) && (frac.is_empty() || digits(frac))) || (int.is_empty() && digits(frac)),
        None => digits(mantissa),
    };
    mantissa_ok && exponent.is_none_or(|e| digits(e.strip_prefix(['+', '-']).unwrap_or(e)))
}

/// JS unary plus on a string; NaN when the text is not a numeric literal.
pub fn js_string_to_number(text: &str) -> f64 {
    let trimmed = text.trim_matches(|c: char| c.is_whitespace() || c == '\u{feff}');
    if trimmed.is_empty() {
        return 0.0;
    }
    if let Some(rest) = trimmed.strip_prefix("0x").or_else(|| trimmed.strip_prefix("0X")) {
        return radix_digits(rest, 16);
    }
    if let Some(rest) = trimmed.strip_prefix("0o").or_else(|| trimmed.strip_prefix("0O")) {
        return radix_digits(rest, 8);
    }
    if let Some(rest) = trimmed.strip_prefix("0b").or_else(|| trimmed.strip_prefix("0B")) {
        return radix_digits(rest, 2);
    }
    let (sign, body) = match trimmed.strip_prefix('-') {
        Some(rest) => (-1.0, rest),
        None => (1.0, trimmed.strip_prefix('+').unwrap_or(trimmed)),
    };
    if body == "Infinity" {
        return sign * f64::INFINITY;
    }
    if is_decimal_literal(body) { body.parse::<f64>().map_or(f64::NAN, |value| sign * value) } else { f64::NAN }
}

fn try_number(value: &Value) -> Option<f64> {
    match value {
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        Value::Number(n) => n.as_f64(),
        Value::Null => Some(0.0),
        Value::String(text) => {
            let coerced = js_string_to_number(text);
            if !coerced.is_nan() {
                return Some(coerced);
            }
            match text.to_lowercase().as_str() {
                "false" => Some(0.0),
                "true" => Some(1.0),
                _ => None,
            }
        }
        _ => None,
    }
}

fn try_null(value: &Value) -> bool {
    match value {
        Value::Bool(false) | Value::Null => true,
        Value::Number(n) => n.as_f64() == Some(0.0),
        Value::String(text) => {
            let lower = text.to_lowercase();
            lower == "undefined" || lower == "null" || text.is_empty() || text == "0"
        }
        _ => false,
    }
}

fn try_string(value: &Value) -> Option<String> {
    match value {
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(n) => Some(js_number(n.as_f64().unwrap_or(0.0))),
        Value::Null => Some("null".into()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    }
}

fn try_boolean(value: &Value) -> Option<bool> {
    match value {
        Value::Bool(flag) => Some(*flag),
        Value::Null => Some(false),
        Value::Number(n) => match n.as_f64() {
            Some(0.0) => Some(false),
            Some(1.0) => Some(true),
            _ => None,
        },
        Value::String(text) => match (text.to_lowercase().as_str(), text.as_str()) {
            ("false", _) | (_, "0") => Some(false),
            ("true", _) | (_, "1") => Some(true),
            _ => None,
        },
        _ => None,
    }
}

fn number(n: f64) -> Value {
    if n.fract() == 0.0 && n.abs() <= 9_007_199_254_740_992.0 {
        Value::Number(Number::from(n as i64))
    } else {
        Number::from_f64(n).map_or(Value::Null, Value::Number)
    }
}

/// The subset of TypeBox's `Value.Convert` these schemas can reach.
pub fn convert(schema: &Value, value: Value) -> Value {
    if let Some(members) = schema.get("anyOf").and_then(Value::as_array) {
        if members.iter().any(|member| check(member, &value, "").is_empty()) {
            return value;
        }
        let candidates: Vec<Value> = members.iter().map(|member| convert(member, value.clone())).collect();
        return candidates
            .into_iter()
            .find(|candidate| members.iter().any(|member| check(member, candidate, "").is_empty()))
            .unwrap_or(value);
    }
    match schema_type(schema) {
        Some("string") => try_string(&value).map_or(value, Value::String),
        Some("number") => try_number(&value).filter(|n| n.is_finite()).map_or(value, number),
        Some("integer") => try_number(&value).filter(|n| n.is_finite()).map_or(value, |n| number(n.trunc())),
        Some("boolean") => try_boolean(&value).map_or(value, Value::Bool),
        Some("null") if try_null(&value) => Value::Null,
        Some("object") => match value {
            Value::Object(mut map) => {
                if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
                    for (key, property) in properties {
                        if let Some(slot) = map.get_mut(key) {
                            let current = std::mem::take(slot);
                            *slot = convert(property, current);
                        }
                    }
                }
                Value::Object(map)
            }
            other => other,
        },
        Some("array") => {
            let items = match value {
                Value::Array(items) => items,
                other => vec![other],
            };
            match schema.get("items") {
                Some(item_schema) => Value::Array(items.into_iter().map(|item| convert(item_schema, item)).collect()),
                None => Value::Array(items),
            }
        }
        _ => value,
    }
}

fn required_keys(schema: &Value) -> Vec<&str> {
    schema.get("required").and_then(Value::as_array).map(|r| r.iter().filter_map(Value::as_str).collect()).unwrap_or_default()
}

/// Pi drops a `null` for an optional property whose schema rejects null; ours all accept it.
fn normalize_optional_nulls(value: &mut Value, schema: &Value) {
    match value {
        Value::Array(items) => {
            if let Some(item_schema) = schema.get("items") {
                for item in items {
                    normalize_optional_nulls(item, item_schema);
                }
            }
        }
        Value::Object(map) => {
            let Some(properties) = schema.get("properties").and_then(Value::as_object) else { return };
            let required = required_keys(schema);
            for (key, property) in properties {
                let Some(current) = map.get_mut(key) else { continue };
                if current.is_null()
                    && !required.contains(&key.as_str())
                    && property.get("$ref").is_none()
                    && !check(property, &Value::Null, "").is_empty()
                {
                    map.remove(key);
                } else {
                    normalize_optional_nulls(current, property);
                }
            }
        }
        _ => {}
    }
}

fn join(path: &str, key: &str) -> String {
    if path.is_empty() { key.to_owned() } else { format!("{path}.{key}") }
}

fn label(path: &str) -> String {
    if path.is_empty() { "root".to_owned() } else { path.to_owned() }
}

fn bounds(schema: &Value, value: f64, path: &str, errors: &mut Vec<(String, String)>) {
    let limit = |key: &str| schema.get(key).and_then(Value::as_f64).map(|limit| (limit, schema[key].to_string()));
    if let Some((min, text)) = limit("minimum")
        && value < min
    {
        errors.push((label(path), format!("must be >= {text}")));
    }
    if let Some((max, text)) = limit("maximum")
        && value > max
    {
        errors.push((label(path), format!("must be <= {text}")));
    }
    if let Some((min, text)) = limit("exclusiveMinimum")
        && value <= min
    {
        errors.push((label(path), format!("must be > {text}")));
    }
    if let Some((max, text)) = limit("exclusiveMaximum")
        && value >= max
    {
        errors.push((label(path), format!("must be < {text}")));
    }
}

/// TypeBox's `Errors` for these schemas: the path and message of every failing rule, in TypeBox's order.
pub fn check(schema: &Value, value: &Value, path: &str) -> Vec<(String, String)> {
    let mut errors = Vec::new();
    if let Some(members) = schema.get("anyOf").and_then(Value::as_array) {
        if members.iter().any(|member| check(member, value, path).is_empty()) {
            return errors;
        }
        for member in members {
            errors.extend(check(member, value, path));
        }
        errors.push((label(path), "must match a schema in anyOf".into()));
        return errors;
    }
    match schema_type(schema) {
        Some("object") => {
            let Some(map) = value.as_object() else {
                errors.push((label(path), "must be object".into()));
                return errors;
            };
            let properties = schema.get("properties").and_then(Value::as_object);
            let required = required_keys(schema);
            let missing: Vec<&str> = required.iter().copied().filter(|key| !map.contains_key(*key)).collect();
            if let Some(first) = missing.first() {
                errors.push((join(path, first), format!("must have required properties {}", missing.join(", "))));
            }
            if schema.get("additionalProperties") == Some(&Value::Bool(false))
                && map.keys().any(|key| !properties.is_some_and(|p| p.contains_key(key)))
            {
                errors.push((label(path), "must not have additional properties".into()));
            }
            for (key, property) in properties.into_iter().flatten() {
                if let Some(current) = map.get(key) {
                    errors.extend(check(property, current, &join(path, key)));
                }
            }
        }
        Some("array") => {
            let Some(items) = value.as_array() else {
                errors.push((label(path), "must be array".into()));
                return errors;
            };
            if let Some(min) = schema.get("minItems").and_then(Value::as_u64)
                && (items.len() as u64) < min
            {
                errors.push((label(path), format!("must not have fewer than {min} items")));
            }
            if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
                && (items.len() as u64) > max
            {
                errors.push((label(path), format!("must not have more than {max} items")));
            }
            if let Some(item_schema) = schema.get("items") {
                for (index, item) in items.iter().enumerate() {
                    errors.extend(check(item_schema, item, &join(path, &index.to_string())));
                }
            }
        }
        Some("string") => {
            let Some(text) = value.as_str() else {
                errors.push((label(path), "must be string".into()));
                return errors;
            };
            let length = utf16_len(text) as u64;
            if let Some(min) = schema.get("minLength").and_then(Value::as_u64)
                && length < min
            {
                errors.push((label(path), format!("must not have fewer than {min} characters")));
            }
            if let Some(max) = schema.get("maxLength").and_then(Value::as_u64)
                && length > max
            {
                errors.push((label(path), format!("must not have more than {max} characters")));
            }
        }
        Some("integer") => match value.as_f64().filter(|n| n.is_finite()) {
            Some(n) => {
                if n.fract() != 0.0 {
                    errors.push((label(path), "must be integer".into()));
                }
                bounds(schema, n, path, &mut errors);
            }
            None => errors.push((label(path), "must be integer".into())),
        },
        Some("number") => match value.as_f64().filter(|n| n.is_finite()) {
            Some(n) => bounds(schema, n, path, &mut errors),
            None => errors.push((label(path), "must be number".into())),
        },
        Some("boolean") if !value.is_boolean() => errors.push((label(path), "must be boolean".into())),
        Some("null") if !value.is_null() => errors.push((label(path), "must be null".into())),
        _ => {}
    }
    errors
}

/// `JSON.stringify(value, null, 2)`, with JavaScript's number formatting.
pub fn js_pretty(value: &Value, depth: usize) -> String {
    let indent = |level: usize| "  ".repeat(level);
    match value {
        Value::Null => "null".into(),
        Value::Bool(flag) => flag.to_string(),
        Value::Number(n) => js_number(n.as_f64().unwrap_or(0.0)),
        Value::String(text) => serde_json::to_string(text).unwrap_or_default(),
        Value::Array(items) if items.is_empty() => "[]".into(),
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(|item| format!("{}{}", indent(depth + 1), js_pretty(item, depth + 1))).collect();
            format!("[\n{}\n{}]", inner.join(",\n"), indent(depth))
        }
        Value::Object(map) if map.is_empty() => "{}".into(),
        Value::Object(map) => {
            let inner: Vec<String> = map
                .iter()
                .map(|(key, item)| {
                    format!("{}{}: {}", indent(depth + 1), serde_json::to_string(key).unwrap_or_default(), js_pretty(item, depth + 1))
                })
                .collect();
            format!("{{\n{}\n{}}}", inner.join(",\n"), indent(depth))
        }
    }
}

/// Recurses over `value`, replacing each number with `leaf`'s value; other leaves are untouched.
pub fn rewrite_numbers(value: Value, leaf: &dyn Fn(Number) -> Value) -> Value {
    match value {
        Value::Number(number) => leaf(number),
        Value::Array(items) => Value::Array(items.into_iter().map(|item| rewrite_numbers(item, leaf)).collect()),
        Value::Object(fields) => Value::Object(fields.into_iter().map(|(key, item)| (key, rewrite_numbers(item, leaf))).collect()),
        other => other,
    }
}

/// JavaScript has one number type: an integral double is the integer it equals.
fn canonical(value: Value) -> Value {
    rewrite_numbers(value, &|n| n.as_f64().map_or(Value::Number(n), number))
}

/// Pi's `validateToolArguments`: normalize, convert, check, and on failure the message the model reads.
pub fn validate_tool_arguments(parameters: &Value, name: &str, arguments: &Value) -> Result<Value, String> {
    let mut args = arguments.clone();
    normalize_optional_nulls(&mut args, parameters);
    let args = canonical(convert(parameters, args));
    let errors = check(parameters, &args, "");
    if errors.is_empty() {
        return Ok(args);
    }
    let listed: Vec<String> = errors.iter().map(|(path, message)| format!("  - {path}: {message}")).collect();
    Err(format!("Validation failed for tool \"{name}\":\n{}\n\nReceived arguments:\n{}", listed.join("\n"), js_pretty(arguments, 0)))
}
