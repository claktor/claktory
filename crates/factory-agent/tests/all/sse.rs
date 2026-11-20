use factory_agent::sse::{SseDecoder, SseMessage};

fn message(event: Option<&str>, data: &str) -> SseMessage {
    SseMessage { event: event.map(str::to_owned), data: data.to_owned() }
}

#[test]
fn decodes_events_split_at_any_byte_boundary() {
    let text = "event: a\ndata: {\"x\":1}\n\n: comment\r\n\r\nevent: b\r\ndata: one\r\ndata: two\r\n\r\ndata:[DONE]\n\n";
    let expected = vec![message(Some("a"), "{\"x\":1}"), message(Some("b"), "one\ntwo"), message(None, "[DONE]")];
    for width in 1..=text.len() {
        let mut decoder = SseDecoder::new();
        let mut seen = Vec::new();
        for chunk in text.as_bytes().chunks(width) {
            seen.extend(decoder.push(chunk));
        }
        seen.extend(decoder.flush());
        assert_eq!(seen, expected, "width {width}");
    }
}

/// As in the SDK, a `\r` at the end of a chunk waits for a possible `\n`, and a message needs its blank line even at the end.
#[test]
fn a_lone_carriage_return_waits_and_a_trailing_message_needs_a_blank_line() {
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push(b"data: a\rdata: b\r"), vec![]);
    assert_eq!(decoder.push(b"\r"), vec![]);
    assert_eq!(decoder.push(b"data: tail"), vec![message(None, "a\nb")]);
    assert_eq!(decoder.flush(), vec![]);
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push(b"data: x\r"), vec![]);
    assert_eq!(decoder.flush(), vec![]);
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push(b"data: x\n"), vec![]);
    assert_eq!(decoder.push(b"\n"), vec![message(None, "x")]);
}

#[test]
fn unknown_fields_and_leading_spaces_follow_the_sdk() {
    let mut decoder = SseDecoder::new();
    let seen = decoder.push(b"id: 1\nretry: 5\ndata:  two spaces\nevent:x\n\n\n\n");
    assert_eq!(seen, vec![message(Some("x"), " two spaces")]);
}

/// Pi's Anthropic reader also yields an event left without its closing blank line.
#[test]
fn finish_yields_a_trailing_event() {
    let mut decoder = SseDecoder::new();
    assert_eq!(decoder.push(b"event: message_stop\ndata: {}"), vec![]);
    assert_eq!(decoder.finish(), vec![message(Some("message_stop"), "{}")]);
    assert_eq!(decoder.finish(), vec![]);
}
