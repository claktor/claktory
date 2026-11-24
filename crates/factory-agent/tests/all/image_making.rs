//! Images made on ChatGPT's subscription (#403), against a mock of Codex's
//! image endpoint: the request Codex itself sends, the image that comes
//! back, and what each refusal means. Which models can make them is read
//! from the sign-in, never a list.

use std::io::Cursor;
use std::sync::Arc;

use base64::Engine;
use factory_agent::auth::Env;
use factory_agent::clock::now_ms;
use factory_agent::credentials::FileCredentialStore;
use factory_agent::images::{ImageFailure, ImageRequest};
use factory_agent::providers::models::{Registry, default_providers};
use factory_agent::providers::openai_codex::images::{failure, images_url, made};
use serde_json::{Value, json};

use crate::support::server::{serve, served_json};

fn codex_token(user: &str) -> String {
    let encode = |value: Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value.to_string());
    let claims = json!({
        "https://api.openai.com/auth": {"chatgpt_account_id": format!("account-{user}"), "chatgpt_user_id": format!("user-{user}"), "chatgpt_plan_type": "plus"},
        "https://api.openai.com/profile": {"email": format!("{user}@x.com")},
    });
    format!("{}.{}.sig", encode(json!({"alg": "none"})), encode(claims))
}

/// A registry signed in to ChatGPT (or not), whose providers are at `base_url`.
fn registry(dir: &std::path::Path, base_url: &str, signed_in: bool) -> Registry {
    let file = dir.join("auth.json");
    if signed_in {
        let access = codex_token("a");
        let account = json!({"type": "oauth", "access": access, "refresh": "refresh-a", "expires": now_ms() + 3_600_000, "factoryAccount": "a", "factoryLabel": "a@x.com"});
        std::fs::write(&file, json!({"openai-codex": account}).to_string()).expect("credentials");
    }
    let base: &'static str = Box::leak(base_url.to_owned().into_boxed_str());
    let mut providers = default_providers();
    for provider in &mut providers {
        provider.base_url = base;
    }
    let env: Env = Arc::new(|_| None);
    Registry::with_providers(FileCredentialStore::new(file), env, providers)
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 40, 40, 255]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

#[tokio::test]
async fn an_image_is_made_on_the_chatgpt_sign_in_as_codex_asks_for_one() {
    let dir = tempfile::tempdir().expect("dir");
    let picture = png(4, 3);
    let encoded = base64::engine::general_purpose::STANDARD.encode(&picture);
    let server = serve(vec![served_json(200, json!({"created": 1, "data": [{"b64_json": encoded}]}))]).await;
    let registry = registry(dir.path(), &format!("{}/backend-api", server.base_url), true);
    let request = ImageRequest {
        prompt: "a red fox in a snowy field, watercolor".into(),
        size: Some("1024x1024".into()),
        quality: Some("low".into()),
        transparent: true,
    };

    let made = registry.make_image("openai-codex", request).await.expect("an image");
    assert_eq!(made.bytes, picture);
    assert_eq!(made.model, "gpt-image-2");
    let requests = server.requests.lock().clone();
    assert_eq!(requests.len(), 1, "one request, never sent again");
    let sent = &requests[0];
    assert_eq!((sent.method.as_str(), sent.path.as_str()), ("POST", "/backend-api/codex/images/generations"));
    let header = |name: &str| sent.headers.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone()).unwrap_or_default();
    assert_eq!(header("authorization"), format!("Bearer {}", codex_token("a")));
    assert_eq!(header("chatgpt-account-id"), "account-a");
    assert_eq!(header("originator"), "pi");
    assert_eq!(
        sent.body,
        json!({"model": "gpt-image-2", "prompt": "a red fox in a snowy field, watercolor", "n": 1, "size": "1024x1024", "quality": "low", "background": "transparent"})
    );

    // Left to the provider when not asked for.
    let server = serve(vec![served_json(200, json!({"data": [{"b64_json": encoded}]}))]).await;
    let registry = self::registry(dir.path(), &format!("{}/backend-api", server.base_url), true);
    registry.make_image("openai-codex", ImageRequest { prompt: "a fox".into(), ..ImageRequest::default() }).await.expect("an image");
    assert_eq!(server.requests.lock()[0].body, json!({"model": "gpt-image-2", "prompt": "a fox", "n": 1}));
}

#[tokio::test]
async fn only_models_that_take_images_on_a_chatgpt_sign_in_make_them() {
    let dir = tempfile::tempdir().expect("dir");
    let registry = registry(dir.path(), "http://127.0.0.1:9", true);
    let models = registry.models().await.expect("models");
    let codex: Vec<_> = models.iter().filter(|model| model.id.starts_with("openai-codex/")).collect();
    assert!(codex.iter().any(|model| model.image_output), "{codex:?}");
    for model in &models {
        // Codex's own rule: image input, on the subscription.
        let expected = model.id.starts_with("openai-codex/") && model.image_input;
        assert_eq!(model.image_output, expected, "{}", model.id);
    }
    // Signed out: nothing to make them on.
    let other = tempfile::tempdir().expect("dir");
    let registry = self::registry(other.path(), "http://127.0.0.1:9", false);
    let failed = registry.make_image("openai-codex", ImageRequest { prompt: "a fox".into(), ..ImageRequest::default() }).await;
    assert_eq!(failed, Err(ImageFailure::SignedOut));
    // A provider that makes none.
    let failed = registry.make_image("anthropic", ImageRequest { prompt: "a fox".into(), ..ImageRequest::default() }).await;
    assert_eq!(failed, Err(ImageFailure::NotOffered));
}

#[tokio::test]
async fn a_refusal_comes_back_once_with_what_it_means() {
    let dir = tempfile::tempdir().expect("dir");
    let resets = now_ms() / 1000 + 3600;
    let limit = json!({"error": {"type": "usage_limit_reached", "message": "The usage limit has been reached", "resets_at": resets}});
    let server = serve(vec![served_json(429, limit)]).await;
    let registry = registry(dir.path(), &format!("{}/backend-api", server.base_url), true);
    let failed = registry.make_image("openai-codex", ImageRequest { prompt: "a fox".into(), ..ImageRequest::default() }).await;
    assert_eq!(failed, Err(ImageFailure::Limit { resets_at: Some(resets * 1000) }));
    assert_eq!(server.requests.lock().len(), 1, "never sent again by itself");
}

#[test]
fn refusals_read_as_what_they_mean() {
    let declined = |stage: &str| {
        json!({"error": {"code": "moderation_blocked", "message": "Your request was rejected", "moderation_details": {"moderation_stage": stage}}})
            .to_string()
    };
    assert_eq!(failure(400, &declined("input")), ImageFailure::Declined { output: false });
    assert_eq!(failure(400, &declined("output")), ImageFailure::Declined { output: true });
    let not_included = json!({"error": {"type": "usage_not_included", "message": "Upgrade"}}).to_string();
    assert_eq!(failure(403, &not_included), ImageFailure::NotOnPlan);
    assert_eq!(failure(429, ""), ImageFailure::Limit { resets_at: None });
    let broken = json!({"error": {"message": "Something broke."}}).to_string();
    assert_eq!(failure(500, &broken), ImageFailure::Refused("ChatGPT refused it (500: Something broke)".into()));
    assert_eq!(failure(502, "Bad gateway"), ImageFailure::Refused("ChatGPT refused it (502: Bad gateway)".into()));
    assert_eq!(made(r#"{"data": []}"#), Err(ImageFailure::Refused("ChatGPT answered with no image".into())));
    assert_eq!(made("not json"), Err(ImageFailure::Refused("ChatGPT's answer wasn't readable".into())));
}

#[test]
fn images_go_to_codexs_endpoint_whatever_the_base_says() {
    assert_eq!(images_url("https://chatgpt.com/backend-api"), "https://chatgpt.com/backend-api/codex/images/generations");
    assert_eq!(images_url("https://chatgpt.com/backend-api/"), "https://chatgpt.com/backend-api/codex/images/generations");
    assert_eq!(images_url("https://chatgpt.com/backend-api/codex"), "https://chatgpt.com/backend-api/codex/images/generations");
    assert_eq!(images_url("https://chatgpt.com/backend-api/codex/responses"), "https://chatgpt.com/backend-api/codex/images/generations");
}

#[tokio::test]
async fn an_answer_bigger_than_an_image_can_be_is_never_held_whole() {
    let dir = tempfile::tempdir().expect("dir");
    let huge = format!("{{\"data\": [{{\"b64_json\": \"{}\"}}]}}", "A".repeat(45 * 1024 * 1024));
    let server = serve(vec![crate::support::server::served_text(200, &huge)]).await;
    let registry = registry(dir.path(), &format!("{}/backend-api", server.base_url), true);
    let failed = registry.make_image("openai-codex", ImageRequest { prompt: "a fox".into(), ..ImageRequest::default() }).await;
    assert_eq!(failed, Err(ImageFailure::Refused("ChatGPT's image was bigger than 32 MB".into())));
}
