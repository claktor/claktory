//! Framing, both ways: a u32 length, then one UTF-8 JSON object. Chrome's
//! native messaging puts the length in the machine's own byte order; the
//! socket to the app, little-endian. Every frame is refused past
//! [`FRAME_LIMIT`] before anything is allocated for it, and a frame that's
//! empty, isn't JSON, has bytes after its object or isn't an object is
//! refused too.

use std::io::{ErrorKind, Read, Write};

use serde_json::{Map, Value};

/// The most any frame may hold, either way (Chrome allows 64 MiB to a host).
pub const FRAME_LIMIT: usize = 256 * 1024;

/// How a side puts the length.
#[derive(Clone, Copy, Debug)]
pub enum Order {
    /// Chrome's native messaging.
    Native,
    /// The app's socket.
    Little,
}

/// Why a frame was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Bad {
    /// The other side closed in the middle of one.
    Truncated,
    Empty,
    TooBig(usize),
    NotJson,
    NotObject,
    /// Reading failed.
    Io(ErrorKind),
}

impl std::fmt::Display for Bad {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Bad::Truncated => write!(f, "a frame was cut short"),
            Bad::Empty => write!(f, "an empty frame"),
            Bad::TooBig(len) => write!(f, "a frame of {len} bytes, past {FRAME_LIMIT}"),
            Bad::NotJson => write!(f, "a frame that isn't one JSON value"),
            Bad::NotObject => write!(f, "a frame that isn't a JSON object"),
            Bad::Io(kind) => write!(f, "reading failed ({kind})"),
        }
    }
}

/// The next frame's object; None when the other side closed between frames.
pub fn read(input: &mut impl Read, order: Order) -> Result<Option<Map<String, Value>>, Bad> {
    let mut len = [0u8; 4];
    let mut got = 0;
    while got < len.len() {
        match input.read(&mut len[got..]) {
            Ok(0) if got == 0 => return Ok(None),
            Ok(0) => return Err(Bad::Truncated),
            Ok(n) => got += n,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(Bad::Io(error.kind())),
        }
    }
    let len = match order {
        Order::Native => u32::from_ne_bytes(len),
        Order::Little => u32::from_le_bytes(len),
    } as usize;
    if len == 0 {
        return Err(Bad::Empty);
    }
    if len > FRAME_LIMIT {
        return Err(Bad::TooBig(len));
    }
    let mut frame = vec![0; len];
    input.read_exact(&mut frame).map_err(|error| match error.kind() {
        ErrorKind::UnexpectedEof => Bad::Truncated,
        kind => Bad::Io(kind),
    })?;
    match serde_json::from_slice::<Value>(&frame) {
        Ok(Value::Object(object)) => Ok(Some(object)),
        Ok(_) => Err(Bad::NotObject),
        Err(_) => Err(Bad::NotJson),
    }
}

/// Writes `object` as one frame; refused, writing nothing, past the limit.
pub fn write(output: &mut impl Write, order: Order, object: &Value) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(object)?;
    if bytes.is_empty() || bytes.len() > FRAME_LIMIT {
        return Err(std::io::Error::new(ErrorKind::InvalidData, format!("a frame of {} bytes", bytes.len())));
    }
    let len = bytes.len() as u32;
    let len = match order {
        Order::Native => len.to_ne_bytes(),
        Order::Little => len.to_le_bytes(),
    };
    // One write, so a frame is never interleaved with another thread's.
    let mut whole = Vec::with_capacity(4 + bytes.len());
    whole.extend_from_slice(&len);
    whole.extend_from_slice(&bytes);
    output.write_all(&whole)?;
    output.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn framed(order: Order, body: &[u8]) -> Vec<u8> {
        let len = body.len() as u32;
        let mut out = match order {
            Order::Native => len.to_ne_bytes().to_vec(),
            Order::Little => len.to_le_bytes().to_vec(),
        };
        out.extend_from_slice(body);
        out
    }

    #[test]
    fn frames_are_one_object_within_the_limit_and_refused_before_allocating() {
        let read_one = |bytes: Vec<u8>| read(&mut bytes.as_slice(), Order::Little);
        assert_eq!(read_one(framed(Order::Little, br#"{"a":1}"#)).unwrap().unwrap()["a"], 1);
        assert_eq!(read_one(Vec::new()), Ok(None), "closed between frames");
        assert_eq!(read_one(vec![1, 0]), Err(Bad::Truncated));
        assert_eq!(read_one(framed(Order::Little, b"")), Err(Bad::Empty));
        assert_eq!(read_one(framed(Order::Little, b"[1]")), Err(Bad::NotObject));
        assert_eq!(read_one(framed(Order::Little, br#"{"a":1} {"b":2}"#)), Err(Bad::NotJson), "nothing after the object");
        assert_eq!(read_one(framed(Order::Little, b"\xff{}")), Err(Bad::NotJson));
        // A length past the limit is refused with no body sent at all: the
        // frame isn't waited for, so nothing was allocated for it.
        let len = (FRAME_LIMIT as u32 + 1).to_le_bytes().to_vec();
        assert_eq!(read_one(len), Err(Bad::TooBig(FRAME_LIMIT + 1)));
        assert_eq!(read_one(u32::MAX.to_le_bytes().to_vec()), Err(Bad::TooBig(u32::MAX as usize)));
        let mut short = framed(Order::Little, br#"{"a":1}"#);
        short.pop();
        assert_eq!(read_one(short), Err(Bad::Truncated));

        let mut out = Vec::new();
        write(&mut out, Order::Native, &serde_json::json!({"type": "x"})).unwrap();
        assert_eq!(read(&mut out.as_slice(), Order::Native).unwrap().unwrap()["type"], "x");
        let big = serde_json::json!({"text": "a".repeat(FRAME_LIMIT)});
        let mut out = Vec::new();
        assert!(write(&mut out, Order::Little, &big).is_err());
        assert!(out.is_empty());
    }
}
