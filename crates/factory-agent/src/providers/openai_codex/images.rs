//! Images on the ChatGPT subscription, as Codex makes them: a request
//! to Codex's own image endpoint, `…/backend-api/codex/images/generations`,
//! with `gpt-image-2`, on the sign-in's token and the headers its other
//! requests carry. One image per request; nothing is ever sent again by
//! itself (a refusal for content says not to, and a limit won't lift).

use std::time::Duration;

use base64::Engine;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::auth::RequestAuth;
use crate::images::{ImageFailure, ImageRequest, MadeImage};
use crate::providers::openai_codex::oauth::account_id;
use crate::providers::{client, user_agent};

/// The image model Codex uses.
pub const IMAGE_MODEL: &str = "gpt-image-2";
/// An image takes up to about two minutes; past this, it isn't coming.
const TIMEOUT: Duration = Duration::from_secs(180);
/// The most an image may be, as Codex caps it.
const MAX_BYTES: usize = 32 * 1024 * 1024;

/// Where images are made, from the provider's base url (ChatGPT's backend,
/// or a test server's).
pub fn images_url(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let base = base.strip_suffix("/codex/responses").or_else(|| base.strip_suffix("/codex")).unwrap_or(base);
    format!("{base}/codex/images/generations")
}

/// The request's body.
pub fn body(request: &ImageRequest) -> Value {
    let mut body = json!({"model": IMAGE_MODEL, "prompt": request.prompt, "n": 1});
    if let Some(size) = &request.size {
        body["size"] = json!(size);
    }
    if let Some(quality) = &request.quality {
        body["quality"] = json!(quality);
    }
    if request.transparent {
        body["background"] = json!("transparent");
    }
    body
}

/// Makes one image on the sign-in.
pub fn generate(base_url: String, auth: RequestAuth, request: ImageRequest) -> BoxFuture<'static, Result<MadeImage, ImageFailure>> {
    Box::pin(async move {
        let Some(token) = auth.api_key.as_deref() else { return Err(ImageFailure::SignedOut) };
        let Some(account) = account_id(token) else { return Err(ImageFailure::NotOffered) };
        let sent = client()
            .post(images_url(&base_url))
            .bearer_auth(token)
            .header("chatgpt-account-id", account)
            .header("originator", "pi")
            .header("User-Agent", user_agent())
            .timeout(TIMEOUT)
            .json(&body(&request))
            .send()
            .await;
        let response = match sent {
            Ok(response) => response,
            Err(error) => return Err(lost(&error)),
        };
        let status = response.status().as_u16();
        let text = read_capped(response).await?;
        if !(200..300).contains(&status) {
            return Err(failure(status, &text));
        }
        made(&text)
    })
}

/// The most an answer may be: a 32 MB image in base64, and room for the
/// rest of its JSON.
const MAX_ANSWER: usize = MAX_BYTES.div_ceil(3) * 4 + 64 * 1024;

/// The answer's body, read a chunk at a time and given up past
/// `MAX_ANSWER`, so an answer too big is never held whole.
async fn read_capped(mut response: reqwest::Response) -> Result<String, ImageFailure> {
    let too_big = || ImageFailure::Refused("ChatGPT's image was bigger than 32 MB".into());
    if response.content_length().is_some_and(|length| length > MAX_ANSWER as u64) {
        return Err(too_big());
    }
    let mut body = Vec::new();
    loop {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                if body.len() + chunk.len() > MAX_ANSWER {
                    return Err(too_big());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(None) => break,
            Err(error) => return Err(lost(&error)),
        }
    }
    Ok(String::from_utf8_lossy(&body).into_owned())
}

/// A request that got no answer, in a few words.
fn lost(error: &reqwest::Error) -> ImageFailure {
    tracing::warn!(%error, "no answer to an image request");
    ImageFailure::Unreachable(
        if error.is_timeout() {
            "it didn't answer in time"
        } else if error.is_connect() {
            "it couldn't be reached"
        } else {
            "the connection was lost"
        }
        .to_owned(),
    )
}

/// The image in a successful answer: `{"data": [{"b64_json": …}]}`.
pub fn made(text: &str) -> Result<MadeImage, ImageFailure> {
    let answer: Value = serde_json::from_str(text).map_err(|_| ImageFailure::Refused("ChatGPT's answer wasn't readable".into()))?;
    let Some(encoded) = answer["data"][0]["b64_json"].as_str().filter(|encoded| !encoded.trim().is_empty()) else {
        return Err(ImageFailure::Refused("ChatGPT answered with no image".into()));
    };
    if encoded.len() > MAX_BYTES.div_ceil(3) * 4 {
        return Err(ImageFailure::Refused("ChatGPT's image was bigger than 32 MB".into()));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| ImageFailure::Refused("ChatGPT's image wasn't readable".into()))?;
    let model = answer["model"].as_str().filter(|model| !model.is_empty()).unwrap_or(IMAGE_MODEL).to_owned();
    Ok(MadeImage { bytes, model })
}

/// What a refusal means: the plan's limit, a plan without images, the
/// content rules, or else what ChatGPT said.
pub fn failure(status: u16, text: &str) -> ImageFailure {
    let answer: Value = serde_json::from_str(text).unwrap_or(Value::Null);
    let error = if answer["error"].is_object() { &answer["error"] } else { &answer };
    let code = [&error["code"], &error["type"]].into_iter().filter_map(Value::as_str).find(|code| !code.is_empty()).unwrap_or("");
    let code = code.to_ascii_lowercase();
    if code == "usage_not_included" {
        return ImageFailure::NotOnPlan;
    }
    if code == "moderation_blocked" || code == "content_policy_violation" {
        let stage =
            error["moderation_details"]["moderation_stage"].as_str().or_else(|| answer["moderation_details"]["moderation_stage"].as_str());
        return ImageFailure::Declined { output: stage == Some("output") };
    }
    if let Some(resets_at) = super::usage_limit(error) {
        return ImageFailure::Limit { resets_at };
    }
    if status == 429 {
        return ImageFailure::Limit { resets_at: None };
    }
    let message = error["message"].as_str().map(str::trim).filter(|message| !message.is_empty());
    let message = message.map(|message| message.trim_end_matches('.').to_owned()).unwrap_or_else(|| {
        let short: String = text.trim().chars().take(200).collect();
        if short.is_empty() { "no reason given".to_owned() } else { short }
    });
    ImageFailure::Refused(format!("ChatGPT refused it ({status}: {message})"))
}
