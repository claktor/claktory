//! Newer models (#404): when a provider's list brings a newer version of a
//! model an agent runs on, Rivet hears of it once, with the agents on the
//! older one, across list reads and restarts; a later version is news
//! again. One already out when Claktory first saw the list isn't news.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, temp_dir};
use factory_agent::auth::AuthInteraction;
use factory_agent::{Error, Model, StreamFn};
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use factoryd::factory::Factory;
use factoryd::models::Models;
use futures_util::future::BoxFuture;

/// The scripted provider, whose list has "scripted/echo-1" and, once read,
/// every version up to `newest`.
struct Versions {
    scripted: Scripted,
    /// The newest version its list has once read.
    newest: AtomicU64,
    /// What its list had when it was last read (or kept).
    read: AtomicU64,
    /// More models it lists, by id.
    more: std::sync::Mutex<Vec<String>>,
}

impl Versions {
    fn new(newest: u64) -> Arc<Versions> {
        Versions::listing(1, newest)
    }

    /// Listing up to `read` from the start.
    fn listing(read: u64, newest: u64) -> Arc<Versions> {
        Arc::new(Versions {
            scripted: Scripted::signed_in(),
            newest: AtomicU64::new(newest),
            read: AtomicU64::new(read),
            more: std::sync::Mutex::default(),
        })
    }
}

impl Models for Versions {
    fn list(&self) -> BoxFuture<'_, Vec<v1::Model>> {
        Box::pin(async move {
            let mut models = self.scripted.list().await;
            for version in 1..=self.read.load(Ordering::Relaxed) {
                models.push(v1::Model {
                    id: format!("scripted/echo-{version}"),
                    name: format!("Echo {version}"),
                    context_window: 10_000,
                    efforts: vec!["off".into()],
                    ..Default::default()
                });
            }
            for id in self.more.lock().expect("lock").iter() {
                models.push(v1::Model { id: id.clone(), name: id.clone(), efforts: vec!["off".into()], ..Default::default() });
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

    fn fetch_models<'a>(&'a self, _provider: &'a str) -> BoxFuture<'a, Result<Option<Vec<u8>>, factory_agent::Error>> {
        let newest = self.newest.load(Ordering::Relaxed);
        Box::pin(async move { Ok(Some(newest.to_string().into_bytes())) })
    }

    fn take_models(&self, _provider: &str, body: &[u8]) -> Result<bool, String> {
        let newest: u64 = String::from_utf8_lossy(body).parse().map_err(|_| "not a list".to_owned())?;
        Ok(self.read.swap(newest, Ordering::Relaxed) != newest)
    }
}

/// A Claktory on `models` in `dir`, and a client of it.
async fn start(dir: &Path, models: Arc<Versions>) -> (Client, View, Arc<Factory>) {
    let (addr, factory) = crate::common::start_with_models(dir, models).await;
    let (client, workspace, _) = Client::connect(addr).await;
    (client, View { workspace, ..View::default() }, factory)
}

/// The Claktory events Rivet's conversation has, as their full text.
fn rivets_news(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::Event(event)) if event.text.contains("has a newer version") => Some(event.text.clone()),
            _ => None,
        })
        .collect()
}

async fn read_list(client: &mut Client, view: &mut View) {
    let read = apply_request(client, view, Request::RefreshModels(v1::RefreshModels { provider_id: "scripted".into() })).await;
    assert!(matches!(read, Body::Ack(_)), "{read:?}");
}

/// Rivet on a model that can run, Mike on Echo 1, Rivet's conversation
/// open.
async fn mike_on_echo_1(client: &mut Client, view: &mut View) {
    let model = |agent: &str, model: &str| {
        Request::UpdateAgent(v1::UpdateAgent {
            agent_id: agent.into(),
            model: Some(model.into()),
            effort: Some("off".into()),
            ..Default::default()
        })
    };
    apply_request(client, view, model("rivet", MODEL)).await;
    apply_request(client, view, create_agent("", "Mike", "Builds things.")).await;
    assert!(matches!(apply_request(client, view, model("mike", "scripted/echo-1")).await, Body::Ack(_)));
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
}

#[tokio::test]
async fn rivet_hears_once_of_a_newer_version_of_a_model_agents_run_on() {
    let dir = temp_dir();
    let models = Versions::new(1);
    let (mut client, mut view, factory) = start(&dir, models.clone()).await;
    mike_on_echo_1(&mut client, &mut view).await;
    let mike = "mike";

    // Nothing newer: nothing said.
    read_list(&mut client, &mut view).await;
    // Echo 2 comes out.
    models.newest.store(2, Ordering::Relaxed);
    read_list(&mut client, &mut view).await;
    view.follow(&mut client, |view| !rivets_news(view).is_empty()).await;
    let news = rivets_news(&view);
    assert_eq!(news.len(), 1, "{news:?}");
    assert!(
        news[0].starts_with(&format!(
            "Scripted has a newer version of a model your agents run on: Echo 2 (scripted/echo-2).\n\n\
             These agents run on an older version:\n- Mike ({mike}), on Echo 1 (scripted/echo-1)\n\n\
             Ask the person before anything changes"
        )),
        "{}",
        news[0]
    );
    assert!(news[0].contains("move each one with models_set, giving only the model (scripted/echo-2)"), "Rivet can change models");

    // Read again: not said again.
    models.newest.store(2, Ordering::Relaxed);
    read_list(&mut client, &mut view).await;
    drop(client);
    drop(factory);

    // A restart doesn't say it again, whatever the person answered.
    let again = Versions::new(2);
    let (mut client, mut view, _factory) = start(&dir, again.clone()).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    read_list(&mut client, &mut view).await;
    assert_eq!(rivets_news(&view).len(), 1, "{:?}", rivets_news(&view));

    // A later one is news.
    again.newest.store(3, Ordering::Relaxed);
    read_list(&mut client, &mut view).await;
    view.follow(&mut client, |view| rivets_news(view).len() == 2).await;
    assert!(rivets_news(&view)[1].contains(": Echo 3 (scripted/echo-3)"));
}

#[tokio::test]
async fn a_newer_model_already_out_when_claktory_first_saw_the_list_isnt_news() {
    let dir = temp_dir();
    // Echo 2 is out already: Mike was put on Echo 1 by choice.
    let models = Versions::listing(2, 2);
    let (mut client, mut view, _factory) = start(&dir, models.clone()).await;
    mike_on_echo_1(&mut client, &mut view).await;
    read_list(&mut client, &mut view).await;
    assert!(rivets_news(&view).is_empty(), "{:?}", rivets_news(&view));
    // Echo 3 comes out: news.
    models.newest.store(3, Ordering::Relaxed);
    read_list(&mut client, &mut view).await;
    view.follow(&mut client, |view| !rivets_news(view).is_empty()).await;
    assert!(rivets_news(&view)[0].contains(": Echo 3 (scripted/echo-3)"), "{:?}", rivets_news(&view));
}

#[tokio::test]
async fn this_versions_first_start_tells_rivet_once_of_every_newer_model_already_out() {
    let dir = temp_dir();
    let models = Versions::listing(1, 1);
    let (mut client, mut view, factory) = start(&dir, models.clone()).await;
    mike_on_echo_1(&mut client, &mut view).await;
    apply_request(&mut client, &mut view, create_agent("", "Ana", "Writes.")).await;
    let talk =
        Request::UpdateAgent(v1::UpdateAgent { agent_id: "ana".into(), model: Some("scripted/talk-1".into()), ..Default::default() });
    models.more.lock().expect("lock").extend(["scripted/talk-1".to_owned()]);
    read_list(&mut client, &mut view).await;
    assert!(matches!(apply_request(&mut client, &mut view, talk).await, Body::Ack(_)));
    drop(client);
    drop(factory);

    // As if this version starts for the first time on a Claktory whose
    // agents have worked: its first look at the models is to come.
    {
        let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
        db.execute_batch("DELETE FROM models_seen; DELETE FROM models_catch_up; INSERT INTO models_catch_up (due) VALUES (1);")
            .expect("reset");
    }
    let again = Versions::listing(2, 2);
    again.more.lock().expect("lock").extend(["scripted/talk-1".to_owned(), "scripted/talk-2".to_owned()]);
    let (mut client, mut view, factory) = start(&dir, again.clone()).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    view.follow(&mut client, |view| {
        view.entries
            .iter()
            .any(|entry| matches!(&entry.kind, Some(v1::entry::Kind::Event(event)) if event.text.starts_with("Newer versions")))
    })
    .await;
    let told: Vec<String> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(v1::entry::Kind::Event(event)) if event.text.starts_with("Newer versions") => Some(event.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(told.len(), 1, "one event for them all: {told:?}");
    assert!(
        told[0].contains("Echo 2 (scripted/echo-2), from Scripted. These agents run on an older version:\n- Mike (mike), on Echo 1 (scripted/echo-1)")
            && told[0].contains("scripted/talk-2 (scripted/talk-2), from Scripted. These agents run on an older version:\n- Ana (ana), on scripted/talk-1 (scripted/talk-1)")
            && told[0].contains("in one ask_person request for them all"),
        "{}",
        told[0]
    );
    assert!(rivets_news(&view).is_empty(), "not one per model as well");
    drop(client);
    drop(factory);

    // Once: the next start says nothing, and a later model is news as usual.
    let third = Versions::listing(2, 3);
    third.more.lock().expect("lock").extend(["scripted/talk-1".to_owned(), "scripted/talk-2".to_owned()]);
    let (mut client, mut view, _factory) = start(&dir, third.clone()).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
    read_list(&mut client, &mut view).await;
    view.follow(&mut client, |view| !rivets_news(view).is_empty()).await;
    assert_eq!(rivets_news(&view).len(), 1);
    assert!(rivets_news(&view)[0].contains(": Echo 3 (scripted/echo-3)"));
    let caught_up = view
        .entries
        .iter()
        .filter(|entry| matches!(&entry.kind, Some(v1::entry::Kind::Event(event)) if event.text.starts_with("Newer versions")))
        .count();
    assert_eq!(caught_up, 1, "not again");
}

#[tokio::test]
async fn a_new_claktory_has_nothing_to_catch_up_on() {
    let dir = temp_dir();
    let (_client, _view, _factory) = start(&dir, Versions::new(1)).await;
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let due: i64 = db.query_row("SELECT COUNT(*) FROM models_catch_up", [], |row| row.get(0)).expect("count");
    assert_eq!(due, 0);
}
