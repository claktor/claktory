//! The Images extension (#403): an agent whose model can make images makes
//! them for the person, with `image_generate`. Claktory runs the tool
//! itself: it asks the image endpoint of the agent's own sign-in (phase 1:
//! ChatGPT's subscription, `factory_agent::providers::openai_codex::images`),
//! keeps the picture in the agent's folder in Claktory's data, never a
//! project's, and returns it to the model as well, so the agent sees what
//! it made. The app draws the call from its details.
//!
//! The tool is in an agent's frame only when its model can make images on
//! its sign-in when the frame is written (`Model.image_output`), so agents
//! on other models pay nothing for it. A model change writes a new frame;
//! a sign-in that changes under the same model (a plan down to Free) leaves
//! the tool there, and a call says why it can't.

use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use factory_agent::images::{ImageFailure, ImageRequest};
use factory_agent::{Tool, ToolResult, ToolResultPart};
use factory_protocol::v1;
use serde::Deserialize;
use serde_json::json;

use super::{Extension, Manifest, PromptSection};
use crate::config::AgentConfig;
use crate::factory::{Factory, Inner};

pub(crate) const ID: &str = "images";
pub(crate) const TOOL: &str = "image_generate";

static MANIFEST: Manifest = Manifest {
    id: ID,
    name: "Images",
    icon: "image",
    description: "Lets it make images for you when its model can: GPT models, signed in with a paid ChatGPT plan.",
    default_enabled: true,
    options: &[],
};

const GENERATE: &str = "Make an image from a description, one per call, with your model's image model on your sign-in (on a ChatGPT plan, it counts against the plan's image limit). It takes up to about two minutes. The image is saved in your agent folder, never in a project: the result gives its path, and shows you the image. The person sees it in your conversation; copy the file where it's wanted. prompt: what to draw, in detail (subject, style, composition, colors, any text in it). size: \"1024x1024\", \"1536x1024\" (landscape), \"1024x1536\" (portrait) or \"auto\". quality: \"low\", \"medium\", \"high\" or \"auto\"; low is quicker. transparent: true for a transparent background. If the description is declined for its content, or a limit is reached, the result says so: don't send it again unchanged; tell the person.";

const SIZES: [&str; 4] = ["1024x1024", "1536x1024", "1024x1536", "auto"];
const QUALITIES: [&str; 4] = ["low", "medium", "high", "auto"];

pub(crate) struct Images;

impl Extension for Images {
    fn manifest(&self) -> &'static Manifest {
        &MANIFEST
    }

    fn prompt(&self, _factory: &Factory, _inner: &Inner, _agent: &AgentConfig) -> Vec<PromptSection> {
        Vec::new()
    }

    fn tools(&self, factory: &Arc<Factory>, agent: &str) -> Vec<Tool> {
        let makes = {
            let inner = factory.lock();
            inner.config.agent(agent).is_some_and(|me| unable(&inner, me).is_none())
        };
        if !makes {
            return Vec::new();
        }
        let parameters = json!({
            "type": "object",
            "required": ["prompt"],
            "properties": {
                "prompt": {"type": "string", "minLength": 1},
                "size": {"type": "string", "enum": SIZES},
                "quality": {"type": "string", "enum": QUALITIES},
                "transparent": {"type": "boolean"},
            },
            "additionalProperties": false,
        });
        vec![super::async_tool(factory, agent, TOOL, "Make an image", GENERATE, parameters, generate)]
    }

    fn owns_tool(&self, name: &str) -> bool {
        name == TOOL
    }
}

/// The models as clients see them: `image_output` only where the plan the
/// provider's requests go to now allows it (not ChatGPT Free, as Codex has
/// it). A plan not read yet counts as allowing it; a call finds out.
pub(crate) fn listed(inner: &Inner) -> Vec<v1::Model> {
    let mut models = inner.models.clone();
    for model in models.iter_mut().filter(|model| model.image_output) {
        let provider = model.id.split_once('/').map_or(model.id.as_str(), |(provider, _)| provider);
        if on_free_plan(inner, provider) {
            model.image_output = false;
        }
    }
    models
}

/// The provider's account in use (or its only one) is on ChatGPT's Free plan.
fn on_free_plan(inner: &Inner, provider: &str) -> bool {
    let Some(listed) = inner.providers.iter().find(|listed| listed.id == provider) else { return false };
    let account = listed.accounts.iter().find(|account| account.in_use).or_else(|| listed.accounts.first());
    let key = (provider.to_owned(), account.map_or_else(String::new, |account| account.id.clone()));
    inner.plans.get(&key).is_some_and(|plan| plan.eq_ignore_ascii_case("ChatGPT Free"))
}

/// Why the agent's model can't make images now; None when it can.
fn unable(inner: &Inner, agent: &AgentConfig) -> Option<String> {
    let Some(model) = inner.models.iter().find(|model| model.id == agent.model) else {
        return Some("Your model isn't available now, so you can't make images.".to_owned());
    };
    if !model.image_output {
        return Some(format!("Your model, {}, can't make images: only GPT models signed in with a paid ChatGPT plan can.", model.name));
    }
    let provider = model.id.split_once('/').map_or(model.id.as_str(), |(provider, _)| provider);
    if on_free_plan(inner, provider) {
        return Some("ChatGPT's Free plan doesn't include making images, so you can't make them now.".to_owned());
    }
    None
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GenerateArgs {
    prompt: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    quality: Option<String>,
    #[serde(default)]
    transparent: bool,
}

async fn generate(factory: Arc<Factory>, agent: String, args: GenerateArgs) -> Result<ToolResult, String> {
    let prompt = args.prompt.trim().to_owned();
    if prompt.is_empty() {
        return Err("Describe the image to make in prompt.".to_owned());
    }
    let choose = |value: Option<String>, from: &[&str], what: &str| match value.map(|value| value.trim().to_ascii_lowercase()) {
        Some(value) if !from.contains(&value.as_str()) => Err(format!("{what} must be one of {}.", from.join(", "))),
        value => Ok(value),
    };
    let size = choose(args.size, &SIZES, "size")?;
    let quality = choose(args.quality, &QUALITIES, "quality")?;
    let provider = {
        let inner = factory.lock();
        let me = super::caller(&inner.config, &agent)?;
        if let Some(why) = unable(&inner, me) {
            return Err(why);
        }
        me.model.split_once('/').map_or(me.model.as_str(), |(provider, _)| provider).to_owned()
    };
    let request = ImageRequest { prompt: prompt.clone(), size: size.clone(), quality: quality.clone(), transparent: args.transparent };
    let made = factory.models.make_image(&provider, request).await.map_err(|failure| said(&failure))?;
    let (mime, extension) = match made.bytes.as_slice() {
        [0x89, b'P', b'N', b'G', ..] => ("image/png", "png"),
        [0xFF, 0xD8, 0xFF, ..] => ("image/jpeg", "jpg"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => ("image/webp", "webp"),
        _ => return Err("ChatGPT's answer wasn't an image, so nothing was saved.".to_owned()),
    };
    let (width, height) = image::ImageReader::new(Cursor::new(&made.bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_dimensions().ok())
        .ok_or_else(|| "ChatGPT's image can't be read, so nothing was saved.".to_owned())?;
    let path = save(&factory, &agent, extension, &made.bytes).await?;
    let bytes = made.bytes.len();
    let (part, shown) = factory_agent::tools::images::model_part(made.bytes, width, height)
        .await
        .map_err(|error| format!("Saved {}, but it can't be shown to you: {error}.", path.display()))?;
    let kind = extension.to_uppercase();
    let scaled = shown.map(|(w, h)| format!(" You see it scaled down to {w}×{h}.")).unwrap_or_default();
    let text = format!(
        "Made {} ({width}×{height} {kind}) with {}. It's in your agent folder, not a project: copy it where it's wanted.{scaled}",
        path.display(),
        made.model
    );
    let details = json!({
        "path": path.display().to_string(),
        "mime": mime,
        "width": width,
        "height": height,
        "bytes": bytes,
        "model": made.model,
        "size": size.as_deref().unwrap_or("auto"),
        "quality": quality.as_deref().unwrap_or("auto"),
        "prompt": prompt,
    });
    Ok(ToolResult { content: vec![ToolResultPart::Text { text }, part], details, usage: None, terminate: None })
}

/// Saves the image in the agent's folder, `images/<today>/`, under a name
/// no other image there has.
async fn save(factory: &Factory, agent: &str, extension: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let dir = factory.home.agent_dir(agent).join("images").join(crate::uploads::today());
    let failed = |error: std::io::Error| format!("The image was made, but couldn't be saved in {}: {error}.", dir.display());
    tokio::fs::create_dir_all(&dir).await.map_err(failed)?;
    let stamp = crate::transcript::local_time(crate::transcript::now_ms(), "%H%M%S");
    for count in 1..100 {
        let name = if count == 1 { format!("image-{stamp}.{extension}") } else { format!("image-{stamp}-{count}.{extension}") };
        let path = dir.join(name);
        let file = tokio::fs::OpenOptions::new().write(true).create_new(true).open(&path).await;
        let mut file = match file {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(failed(error)),
        };
        tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await.map_err(failed)?;
        tokio::io::AsyncWriteExt::flush(&mut file).await.map_err(failed)?;
        return Ok(path);
    }
    Err(failed(std::io::Error::other("no free name")))
}

/// Why no image was made, for the agent to tell the person. Never sent
/// again by itself.
fn said(failure: &ImageFailure) -> String {
    match failure {
        ImageFailure::NotOffered => "Your sign-in doesn't make images, so no image was made.".to_owned(),
        ImageFailure::SignedOut => "Claktory isn't signed in to ChatGPT now, so no image was made.".to_owned(),
        ImageFailure::Limit { resets_at: Some(at) } => format!(
            "Your ChatGPT plan's image limit is reached until {}. No image was made; don't try again before then.",
            factory_agent::accounts::local_time(*at).unwrap_or_else(|| factory_agent::accounts::when(*at))
        ),
        ImageFailure::Limit { resets_at: None } => {
            "Your ChatGPT plan's image limit is reached for now. No image was made; don't try again right away.".to_owned()
        }
        ImageFailure::NotOnPlan => "Your ChatGPT plan doesn't include making images, so no image was made.".to_owned(),
        ImageFailure::Declined { output: false } => {
            "OpenAI declined to make this image: the description didn't pass its content rules. Change it before you try again."
                .to_owned()
        }
        ImageFailure::Declined { output: true } => {
            "OpenAI declined to make this image: the image it made didn't pass its content rules. Change the description before you try again."
                .to_owned()
        }
        ImageFailure::Unreachable(why) => format!("Couldn't reach ChatGPT to make the image ({why}). No image was made."),
        ImageFailure::Refused(why) => format!("No image was made: {why}."),
    }
}
