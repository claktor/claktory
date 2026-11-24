//! Model lists from the providers themselves (friend bug #14): reading one
//! now (RefreshModels), how it went on each Provider, and adding a model the
//! list doesn't have by its id (AddModel, UpdateAgent), checked with the
//! provider and kept across restarts.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use crate::common::{Client, Scripted, View, apply_request, temp_dir};
use factory_agent::auth::AuthInteraction;
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use factoryd::models::Models;
use futures_util::future::BoxFuture;
use tokio::net::TcpListener;

/// The scripted provider, whose list can be read and asked about: it lists
/// "scripted/listed" once read, has every model whose id starts "new-",
/// and nothing else.
struct Listing {
    scripted: Scripted,
    /// The models added by their id.
    added: Mutex<Vec<String>>,
    /// Its list was read.
    read: AtomicBool,
    reads: AtomicUsize,
    /// It can't be reached.
    down: AtomicBool,
}

impl Listing {
    fn new() -> Arc<Listing> {
        Arc::new(Listing {
            scripted: Scripted::signed_in(),
            added: Mutex::default(),
            read: AtomicBool::new(false),
            reads: AtomicUsize::new(0),
            down: AtomicBool::new(false),
        })
    }
}

impl Models for Listing {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async move {
            let mut models = self.scripted.list().await;
            let model = |id: &str, added: bool| v1::Model {
                id: id.into(),
                name: id.into(),
                context_window: 10_000,
                efforts: vec!["off".into()],
                image_input: false,
                added,
                image_output: false,
            };
            if self.read.load(Ordering::Relaxed) {
                models.push(model("scripted/listed", false));
            }
            for id in self.added.lock().expect("lock").iter() {
                models.push(model(&format!("scripted/{id}"), true));
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

    fn fetches_models(&self, provider: &str) -> bool {
        provider == "scripted"
    }

    fn fetch_models<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, String>> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        let down = self.down.load(Ordering::Relaxed);
        Box::pin(async move { if down { Err("Scripted couldn't be reached.".into()) } else { Ok(Some(b"the list".to_vec())) } })
    }

    fn take_models(&self, _provider: &str, body: &[u8]) -> Result<bool, String> {
        assert_eq!(body, b"the list");
        Ok(!self.read.swap(true, Ordering::Relaxed))
    }

    fn check_model<'a>(&'a self, _provider: &'a str, model: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, String>> {
        Box::pin(async move {
            match model.starts_with("new-") {
                true => Ok(Some(serde_json::json!({"id": model}).to_string().into_bytes())),
                false => Err(format!("Scripted has no model “{model}” for this sign-in.")),
            }
        })
    }

    fn add_model(&self, _provider: &str, model: &[u8]) -> Result<bool, String> {
        let model: serde_json::Value = serde_json::from_slice(model).map_err(|error| error.to_string())?;
        let id = model["id"].as_str().ok_or("no id")?.to_owned();
        let mut added = self.added.lock().expect("lock");
        if added.contains(&id) {
            return Ok(false);
        }
        added.push(id);
        Ok(true)
    }
}

/// A Claktory on `models` in `dir`, and a client of it.
async fn start(dir: &Path, models: Arc<Listing>) -> (Client, View, Arc<Factory>) {
    let home = crate::common::home(dir);
    let factory = Factory::open(home.clone(), models).await.expect("open");
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    crate::common::serves(addr, home);
    tokio::spawn(factoryd::serve(listener, factory.clone()));
    let (client, workspace, _) = Client::connect(addr).await;
    (client, View { workspace, ..View::default() }, factory)
}

fn scripted(view: &View) -> v1::Provider {
    view.workspace.providers.iter().find(|provider| provider.id == "scripted").cloned().expect("scripted")
}

fn listed(view: &View, id: &str) -> Option<v1::Model> {
    view.workspace.models.iter().find(|model| model.id == id).cloned()
}

fn refused(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn a_providers_list_is_read_on_request_and_a_model_it_lacks_is_added_once_the_provider_has_it() {
    let dir = temp_dir();
    let models = Listing::new();
    let (mut client, mut view, _factory) = start(&dir, models.clone()).await;
    assert!(scripted(&view).models_fetchable);
    // None kept: read at once, by itself.
    view.follow(&mut client, |view| scripted(view).models_fetched_ms > 0 && listed(view, "scripted/listed").is_some()).await;
    let first = scripted(&view).models_fetched_ms;

    // Refresh models: read again now, and when it was says so.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(matches!(
        apply_request(&mut client, &mut view, Request::RefreshModels(v1::RefreshModels { provider_id: "scripted".into() })).await,
        Body::Ack(_)
    ));
    view.follow(&mut client, |view| scripted(view).models_fetched_ms > first).await;
    assert_eq!(models.reads.load(Ordering::Relaxed), 2);
    let read_at = scripted(&view).models_fetched_ms;
    assert!(!scripted(&view).models_fetching && scripted(&view).models_fetch_error.is_empty());

    // Unreachable: why, on the provider, and the list there was stays.
    models.down.store(true, Ordering::Relaxed);
    let failed = apply_request(&mut client, &mut view, Request::RefreshModels(v1::RefreshModels { provider_id: String::new() })).await;
    assert_eq!(refused(&failed), "Scripted couldn't be reached.");
    view.follow(&mut client, |view| scripted(view).models_fetch_error == "Scripted couldn't be reached.").await;
    assert_eq!(scripted(&view).models_fetched_ms, read_at);
    assert!(listed(&view, "scripted/listed").is_some());
    let unknown = apply_request(&mut client, &mut view, Request::RefreshModels(v1::RefreshModels { provider_id: "nowhere".into() })).await;
    assert_eq!(refused(&unknown), "There's no provider “nowhere”.");

    // A model the list lacks: refused when the provider hasn't it, added
    // when it has.
    let add = |model: &str| Request::AddModel(v1::AddModel { model: model.into() });
    assert_eq!(
        refused(&apply_request(&mut client, &mut view, add("scripted/old-one")).await),
        "Scripted has no model “old-one” for this sign-in."
    );
    assert!(listed(&view, "scripted/old-one").is_none());
    assert!(matches!(apply_request(&mut client, &mut view, add("scripted/new-one")).await, Body::Ack(_)));
    view.follow(&mut client, |view| listed(view, "scripted/new-one").is_some_and(|model| model.added)).await;
    assert!(matches!(apply_request(&mut client, &mut view, add("scripted/new-one")).await, Body::Ack(_)), "there already");
    assert_eq!(refused(&apply_request(&mut client, &mut view, add("elsewhere/x")).await), "There's no provider “elsewhere”.");
    assert!(
        refused(&apply_request(&mut client, &mut view, add("no-provider")).await).starts_with("Give the model as <provider>/<model id>")
    );

    // An agent's model set to one the list lacks is checked the same way.
    let update =
        |model: &str| Request::UpdateAgent(v1::UpdateAgent { agent_id: "rivet".into(), model: Some(model.into()), ..Default::default() });
    assert_eq!(
        refused(&apply_request(&mut client, &mut view, update("scripted/old-two")).await),
        "Scripted has no model “old-two” for this sign-in."
    );
    assert_ne!(view.agent("rivet").model, "scripted/old-two");
    assert!(matches!(apply_request(&mut client, &mut view, update("scripted/new-two")).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.agent("rivet").model == "scripted/new-two" && listed(view, "scripted/new-two").is_some()).await;
    drop(client);
    drop(_factory);

    // Kept: a new Claktory on the same data lists them from the start.
    let again = Listing::new();
    let (_client, view, _factory) = start(&dir, again.clone()).await;
    assert!(
        listed(&view, "scripted/new-one").is_some_and(|model| model.added) && listed(&view, "scripted/new-two").is_some(),
        "{:?}",
        view.workspace.models
    );
}
