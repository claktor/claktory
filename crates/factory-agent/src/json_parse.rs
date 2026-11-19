use serde_json::{Map, Value};

const VALID_ESCAPES: &[u8] = b"\"\\/bfnrtu";

fn escape_control(byte: u8) -> String {
    match byte {
        b'\x08' => "\\b".into(),
        b'\x0c' => "\\f".into(),
        b'\n' => "\\n".into(),
        b'\r' => "\\r".into(),
        b'\t' => "\\t".into(),
        other => format!("\\u{other:04x}"),
    }
}

/// Escapes raw control characters inside strings and doubles backslashes before invalid escapes.
pub fn repair_json(json: &str) -> String {
    let bytes = json.as_bytes();
    let mut repaired = Vec::with_capacity(bytes.len());
    let mut in_string = false;
    let mut index = 0;
    while index < bytes.len() {
        let byte = bytes[index];
        if !in_string {
            repaired.push(byte);
            in_string = byte == b'"';
            index += 1;
            continue;
        }
        if byte == b'"' {
            repaired.push(byte);
            in_string = false;
            index += 1;
            continue;
        }
        if byte == b'\\' {
            let Some(&next) = bytes.get(index + 1) else {
                repaired.extend_from_slice(b"\\\\");
                index += 1;
                continue;
            };
            if next == b'u' && bytes.get(index + 2..index + 6).is_some_and(|digits| digits.iter().all(u8::is_ascii_hexdigit)) {
                repaired.extend_from_slice(&bytes[index..index + 6]);
                index += 6;
                continue;
            }
            if VALID_ESCAPES.contains(&next) {
                repaired.extend_from_slice(&bytes[index..index + 2]);
                index += 2;
                continue;
            }
            repaired.extend_from_slice(b"\\\\");
            index += 1;
            continue;
        }
        if byte <= 0x1f {
            repaired.extend_from_slice(escape_control(byte).as_bytes());
        } else {
            repaired.push(byte);
        }
        index += 1;
    }
    String::from_utf8(repaired).unwrap_or_else(|_| json.to_owned())
}

pub fn parse_json_with_repair(json: &str) -> Result<Value, serde_json::Error> {
    serde_json::from_str(json).or_else(|error| {
        let repaired = repair_json(json);
        if repaired == json { Err(error) } else { serde_json::from_str(&repaired) }
    })
}

/// Always a value, even for incomplete JSON: an object when nothing can be salvaged.
pub fn parse_streaming_json(partial: &str) -> Value {
    if partial.trim().is_empty() {
        return Value::Object(Map::new());
    }
    parse_json_with_repair(partial)
        .or_else(|_| partial_parse(partial))
        .or_else(|()| partial_parse(&repair_json(partial)))
        .unwrap_or_else(|()| Value::Object(Map::new()))
}

struct Partial<'a> {
    text: &'a str,
    bytes: &'a [u8],
    index: usize,
}

/// `partial-json`'s parser with every type allowed to be partial; `Err` is its PartialJSON or MalformedJSON throw.
fn partial_parse(text: &str) -> Result<Value, ()> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Err(());
    }
    let mut parser = Partial { text: trimmed, bytes: trimmed.as_bytes(), index: 0 };
    parser.any()
}

fn js_json(text: &str) -> Result<Value, ()> {
    serde_json::from_str(text).map_err(|_| ())
}

impl Partial<'_> {
    fn byte(&self) -> Option<u8> {
        self.bytes.get(self.index).copied()
    }

    fn rest(&self) -> &str {
        &self.text[self.index..]
    }

    fn skip_blank(&mut self) {
        while self.byte().is_some_and(|byte| b" \n\r\t".contains(&byte)) {
            self.index += 1;
        }
    }

    fn literal(&mut self, word: &str, value: Value) -> Option<Value> {
        let rest = self.rest();
        let remaining = self.bytes.len() - self.index;
        if rest.starts_with(word) || (remaining < word.len() && word.starts_with(rest)) {
            self.index += word.len();
            return Some(value);
        }
        None
    }

    fn any(&mut self) -> Result<Value, ()> {
        self.skip_blank();
        match self.byte() {
            None => Err(()),
            Some(b'"') => self.string(),
            Some(b'{') => self.object(),
            Some(b'[') => self.array(),
            Some(_) => {
                for (word, value) in
                    [("null", Value::Null), ("true", Value::Bool(true)), ("false", Value::Bool(false)), ("Infinity", Value::Null)]
                {
                    if let Some(value) = self.literal(word, value) {
                        return Ok(value);
                    }
                }
                let remaining = self.bytes.len() - self.index;
                let rest = self.rest();
                if rest.starts_with("-Infinity") || (remaining > 1 && remaining < 9 && "-Infinity".starts_with(rest)) {
                    self.index += 9;
                    return Ok(Value::Null);
                }
                if let Some(value) = self.literal("NaN", Value::Null) {
                    return Ok(value);
                }
                self.number()
            }
        }
    }

    fn string(&mut self) -> Result<Value, ()> {
        let start = self.index;
        let mut escape = false;
        self.index += 1;
        while let Some(byte) = self.byte() {
            if byte == b'"' && !(escape && self.bytes[self.index - 1] == b'\\') {
                break;
            }
            escape = if byte == b'\\' { !escape } else { false };
            self.index += 1;
        }
        let escaped = usize::from(escape);
        if self.byte() == Some(b'"') {
            self.index += 1;
            return js_json(&self.text[start..self.index - escaped]);
        }
        let closed = format!("{}\"", &self.text[start..self.index - escaped]);
        js_json(&closed).or_else(|()| self.js_json_prefix(start, '\\', "\""))
    }

    /// Retries `text[start..]` cut before the last `marker`, with `suffix` re-appended.
    fn js_json_prefix(&self, start: usize, marker: char, suffix: &str) -> Result<Value, ()> {
        let cut = self.text.rfind(marker).unwrap_or(0).max(start);
        js_json(&format!("{}{suffix}", &self.text[start..cut]))
    }

    fn object(&mut self) -> Result<Value, ()> {
        self.index += 1;
        self.skip_blank();
        let mut object = Map::new();
        while self.byte() != Some(b'}') {
            self.skip_blank();
            if self.index >= self.bytes.len() {
                return Ok(Value::Object(object));
            }
            let Ok(key) = self.string() else { return Ok(Value::Object(object)) };
            self.skip_blank();
            self.index += 1;
            let Ok(value) = self.any() else { return Ok(Value::Object(object)) };
            object.insert(key.as_str().unwrap_or_default().to_owned(), value);
            self.skip_comma();
        }
        self.index += 1;
        Ok(Value::Object(object))
    }

    fn array(&mut self) -> Result<Value, ()> {
        self.index += 1;
        let mut items = Vec::new();
        while self.byte() != Some(b']') {
            match self.any() {
                Ok(value) => items.push(value),
                Err(()) => return Ok(Value::Array(items)),
            }
            self.skip_comma();
        }
        self.index += 1;
        Ok(Value::Array(items))
    }

    fn skip_comma(&mut self) {
        self.skip_blank();
        if self.byte() == Some(b',') {
            self.index += 1;
        }
    }

    fn number(&mut self) -> Result<Value, ()> {
        if self.index == 0 {
            if self.text == "-" {
                return Err(());
            }
            return js_json(self.text).or_else(|()| self.js_json_prefix(0, 'e', ""));
        }
        let start = self.index;
        if self.byte() == Some(b'-') {
            self.index += 1;
        }
        while self.byte().is_some_and(|byte| !b",]}".contains(&byte)) {
            self.index += 1;
        }
        let slice = &self.text[start..self.index];
        js_json(slice).or_else(|()| {
            if slice == "-" {
                return Err(());
            }
            self.js_json_prefix(start, 'e', "")
        })
    }
}
