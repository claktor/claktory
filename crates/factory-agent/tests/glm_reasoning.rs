#[path = "support/server.rs"]
mod server;

use factory_agent::providers::{catalog::models, openrouter::stream_simple};
use factory_agent::stream::{LlmContext, StreamOptions, ThinkingLevel};
use factory_agent::types::StopReason;
use serde::Deserialize;
use server::{HANG, Served, serve};

#[derive(Deserialize)]
struct Request {
    reasoning: Reasoning,
}

#[derive(Deserialize)]
struct Reasoning {
    effort: String,
}

#[tokio::test]
async fn glm_flash_sends_native_efforts_without_clamping_max_to_high() {
    let server = serve(vec![Served {
        status: 200,
        content_type: "text/event-stream",
        chunks: vec!["data: {\"choices\":[{\"delta\":{\"content\":\"OK\"},\"finish_reason\":\"stop\"}]}\n\n".into()],
        hold: false,
        hold_before_headers: false,
    }])
    .await;
    let mut model = models("openrouter").into_iter().find(|model| model.id == "z-ai/glm-5.3-flash").expect("GLM Flash");
    model.base_url = server.base_url.clone();
    for (level, expected) in [
        (ThinkingLevel::Low, "low"),
        (ThinkingLevel::High, "high"),
        (ThinkingLevel::Max, "max"),
        (ThinkingLevel::Medium, "low"),
        (ThinkingLevel::Xhigh, "max"),
        (ThinkingLevel::Minimal, "low"),
        (ThinkingLevel::Off, "low"),
    ] {
        let options = StreamOptions { reasoning: Some(level), api_key: Some("test".into()), ..Default::default() };
        let stream = stream_simple(&model, LlmContext::default(), options).expect("stream");
        let message = tokio::time::timeout(HANG, stream.result()).await.expect("response").expect("message");
        assert_eq!(message.stop_reason, StopReason::Stop);
        let requests = server.requests.lock();
        let request: Request = serde_json::from_str(&requests.last().expect("request").raw).expect("request JSON");
        assert_eq!(request.reasoning.effort, expected, "{level:?}");
    }
}
