//! The host: takes factoryd's frames, keeps the loaded apps, and answers.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use factory_protocol::apps::{Call, Done, Frame, Load, Ready, ViewAction, ViewClosed, ViewOpen, frame::Body};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine};

use crate::app::{Deadlines, Owner, Request, Spec, State};
use crate::frames::{self, Outbox};
use crate::net::Rule;

/// The WIT worlds this host runs: 1.0 apps, and 1.1 apps (views).
pub const WORLDS: [&str; 2] = ["claktory:app@1", "claktory:app@1.1"];

/// How often the epoch ticks: how soon a busy app yields.
const TICK: Duration = Duration::from_millis(10);

/// A loaded app: its owner task and the way to it.
struct Loaded {
    requests: mpsc::UnboundedSender<Request>,
    task: JoinHandle<()>,
}

pub struct Host {
    engine: Engine,
    linker: Linker<State>,
    outbox: Arc<Outbox>,
    apps: Mutex<HashMap<String, Loaded>>,
    deadlines: Deadlines,
}

/// The engine apps run on: async, with epoch interruption.
pub fn engine() -> wasmtime::Result<Engine> {
    let mut config = Config::new();
    config.epoch_interruption(true);
    let engine = Engine::new(&config)?;
    let ticking = engine.weak();
    std::thread::Builder::new().name("epoch".into()).spawn(move || {
        while let Some(engine) = ticking.upgrade() {
            engine.increment_epoch();
            drop(engine);
            std::thread::sleep(TICK);
        }
    })?;
    Ok(engine)
}

/// Serves factoryd on `input` and `output` until `input` ends, then stops
/// every app.
pub async fn serve(input: impl AsyncRead + Unpin, output: impl AsyncWrite + Unpin + Send + 'static) -> std::io::Result<()> {
    serve_with(input, output, Deadlines::default()).await
}

/// As `serve`, with the apps' exports held to `deadlines` (shorter ones
/// for tests).
pub async fn serve_with(
    mut input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin + Send + 'static,
    deadlines: Deadlines,
) -> std::io::Result<()> {
    let engine = engine().map_err(std::io::Error::other)?;
    let linker = crate::app::linker(&engine).map_err(std::io::Error::other)?;
    let (frames, mut outgoing) = mpsc::unbounded_channel::<Frame>();
    let writer = tokio::spawn(async move {
        while let Some(frame) = outgoing.recv().await {
            if let Err(error) = frames::write(&mut output, &frame).await {
                tracing::error!(%error, "could not write to factoryd");
                break;
            }
        }
    });
    let host = Arc::new(Host { engine, linker, outbox: Arc::new(Outbox::new(frames)), apps: Mutex::new(HashMap::new()), deadlines });
    let ended = loop {
        let frame = match frames::read(&mut input).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break Ok(()),
            Err(error) => break Err(error),
        };
        if frame.reply_to != 0 {
            host.outbox.answered(frame);
            continue;
        }
        host.dispatch(frame);
    };
    host.outbox.close();
    host.unload_all().await;
    drop(host);
    let _ = writer.await;
    ended
}

impl Host {
    /// Starts on factoryd's request. Start, Stop and Call reach the app's
    /// owner in the order they came; the answers go back as each is done.
    fn dispatch(self: &Arc<Self>, frame: Frame) {
        let id = frame.id;
        let Some(body) = frame.body else { return };
        let answer = move |host: Arc<Host>, answer: Body| {
            if id != 0 {
                host.outbox.answer(id, answer);
            }
        };
        let queued = match body {
            Body::Start(start) => self.enqueue(&start.app_id, Request::Start),
            Body::Stop(stop) => self.enqueue(&stop.app_id, Request::Stop),
            Body::Call(Call { app_id, tool, args, caller }) => self.enqueue(&app_id, |answer| Request::Call { tool, args, caller, answer }),
            Body::ViewOpen(ViewOpen { app_id, view, params, surface }) => {
                self.enqueue(&app_id, |answer| Request::ViewOpen { view, params, surface, answer })
            }
            Body::ViewAction(ViewAction { app_id, surface, message }) => {
                self.enqueue(&app_id, |answer| Request::ViewAction { surface, message, answer })
            }
            Body::ViewClosed(ViewClosed { app_id, surface }) => self.enqueue(&app_id, |answer| Request::ViewClosed { surface, answer }),
            Body::Hello(hello) => {
                tracing::info!(factoryd = %hello.version, "factoryd said hello");
                let ready = Ready { version: env!("CARGO_PKG_VERSION").into(), worlds: WORLDS.map(String::from).to_vec() };
                return answer(self.clone(), Body::Ready(ready));
            }
            Body::Load(load) => {
                let host = self.clone();
                tokio::spawn(async move { answer(host.clone(), done(host.load(load).await)) });
                return;
            }
            Body::Unload(unload) => {
                let host = self.clone();
                tokio::spawn(async move {
                    host.unload(&unload.app_id).await;
                    answer(host, done(Ok(())));
                });
                return;
            }
            other => {
                tracing::warn!(?other, "not a request the host takes");
                return answer(self.clone(), done(Err("The host doesn't take that request.".into())));
            }
        };
        let host = self.clone();
        tokio::spawn(async move {
            let body = match queued {
                Ok(answered) => answered.await.unwrap_or_else(|_| done(Err("It was unloaded.".into()))),
                Err(refused) => done(Err(refused)),
            };
            answer(host, body);
        });
    }

    /// Compiles the app and keeps it ready, replacing (and stopping) one
    /// loaded before.
    async fn load(&self, load: Load) -> Result<(), String> {
        if !WORLDS.contains(&load.world.as_str()) && !load.world.is_empty() {
            return Err(format!("This Claktory can't run apps built for {}.", load.world));
        }
        let mut rules = Vec::new();
        for host in &load.hosts {
            rules.push(Rule::parse(host).ok_or_else(|| format!("\"{host}\" isn't a host name."))?);
        }
        let engine = self.engine.clone();
        let path = PathBuf::from(&load.component);
        let component = tokio::task::spawn_blocking(move || Component::from_file(&engine, &path))
            .await
            .map_err(|error| format!("Compiling it failed: {error}."))?
            .map_err(|error| format!("{} isn't a component this Claktory can run: {error}.", load.component))?;
        let spec = Spec {
            app_id: load.app_id.clone(),
            data_dir: PathBuf::from(&load.data_dir),
            rules: Arc::new(rules),
            memory_mb: load.memory_mb,
            storage_mb: load.storage_mb,
            upgrade_from: Some(load.upgrade_from.clone()).filter(|from| !from.is_empty()),
            deadlines: self.deadlines,
        };
        let owner = Owner::new(spec, &component, &self.linker, self.outbox.clone())?;
        self.unload(&load.app_id).await;
        let (requests, receiver) = mpsc::unbounded_channel();
        let task = tokio::spawn(owner.run(receiver));
        self.apps.lock().unwrap_or_else(PoisonError::into_inner).insert(load.app_id.clone(), Loaded { requests, task });
        tracing::info!(app = %load.app_id, version = %load.version, "loaded");
        Ok(())
    }

    /// Hands a request to the app's owner, in order: its answer comes on
    /// the receiver.
    fn enqueue(&self, app_id: &str, request: impl FnOnce(oneshot::Sender<Body>) -> Request) -> Result<oneshot::Receiver<Body>, String> {
        let apps = self.apps.lock().unwrap_or_else(PoisonError::into_inner);
        let not_loaded = || format!("{app_id} isn't loaded.");
        let app = apps.get(app_id).ok_or_else(not_loaded)?;
        let (answer, answered) = oneshot::channel();
        app.requests.send(request(answer)).map_err(|_| not_loaded())?;
        Ok(answered)
    }

    /// Stops the app if it runs and forgets it.
    async fn unload(&self, app_id: &str) {
        let loaded = self.apps.lock().unwrap_or_else(PoisonError::into_inner).remove(app_id);
        if let Some(Loaded { requests, task }) = loaded {
            drop(requests);
            let _ = task.await;
            tracing::info!(app = %app_id, "unloaded");
        }
    }

    async fn unload_all(&self) {
        let ids: Vec<String> = self.apps.lock().unwrap_or_else(PoisonError::into_inner).keys().cloned().collect();
        for id in ids {
            self.unload(&id).await;
        }
    }
}

fn done(result: Result<(), String>) -> Body {
    Body::Done(Done { error: result.err().unwrap_or_default() })
}
