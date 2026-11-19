#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SseMessage {
    pub event: Option<String>,
    pub data: String,
}

/// Lines end at `\r\n`, `\n` or a lone `\r`; a `\r` at the end of a chunk waits for the next chunk.
#[derive(Default)]
pub struct SseDecoder {
    buffer: Vec<u8>,
    event: Option<String>,
    data: Vec<String>,
}

impl SseDecoder {
    pub fn new() -> SseDecoder {
        SseDecoder::default()
    }

    pub fn push(&mut self, chunk: &[u8]) -> Vec<SseMessage> {
        self.buffer.extend_from_slice(chunk);
        self.messages(false)
    }

    pub fn flush(&mut self) -> Vec<SseMessage> {
        self.messages(true)
    }

    fn messages(&mut self, flushing: bool) -> Vec<SseMessage> {
        let mut messages = Vec::new();
        while let Some(line) = self.next_line(flushing) {
            messages.extend(self.line(&line));
        }
        messages
    }

    /// Pi's Anthropic reader also yields an event left without its closing blank line.
    pub fn finish(&mut self) -> Vec<SseMessage> {
        let mut messages = self.flush();
        messages.extend(self.take_message());
        messages
    }

    fn take_message(&mut self) -> Option<SseMessage> {
        if self.event.is_none() && self.data.is_empty() {
            return None;
        }
        let message = SseMessage { event: self.event.take(), data: self.data.join("\n") };
        self.data.clear();
        Some(message)
    }

    fn next_line(&mut self, flushing: bool) -> Option<String> {
        let end = self.buffer.iter().position(|byte| b"\n\r".contains(byte)).unwrap_or(self.buffer.len());
        let next = self.buffer.get(end + 1);
        let skip = match self.buffer.get(end) {
            Some(b'\n') => 1,
            Some(_) if next == Some(&b'\n') => 2,
            Some(_) if next.is_some() || flushing => 1,
            _ if flushing && end > 0 => 0,
            _ => return None,
        };
        let line = String::from_utf8_lossy(&self.buffer[..end]).into_owned();
        self.buffer.drain(..end + skip);
        Some(line)
    }

    fn line(&mut self, line: &str) -> Option<SseMessage> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.take_message();
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = line.split_once(':').unwrap_or((line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "event" => self.event = Some(value.to_owned()),
            "data" => self.data.push(value.to_owned()),
            _ => {}
        }
        None
    }
}
