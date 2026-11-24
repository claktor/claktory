//! The Images extension (#403): an agent whose model can make images makes
//! them with image_generate. The image is kept in its agent folder, the
//! call's details are what the app draws it from, and the tool is in the
//! frame only of agents whose model can make images on its sign-in.

use std::io::Cursor;
use std::sync::{Arc, Mutex};

use factory_agent::auth::AuthInteraction;
use factory_agent::images::{ImageFailure, ImageRequest, MadeImage};
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1::{self, AgentStatus, client_message, entry::Kind};
use factoryd::models::Models;
use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use crate::common::{
    Client, MODEL, OTHER_MODEL, Scripted, View, apply_request, create_project, home, send_message, start_with_models, temp_dir,
};

/// Scripted models, where Parrot (which reads images) makes them too, on a
/// plan the test sets.
pub(crate) struct Painter {
    scripted: Scripted,
    /// What the next image requests get; an image when None.
    fails: Mutex<Option<ImageFailure>>,
    /// The image requests, oldest first.
    asked: Mutex<Vec<ImageRequest>>,
    plan: Mutex<String>,
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([20, 120, 220, 255]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).expect("png");
    out.into_inner()
}

impl Models for Painter {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async move {
            let mut models = self.scripted.list().await;
            for model in &mut models {
                model.image_input = model.id == OTHER_MODEL;
                model.image_output = model.id == OTHER_MODEL;
            }
            models
        })
    }

    fn providers(&self) -> BoxFuture<'_, Vec<v1::Provider>> {
        self.scripted.providers()
    }

    fn resolve(&self, id: &str) -> Option<Model> {
        self.scripted.resolve(id)
    }

    fn stream(&self) -> StreamFn {
        self.scripted.stream()
    }

    fn login<'a>(
        &'a self,
        provider: &'a str,
        method: &'a str,
        interaction: &'a dyn AuthInteraction,
        add: bool,
    ) -> BoxFuture<'a, Result<(), Error>> {
        self.scripted.login(provider, method, interaction, add)
    }

    fn logout<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<(), Error>> {
        self.scripted.logout(provider, account)
    }

    fn usage<'a>(&'a self, provider: &'a str, account: &'a str) -> BoxFuture<'a, Result<Option<v1::ProviderUsage>, Error>> {
        self.scripted.usage(provider, account)
    }

    fn plan<'a>(&'a self, _provider: &'a str, _account: &'a str) -> BoxFuture<'a, Result<String, Error>> {
        let plan = self.plan.lock().expect("lock").clone();
        Box::pin(async move { Ok(plan) })
    }

    fn make_image<'a>(&'a self, provider: &'a str, request: ImageRequest) -> BoxFuture<'a, Result<MadeImage, ImageFailure>> {
        assert_eq!(provider, "scripted");
        self.asked.lock().expect("lock").push(request);
        let fails = self.fails.lock().expect("lock").clone();
        Box::pin(async move {
            match fails {
                Some(failure) => Err(failure),
                None => Ok(MadeImage { bytes: png(48, 32), model: "gpt-image-2".into() }),
            }
        })
    }
}

pub(crate) fn painter(plan: &str) -> Arc<Painter> {
    Arc::new(Painter {
        scripted: Scripted::subscribed(),
        fails: Mutex::new(None),
        asked: Mutex::new(Vec::new()),
        plan: Mutex::new(plan.to_owned()),
    })
}

pub(crate) fn create(name: &str, model: &str) -> client_message::Body {
    client_message::Body::CreateAgent(v1::CreateAgent {
        project_id: "site".into(),
        name: name.into(),
        model: model.into(),
        effort: "off".into(),
        ..Default::default()
    })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// Has `agent` call image_generate with `args`; its call.
pub(crate) async fn call(client: &mut Client, view: &mut View, agent: &str, args: Value) -> v1::ToolCall {
    let select = client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() });
    apply_request(client, view, select).await;
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, &format!("call image_generate {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    view.entries[before..]
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::ToolCall(call)) => Some(call.clone()),
            _ => None,
        })
        .expect("a tool call")
}

pub(crate) async fn setup(models: Arc<Painter>) -> (std::path::PathBuf, Client, View) {
    let dir = temp_dir();
    let (addr, _factory) = start_with_models(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    (dir, client, view)
}

#[tokio::test]
async fn an_agent_whose_model_makes_images_makes_one_into_its_own_folder() {
    let models = painter("ChatGPT Plus");
    let (dir, mut client, mut view) = setup(models.clone()).await;
    apply_request(&mut client, &mut view, create("Painter", OTHER_MODEL)).await;
    apply_request(&mut client, &mut view, create("Writer", MODEL)).await;
    view.follow(&mut client, |view| view.workspace.models.iter().any(|model| model.id == OTHER_MODEL && model.image_output)).await;
    assert!(view.workspace.models.iter().any(|model| model.id == MODEL && !model.image_output));

    let args = json!({"prompt": "  a lighthouse at dusk, flat colors  ", "size": "1536x1024", "quality": "low", "transparent": true});
    let made = call(&mut client, &mut view, "painter", args).await;
    assert_eq!(made.state(), v1::ToolCallState::Done, "{}", made.output);
    let details: Value = serde_json::from_str(&made.details).expect("details are JSON");
    let path = details["path"].as_str().expect("a path").to_owned();
    // The details the app draws the call from, exactly these keys (#403 §3).
    assert_eq!(
        details,
        json!({
            "path": path,
            "mime": "image/png",
            "width": 48,
            "height": 32,
            "bytes": png(48, 32).len(),
            "model": "gpt-image-2",
            "size": "1536x1024",
            "quality": "low",
            "prompt": "a lighthouse at dusk, flat colors",
        })
    );
    let images = home(&dir).agent_dir("painter").join("images");
    assert!(std::path::Path::new(&path).starts_with(&images), "{path} is in the agent's images");
    assert!(path.ends_with(".png"), "{path}");
    assert_eq!(std::fs::read(&path).expect("the image"), png(48, 32));
    assert!(made.output.contains(&path), "{}", made.output);
    assert_eq!(
        models.asked.lock().expect("lock").clone(),
        [ImageRequest {
            prompt: "a lighthouse at dusk, flat colors".into(),
            size: Some("1536x1024".into()),
            quality: Some("low".into()),
            transparent: true,
        }]
    );
    // Asked for nothing in particular: the provider's choice, said as "auto".
    let again = call(&mut client, &mut view, "painter", json!({"prompt": "a lighthouse"})).await;
    let details: Value = serde_json::from_str(&again.details).expect("details are JSON");
    assert_eq!((details["size"].as_str(), details["quality"].as_str()), (Some("auto"), Some("auto")));
    assert_ne!(details["path"].as_str(), Some(path.as_str()), "a new file for each image");

    // Only the agent whose model makes images has the tool.
    let tools_of = |model: &str| {
        let requests = models.scripted.requests.lock().expect("lock");
        requests.iter().rev().find(|request| request.model == model).map(|request| request.tools.clone()).expect("a request")
    };
    assert!(tools_of("parrot").iter().any(|tool| tool == "image_generate"));
    apply_request(&mut client, &mut view, send_message("writer", "hello")).await;
    view.follow(&mut client, |view| idle(view, "writer") && view.agent("writer").context_tokens > 0).await;
    assert!(!tools_of("echo").iter().any(|tool| tool == "image_generate"));
}

#[tokio::test]
async fn a_refusal_ends_the_call_with_what_it_means_and_saves_nothing() {
    let models = painter("ChatGPT Plus");
    let (dir, mut client, mut view) = setup(models.clone()).await;
    apply_request(&mut client, &mut view, create("Painter", OTHER_MODEL)).await;
    *models.fails.lock().expect("lock") = Some(ImageFailure::Declined { output: false });

    let declined = call(&mut client, &mut view, "painter", json!({"prompt": "something it won't draw"})).await;
    assert_eq!(declined.state(), v1::ToolCallState::Failed);
    assert_eq!(
        declined.output,
        "OpenAI declined to make this image: the description didn't pass its content rules. Change it before you try again."
    );
    assert!(declined.details.is_empty() || declined.details == "{}", "{}", declined.details);
    assert_eq!(models.asked.lock().expect("lock").len(), 1, "never sent again by itself");
    assert!(!home(&dir).agent_dir("painter").join("images").exists(), "nothing saved");

    *models.fails.lock().expect("lock") = Some(ImageFailure::Limit { resets_at: None });
    let limited = call(&mut client, &mut view, "painter", json!({"prompt": "a lighthouse"})).await;
    assert_eq!(limited.state(), v1::ToolCallState::Failed);
    assert!(limited.output.starts_with("Your ChatGPT plan's image limit is reached"), "{}", limited.output);

    let wrong = call(&mut client, &mut view, "painter", json!({"prompt": "a lighthouse", "size": "640x480"})).await;
    assert_eq!(wrong.state(), v1::ToolCallState::Failed);
    assert_eq!(models.asked.lock().expect("lock").len(), 2, "a size it can't make isn't asked for");
}

#[tokio::test]
async fn on_chatgpts_free_plan_no_model_makes_images() {
    let models = painter("ChatGPT Free");
    let (_dir, mut client, mut view) = setup(models.clone()).await;
    apply_request(&mut client, &mut view, create("Painter", OTHER_MODEL)).await;
    // Listed as making images until the plan is read, then not.
    view.follow(&mut client, |view| {
        view.workspace.providers.iter().any(|provider| provider.plan == "ChatGPT Free")
            && view.workspace.models.iter().any(|model| model.id == OTHER_MODEL && !model.image_output)
    })
    .await;
    let refused = call(&mut client, &mut view, "painter", json!({"prompt": "a lighthouse"})).await;
    assert_eq!(refused.state(), v1::ToolCallState::Failed);
    assert!(models.asked.lock().expect("lock").is_empty());
    let requests = models.scripted.requests.lock().expect("lock").clone();
    assert!(!requests.iter().any(|request| request.tools.iter().any(|tool| tool == "image_generate")), "not in its frame");
}
