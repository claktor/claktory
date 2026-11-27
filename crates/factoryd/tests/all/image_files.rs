//! The images a conversation lets go of, at /clear or compaction,
//! have their files deleted at the provider (Anthropic keeps each image an
//! agent read as a file, so later requests don't send it again).

use std::sync::Arc;

use crate::common::{Client, OTHER_MODEL, Scripted, View, apply_request, create_agent, create_project, start_with_models, temp_dir};
use crate::pipes::call;
use factory_agent::auth::AuthInteraction;
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1::{self, client_message::Body as Request};
use factoryd::models::Models;
use futures_util::future::BoxFuture;
use std::sync::Mutex;

/// Scripted models that note which images they were told to forget.
struct Forgetting {
    scripted: Scripted,
    forgotten: Arc<Mutex<Vec<String>>>,
}

impl Models for Forgetting {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        self.scripted.list()
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

    fn forget_images(&self, digests: Vec<String>) -> BoxFuture<'_, ()> {
        self.forgotten.lock().expect("forgotten").extend(digests);
        Box::pin(async {})
    }
}

/// A 1×1 PNG.
fn png() -> Vec<u8> {
    use base64::Engine as _;
    let png = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";
    base64::engine::general_purpose::STANDARD.decode(png).expect("png")
}

#[tokio::test]
async fn a_cleared_conversation_lets_go_of_the_files_of_the_images_it_read() {
    let dir = temp_dir();
    let forgotten = Arc::new(Mutex::new(Vec::new()));
    let models = Arc::new(Forgetting { scripted: Scripted::signed_in(), forgotten: forgotten.clone() });
    let (addr, factory) = start_with_models(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lea", "")).await;
    let parrot = v1::UpdateAgent { agent_id: "lea".into(), model: Some(OTHER_MODEL.into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(parrot)).await;
    let bytes = png();
    std::fs::write(dir.join("site").join("shot.png"), &bytes).expect("write");
    let read = call(&mut client, &mut view, "lea", "read_image", r#"{"path": "shot.png"}"#).await;
    assert!(read.contains("shot.png"), "{read}");
    assert!(forgotten.lock().expect("forgotten").is_empty(), "nothing let go of yet");

    factory.run_command("lea", "clear", "").await.expect("clear");
    let digest: String = {
        use sha2::Digest as _;
        sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !forgotten.lock().expect("forgotten").contains(&digest) {
        assert!(std::time::Instant::now() < deadline, "the image's file never let go of: {:?}", forgotten.lock().expect("forgotten"));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}

#[tokio::test]
async fn a_compaction_lets_go_of_the_files_of_the_images_it_summarized() {
    let dir = temp_dir();
    let forgotten = Arc::new(Mutex::new(Vec::new()));
    let models = Arc::new(Forgetting { scripted: Scripted::signed_in(), forgotten: forgotten.clone() });
    let (addr, factory) = start_with_models(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Lea", "")).await;
    let parrot = v1::UpdateAgent { agent_id: "lea".into(), model: Some(OTHER_MODEL.into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(parrot)).await;
    let bytes = png();
    std::fs::write(dir.join("site").join("shot.png"), &bytes).expect("write");
    call(&mut client, &mut view, "lea", "read_image", r#"{"path": "shot.png"}"#).await;
    factory.run_command("lea", "compact", "").await.expect("compact");
    let digest: String = {
        use sha2::Digest as _;
        sha2::Sha256::digest(&bytes).iter().map(|byte| format!("{byte:02x}")).collect()
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !forgotten.lock().expect("forgotten").contains(&digest) {
        assert!(std::time::Instant::now() < deadline, "the image's file never let go of: {:?}", forgotten.lock().expect("forgotten"));
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
}
