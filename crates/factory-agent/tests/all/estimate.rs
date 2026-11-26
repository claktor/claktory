use factory_agent::estimate::{
    Estimate, ImageRule, estimate_context, estimate_context_tokens, estimate_message_tokens, estimate_message_tokens_for, estimate_messages,
    estimate_text_tokens, image_tokens,
};
use factory_agent::types::{
    AssistantMessage, AssistantPart, ImageData, Message, StopReason, ToolResultMessage, ToolResultPart, Usage, UserContent, UserMessage,
    UserPart,
};
use serde_json::json;

fn assistant(parts: Vec<AssistantPart>, total_tokens: u64, stop_reason: StopReason, timestamp: u64) -> Message {
    let usage = Usage { total_tokens, ..Usage::empty() };
    Message::Assistant(AssistantMessage {
        content: parts,
        api: "a".into(),
        provider: "p".into(),
        model: "m".into(),
        usage,
        stop_reason,
        error_message: None,
        timestamp,
        extra: Default::default(),
    })
}

fn tool_result(text: &str, timestamp: u64) -> Message {
    Message::ToolResult(ToolResultMessage {
        tool_call_id: "c".into(),
        tool_name: "bash".into(),
        content: vec![ToolResultPart::Text { text: text.into() }],
        details: json!({}),
        usage: None,
        is_error: false,
        timestamp,
        started: None,
    })
}

#[test]
fn text_tokens_count_utf16_units_in_fours() {
    assert_eq!(estimate_text_tokens(""), 0);
    assert_eq!(estimate_text_tokens("abcd"), 1);
    assert_eq!(estimate_text_tokens("abcde"), 2);
    assert_eq!(estimate_text_tokens("😀😀😀😀"), 2);
    let call =
        AssistantPart::ToolCall { id: "1".into(), name: "bash".into(), arguments: json!({"command": "ls"}), thought_signature: None };
    assert_eq!(estimate_message_tokens(&assistant(vec![call], 0, StopReason::ToolUse, 1)), (4 + 16_u64).div_ceil(4));
    assert_eq!(estimate_message_tokens(&Message::User(UserMessage::text("12345", 1))), 2);
}

#[test]
fn messages_after_the_last_usable_usage_are_estimated_on_top_of_it() {
    let messages = vec![
        Message::User(UserMessage::text("x".repeat(400), 1)),
        assistant(vec![AssistantPart::Text { text: "y".repeat(40), text_signature: None }], 1000, StopReason::Stop, 2),
        tool_result(&"z".repeat(80), 3),
        assistant(vec![AssistantPart::Text { text: "aborted".into(), text_signature: None }], 2000, StopReason::Aborted, 4),
    ];
    assert_eq!(estimate_messages(&messages), Estimate { tokens: 1022, usage_tokens: 1000, trailing_tokens: 22, last_usage_index: Some(1) });
    assert_eq!(estimate_context(Some("prompt"), &messages, Some(&json!([{"name": "bash"}]))).tokens, 1022);
    let fresh = vec![Message::User(UserMessage::text("x".repeat(400), 1))];
    assert_eq!(estimate_messages(&fresh), Estimate { tokens: 100, usage_tokens: 0, trailing_tokens: 100, last_usage_index: None });
    let with_prefix = estimate_context(Some("12345678"), &fresh, Some(&json!([{"name": "bash"}])));
    assert_eq!(with_prefix.tokens, 100 + 2 + estimate_text_tokens(r#"[{"name":"bash"}]"#));
    assert_eq!(estimate_context(Some(""), &fresh, Some(&json!([]))).tokens, 100);
    let stale = vec![
        assistant(vec![], 500, StopReason::Stop, 1),
        Message::User(UserMessage::text("later prefix", 5)),
        assistant(vec![], 700, StopReason::Stop, 3),
    ];
    assert_eq!(estimate_messages(&stale).last_usage_index, Some(0));
}

#[test]
fn the_headless_estimate_takes_the_last_usable_usage_regardless_of_timestamps() {
    let stale = vec![
        assistant(vec![], 500, StopReason::Stop, 1),
        Message::User(UserMessage::text("later prefix", 5)),
        assistant(vec![], 700, StopReason::Stop, 3),
        tool_result("zzzz", 6),
    ];
    assert_eq!(estimate_context_tokens(&stale), Estimate { tokens: 701, usage_tokens: 700, trailing_tokens: 1, last_usage_index: Some(2) });
    assert_eq!(estimate_messages(&stale).last_usage_index, Some(0));
    let unusable = vec![assistant(vec![AssistantPart::Text { text: "abcd".into(), text_signature: None }], 900, StopReason::Aborted, 1)];
    assert_eq!(estimate_context_tokens(&unusable), Estimate { tokens: 1, usage_tokens: 0, trailing_tokens: 1, last_usage_index: None });
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::DynamicImage::new_rgb8(width, height).write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png).expect("png");
    bytes
}

fn with_image(data: ImageData) -> Message {
    Message::User(UserMessage {
        content: UserContent::Parts(vec![UserPart::Image { data, mime_type: "image/png".into() }]),
        timestamp: 1,
        ..UserMessage::text("", 1)
    })
}

#[test]
fn an_image_costs_what_its_provider_charges_for_its_size() {
    // Anthropic: width × height / 750, scaled to a long edge of 1568 and 1600 tokens.
    assert_eq!(image_tokens(ImageRule::Anthropic, 200, 200), 54);
    assert_eq!(image_tokens(ImageRule::Anthropic, 1000, 1000), 1334);
    assert!((1590..=1600).contains(&image_tokens(ImageRule::Anthropic, 1092, 1092)));
    assert!(image_tokens(ImageRule::Anthropic, 8000, 6000) <= 1600);
    // OpenAI: fitted in 2048, short side to 768, 170 a 512-pixel tile and 85.
    assert_eq!(image_tokens(ImageRule::OpenAi, 1024, 1024), 765);
    assert_eq!(image_tokens(ImageRule::OpenAi, 2048, 4096), 1105);
    assert_eq!(image_tokens(ImageRule::OpenAi, 100, 100), 255);
    // Google: 258 within 384×384, else 258 a 768-pixel tile.
    assert_eq!(image_tokens(ImageRule::Gemini, 300, 300), 258);
    assert_eq!(image_tokens(ImageRule::Gemini, 1000, 1000), 1032);

    assert_eq!(ImageRule::for_model("anthropic-messages", "claude-opus-4-8"), ImageRule::Anthropic);
    assert_eq!(ImageRule::for_model("openai-completions", "anthropic/claude-sonnet-4-6"), ImageRule::Anthropic);
    assert_eq!(ImageRule::for_model("openai-completions", "google/gemini-2.5-pro"), ImageRule::Gemini);
    assert_eq!(ImageRule::for_model("openai-codex-responses", "gpt-5.5"), ImageRule::OpenAi);
}

#[test]
fn images_in_a_conversation_are_estimated_by_their_size_never_their_bytes() {
    use base64::Engine as _;
    let bytes = png(2400, 1800);
    let inline = ImageData::from(base64::engine::general_purpose::STANDARD.encode(&bytes));
    assert_eq!(inline.dimensions(), Some((2400, 1800)));
    let full = image_tokens(ImageRule::Anthropic, 2400, 1800);
    assert_eq!(estimate_message_tokens(&with_image(inline.clone())), full);
    assert_eq!(estimate_message_tokens_for(&with_image(inline), ImageRule::OpenAi), image_tokens(ImageRule::OpenAi, 2400, 1800));

    // Stored in a file, as after a restart: read from the file's header.
    let dir = std::env::temp_dir().join(format!("estimate-images-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("dir");
    let path = dir.join("a.png");
    std::fs::write(&path, &bytes).expect("write");
    let three: Vec<Message> = (0..3).map(|_| with_image(ImageData::stored(path.clone(), "a.png"))).collect();
    let estimate = estimate_context(None, &three, None).tokens;
    std::fs::remove_dir_all(&dir).expect("remove");
    assert_eq!(estimate, 3 * full);
    assert!(estimate < 5000, "three screenshots are a few thousand tokens, not {estimate}");

    // One whose size can't be read costs about what a full-size one does.
    assert_eq!(estimate_message_tokens(&with_image(ImageData::from("bm90IGFuIGltYWdl"))), 1200);
}
