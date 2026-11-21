//! `read_image`: an image file goes into the conversation as a real image
//! block, scaled down when it's too big for what models take, and a model
//! without image input gets a plain refusal.

use std::path::Path;
use std::sync::Arc;

use crate::support::{deepseek, luna, reply};
use base64::Engine;
use factory_agent::agent::{Agent, AgentOptions};
use factory_agent::stream::{StreamFn, ThinkingLevel, event_stream};
use factory_agent::tools::{Tool, coding_tools};
use factory_agent::types::{AssistantPart, MAX_IMAGE_BYTES, Message, Model, StreamEvent, ToolResult, ToolResultMessage, ToolResultPart};
use image::ImageEncoder;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn read_image(dir: &Path) -> Tool {
    coding_tools(dir, &[], None).into_iter().find(|tool| tool.spec.name == "read_image").expect("read_image")
}

async fn run(dir: &Path, args: Value) -> Result<ToolResult, factory_agent::errors::Error> {
    read_image(dir).execute(&args, &CancellationToken::new(), &|_| {}).await
}

/// A PNG of `width`×`height`; `noisy` fills it with pixels PNG can't
/// compress, so its bytes grow with its area.
fn png(width: u32, height: u32, noisy: bool) -> Vec<u8> {
    let mut seed = 0x2545F4914F6CDD1Du64;
    let image = image::RgbImage::from_fn(width, height, |x, y| {
        if noisy {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(x as u64 ^ (y as u64) << 17 | 1);
            let bytes = seed.to_le_bytes();
            image::Rgb([bytes[5], bytes[6], bytes[7]])
        } else {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 7])
        }
    });
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out).write_image(image.as_raw(), width, height, image::ExtendedColorType::Rgb8).expect("png");
    out
}

fn parts(result: &ToolResult) -> (String, Vec<u8>, String) {
    let [ToolResultPart::Text { text }, ToolResultPart::Image { data, mime_type }] = result.content.as_slice() else {
        panic!("a text note then one image, got {:?}", result.content.len());
    };
    (text.clone(), base64::engine::general_purpose::STANDARD.decode(data).expect("base64"), mime_type.clone())
}

#[tokio::test]
async fn an_image_within_bounds_goes_as_its_own_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let original = png(40, 30, false);
    std::fs::write(dir.path().join("shot.png"), &original).unwrap();
    let result = run(dir.path(), json!({"path": "shot.png"})).await.unwrap();
    let (note, bytes, mime) = parts(&result);
    assert_eq!(note, "shot.png (40×30 PNG).");
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, original, "within bounds, the file's own bytes go as they are");
    assert_eq!(result.details, json!({"path": "shot.png", "width": 40, "height": 30, "scaled": false}));
}

#[tokio::test]
async fn a_wide_image_is_scaled_to_the_long_edge_and_says_so() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("wide.png"), png(3000, 1500, false)).unwrap();
    let result = run(dir.path(), json!({"path": "wide.png"})).await.unwrap();
    let (note, bytes, mime) = parts(&result);
    assert_eq!(note, "wide.png (3000×1500 PNG), scaled down to 2048×1024 PNG to fit what your model takes.");
    assert_eq!(mime, "image/png");
    let sent = image::load_from_memory(&bytes).unwrap();
    assert_eq!((sent.width(), sent.height()), (2048, 1024));
    assert_eq!(result.details["scaled"], json!(true));
}

#[tokio::test]
async fn an_image_too_heavy_in_bytes_becomes_a_jpeg_under_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    let original = png(1600, 1600, true);
    assert!(original.len() as u64 > MAX_IMAGE_BYTES, "the noise PNG must outweigh the limit to test anything");
    std::fs::write(dir.path().join("noise.png"), &original).unwrap();
    let result = run(dir.path(), json!({"path": "noise.png"})).await.unwrap();
    let (note, bytes, mime) = parts(&result);
    assert_eq!(mime, "image/jpeg");
    assert!((bytes.len() as u64) <= MAX_IMAGE_BYTES, "{} bytes still over the limit", bytes.len());
    assert!(note.contains("scaled down to"), "{note}");
    // Pure noise is the worst case JPEG knows: the encoder may have had to
    // shrink it too. What matters is that something readable under the limit
    // came out, without the agent doing anything.
    let sent = image::load_from_memory(&bytes).unwrap();
    assert!(sent.width() <= 1600 && sent.height() <= 1600);
}

#[tokio::test]
async fn a_file_that_is_not_an_image_is_refused_with_the_reason() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "just words").unwrap();
    let error = run(dir.path(), json!({"path": "notes.txt"})).await.unwrap_err();
    assert_eq!(error.to_string(), "notes.txt isn't an image models read (PNG, JPEG, GIF or WebP). For text, use read_file.");
}

fn options(cwd: &Path, model: Model, stream: StreamFn) -> AgentOptions {
    AgentOptions {
        system_prompt: format!("You are a coding agent. Work in {}.", cwd.display()),
        model,
        thinking_level: ThinkingLevel::Medium,
        tools: coding_tools(cwd, &[], None),
        messages: Vec::new(),
        stream,
        session_id: Some("session".into()),
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: None,
        fallback_model: None,
    }
}

/// Answers the first request with one read_image call on `path`, later ones with plain text.
fn calls_read_image(path: &str) -> StreamFn {
    let path = path.to_owned();
    let calls = Arc::new(Mutex::new(0));
    Arc::new(move |model, _context, _options| {
        let (sink, stream) = event_stream();
        *calls.lock() += 1;
        let content = if *calls.lock() == 1 {
            vec![AssistantPart::ToolCall {
                id: "call-1".into(),
                name: "read_image".into(),
                arguments: json!({"path": path}),
                thought_signature: None,
            }]
        } else {
            vec![AssistantPart::Text { text: "Done".into(), text_signature: None }]
        };
        let message = reply(model, content);
        sink.push(StreamEvent::Start { partial: message.clone() });
        sink.push(StreamEvent::Done { reason: message.stop_reason, message });
        Ok(stream)
    })
}

fn tool_result(message: &Message) -> &ToolResultMessage {
    let Message::ToolResult(result) = message else { panic!("tool result, got {message:?}") };
    result
}

#[tokio::test]
async fn a_model_without_image_input_hears_a_plain_refusal() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), png(40, 30, false)).unwrap();
    let mut agent = Agent::new(options(dir.path(), deepseek(), calls_read_image("shot.png")));
    agent.prompt("look").await.unwrap();
    let result = tool_result(&agent.state.messages[2]);
    let [ToolResultPart::Text { text }] = result.content.as_slice() else { panic!("one text part") };
    assert_eq!(text, "Your model, DeepSeek: DeepSeek V4 Flash 0731, can't read images.");
}

#[tokio::test]
async fn a_model_with_image_input_gets_the_image_block() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("shot.png"), png(40, 30, false)).unwrap();
    let mut agent = Agent::new(options(dir.path(), luna(), calls_read_image("shot.png")));
    agent.prompt("look").await.unwrap();
    let result = tool_result(&agent.state.messages[2]);
    assert!(
        result.content.iter().any(|part| matches!(part, ToolResultPart::Image { mime_type, .. } if mime_type == "image/png")),
        "the result carries the image: {:?}",
        result.content
    );
}
