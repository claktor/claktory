//! One app: its compiled component and, while it runs, its instance, owned
//! by a task of its own that takes factoryd's requests one at a time and
//! fires the app's timers between them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use factory_protocol::apps::{CallResult, Crashed, Done, Emit, GetSecret, SetStatus, frame::Body};
use tokio::sync::{mpsc, oneshot};
use tokio::time::Instant;
use wasmtime::component::{Component, Linker, ResourceTable};
use wasmtime::{Engine, ResourceLimiter, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

use crate::frames::Outbox;
use crate::kv::Kv;
use crate::net::{Filter, Rule};

wasmtime::component::bindgen!({
    path: "wit",
    world: "app",
    imports: { default: async | trappable },
    exports: { default: async },
});

use claktory::app::types::{Event, LogLevel};

/// How long each of the app's exports may run.
#[derive(Debug, Clone, Copy)]
pub struct Deadlines {
    /// start(), and upgrade() before it.
    pub start: Duration,
    pub call: Duration,
    pub timer: Duration,
    pub stop: Duration,
}

impl Default for Deadlines {
    fn default() -> Deadlines {
        Deadlines {
            start: Duration::from_secs(60),
            call: Duration::from_secs(60),
            timer: Duration::from_secs(60),
            stop: Duration::from_secs(5),
        }
    }
}

/// The most timers an app has at once.
pub const MAX_TIMERS: usize = 64;
/// Memory and storage when Load leaves them at 0.
const DEFAULT_MEMORY_MB: u32 = 64;
const DEFAULT_STORAGE_MB: u32 = 16;

/// An app as factoryd loaded it.
pub struct Spec {
    pub app_id: String,
    pub data_dir: PathBuf,
    pub rules: Arc<Vec<Rule>>,
    pub memory_mb: u32,
    pub storage_mb: u32,
    /// Calls upgrade(from) once, before the next start().
    pub upgrade_from: Option<String>,
    pub deadlines: Deadlines,
}

impl Spec {
    fn memory_mb(&self) -> u32 {
        if self.memory_mb == 0 { DEFAULT_MEMORY_MB } else { self.memory_mb }
    }

    fn storage_mb(&self) -> u32 {
        if self.storage_mb == 0 { DEFAULT_STORAGE_MB } else { self.storage_mb }
    }
}

/// What factoryd asks of a loaded app; each answers with its Frame body.
pub enum Request {
    Start(oneshot::Sender<Body>),
    Stop(oneshot::Sender<Body>),
    Call { tool: String, args: String, caller: String, answer: oneshot::Sender<Body> },
}

/// The store's data: everything the app's imports reach.
pub struct State {
    app_id: String,
    wasi: WasiCtx,
    http: WasiHttpCtx,
    table: ResourceTable,
    filter: Filter,
    limits: Limits,
    kv: Kv,
    timers: HashMap<String, Instant>,
    outbox: Arc<Outbox>,
}

impl WasiView for State {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl WasiHttpView for State {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView { ctx: &mut self.http, table: &mut self.table, hooks: &mut self.filter }
    }
}

/// The app's memory allowance; remembers when it ran out.
struct Limits {
    memory: usize,
    ran_out: bool,
}

impl ResourceLimiter for Limits {
    fn memory_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        let fits = desired <= self.memory;
        self.ran_out |= !fits;
        Ok(fits)
    }

    fn table_growing(&mut self, _current: usize, desired: usize, _maximum: Option<usize>) -> wasmtime::Result<bool> {
        Ok(desired <= 100_000)
    }
}

impl claktory::app::types::Host for State {}

impl claktory::app::host::Host for State {
    async fn secret(&mut self, name: String) -> wasmtime::Result<Option<String>> {
        let ask = Body::GetSecret(GetSecret { app_id: self.app_id.clone(), name });
        Ok(match self.outbox.ask(ask).await {
            Some(Body::SecretValue(secret)) if secret.found => Some(secret.value),
            _ => None,
        })
    }

    async fn kv_get(&mut self, key: String) -> wasmtime::Result<Option<Vec<u8>>> {
        Ok(self.kv.get(&key))
    }

    async fn kv_set(&mut self, key: String, value: Vec<u8>) -> wasmtime::Result<Result<(), String>> {
        Ok(self.kv.set(&key, &value))
    }

    async fn kv_delete(&mut self, key: String) -> wasmtime::Result<()> {
        self.kv.delete(&key);
        Ok(())
    }

    async fn emit(&mut self, event: Event) -> wasmtime::Result<()> {
        self.outbox.tell(Body::Emit(Emit {
            app_id: self.app_id.clone(),
            kind: event.kind,
            scope: event.scope,
            text: event.text,
            data: event.data.unwrap_or_default(),
            dedupe_key: event.dedupe_key.unwrap_or_default(),
        }));
        Ok(())
    }

    async fn set_timer(&mut self, name: String, after_ms: u64) -> wasmtime::Result<()> {
        if !self.timers.contains_key(&name) && self.timers.len() >= MAX_TIMERS {
            tracing::warn!(app = %self.app_id, timer = %name, "timer not set: it has {MAX_TIMERS} already");
            return Ok(());
        }
        // A day at most, so the deadline can't overflow.
        let after = Duration::from_millis(after_ms.min(86_400_000));
        self.timers.insert(name, Instant::now() + after);
        Ok(())
    }

    async fn cancel_timer(&mut self, name: String) -> wasmtime::Result<()> {
        self.timers.remove(&name);
        Ok(())
    }

    async fn set_status(&mut self, text: String) -> wasmtime::Result<()> {
        self.outbox.tell(Body::SetStatus(SetStatus { app_id: self.app_id.clone(), text }));
        Ok(())
    }

    async fn log(&mut self, level: LogLevel, message: String) -> wasmtime::Result<()> {
        let app = &self.app_id;
        match level {
            LogLevel::Error => tracing::error!(app = %app, "{message}"),
            LogLevel::Warn => tracing::warn!(app = %app, "{message}"),
            LogLevel::Info => tracing::info!(app = %app, "{message}"),
            LogLevel::Debug => tracing::debug!(app = %app, "{message}"),
        }
        Ok(())
    }
}

/// The linker every app is instantiated with: WASI 0.2 with nothing
/// granted, wasi:http through each app's filter, and the host interface.
pub fn linker(engine: &Engine) -> wasmtime::Result<Linker<State>> {
    let mut linker = Linker::new(engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    wasmtime_wasi_http::p2::add_only_http_to_linker_async(&mut linker)?;
    App::add_to_linker::<State, wasmtime::component::HasSelf<State>>(&mut linker, |state| state)?;
    Ok(linker)
}

/// A running instance.
struct Running {
    store: Store<State>,
    app: App,
}

/// The task that owns one app.
pub struct Owner {
    spec: Spec,
    pre: AppPre<State>,
    outbox: Arc<Outbox>,
    running: Option<Running>,
}

impl Owner {
    /// Checks that `component` links as a claktory:app@1 app.
    pub fn new(spec: Spec, component: &Component, linker: &Linker<State>, outbox: Arc<Outbox>) -> Result<Owner, String> {
        let pre = linker
            .instantiate_pre(component)
            .and_then(AppPre::new)
            .map_err(|error| format!("The component isn't a claktory:app@1 app: {}", first_line(&error)))?;
        Ok(Owner { spec, pre, outbox, running: None })
    }

    /// Takes requests until the sender is dropped (Unload), then stops the
    /// app if it runs.
    pub async fn run(mut self, mut requests: mpsc::UnboundedReceiver<Request>) {
        loop {
            let next_timer = self.next_timer();
            tokio::select! {
                request = requests.recv() => match request {
                    Some(request) => self.handle(request).await,
                    None => break,
                },
                () = sleep_until(next_timer), if next_timer.is_some() => self.fire_timer().await,
            }
        }
        self.stop().await;
    }

    async fn handle(&mut self, request: Request) {
        match request {
            Request::Start(answer) => {
                let error = self.start().await.err().unwrap_or_default();
                let _ = answer.send(Body::Done(Done { error }));
            }
            Request::Stop(answer) => {
                self.stop().await;
                let _ = answer.send(Body::Done(Done::default()));
            }
            Request::Call { tool, args, caller, answer } => {
                let _ = answer.send(self.call(&tool, &args, &caller).await);
            }
        }
    }

    async fn start(&mut self) -> Result<(), String> {
        if self.running.is_some() {
            return Ok(());
        }
        let mut store = self.store()?;
        let deadlines = self.spec.deadlines;
        let app = match tokio::time::timeout(deadlines.start, self.pre.instantiate_async(&mut store)).await {
            Ok(Ok(app)) => app,
            Ok(Err(error)) => return Err(format!("It didn't start: {}", first_line(&error))),
            Err(_) => return Err(format!("It didn't start within {} seconds.", deadlines.start.as_secs())),
        };
        self.running = Some(Running { store, app });
        if let Some(from) = self.spec.upgrade_from.take() {
            self.guest("upgrade", deadlines.start, async |app, store| app.call_upgrade(store, &from).await).await?;
        }
        self.guest("start", deadlines.start, async |app, store| app.call_start(store).await).await
    }

    /// Calls stop() if it runs, then drops the instance and its timers.
    async fn stop(&mut self) {
        if self.running.is_none() {
            return;
        }
        let stopped = self.guest_quietly("stop", self.spec.deadlines.stop, async |app, store| app.call_stop(store).await).await;
        if let Err(error) = stopped {
            tracing::warn!(app = %self.spec.app_id, %error, "stop() failed; the instance is dropped anyway");
        }
        self.running = None;
    }

    async fn call(&mut self, tool: &str, args: &str, caller: &str) -> Body {
        if self.running.is_none() {
            return Body::Done(Done { error: format!("{} isn't running.", self.spec.app_id) });
        }
        let result = self.guest("call", self.spec.deadlines.call, async |app, store| app.call_call(store, tool, args, caller).await).await;
        Body::CallResult(match result {
            Ok(result) => CallResult { text: result.text, data: result.data.unwrap_or_default(), is_error: result.is_error },
            Err(error) => CallResult { text: error, data: String::new(), is_error: true },
        })
    }

    fn next_timer(&self) -> Option<Instant> {
        self.running.as_ref()?.store.data().timers.values().min().copied()
    }

    async fn fire_timer(&mut self) {
        let Some(running) = self.running.as_mut() else { return };
        let timers = &mut running.store.data_mut().timers;
        let Some(name) = timers.iter().min_by_key(|(_, at)| **at).map(|(name, _)| name.clone()) else { return };
        timers.remove(&name);
        let _ = self.guest("on-timer", self.spec.deadlines.timer, async |app, store| app.call_on_timer(store, &name).await).await;
    }

    /// A new store for an instance, with the app's limits and capabilities.
    fn store(&self) -> Result<Store<State>, String> {
        let spec = &self.spec;
        let kv = Kv::open(&spec.data_dir, u64::from(spec.storage_mb()) << 20)
            .map_err(|error| format!("Its store in {} couldn't be opened: {error}.", spec.data_dir.display()))?;
        let mut wasi = WasiCtx::builder();
        wasi.allow_tcp(false).allow_udp(false).allow_ip_name_lookup(false);
        let state = State {
            app_id: spec.app_id.clone(),
            wasi: wasi.build(),
            http: WasiHttpCtx::new(),
            table: ResourceTable::new(),
            filter: Filter { rules: spec.rules.clone() },
            limits: Limits { memory: (spec.memory_mb() as usize) << 20, ran_out: false },
            kv,
            timers: HashMap::new(),
            outbox: self.outbox.clone(),
        };
        let mut store = Store::new(self.pre.engine(), state);
        store.limiter(|state| &mut state.limits);
        // Yield to tokio every tick, so deadlines can end a busy app.
        store.set_epoch_deadline(1);
        store.epoch_deadline_async_yield_and_update(1);
        Ok(store)
    }

    /// Calls into the running app within `deadline`. A trap, a missed
    /// deadline or running out of memory drops the instance, tells
    /// factoryd it crashed, and says why.
    async fn guest<T>(
        &mut self,
        what: &str,
        deadline: Duration,
        call: impl AsyncFnOnce(&App, &mut Store<State>) -> wasmtime::Result<T>,
    ) -> Result<T, String> {
        let result = self.guest_quietly(what, deadline, call).await;
        if let Err(error) = &result {
            tracing::warn!(app = %self.spec.app_id, %error, "the app crashed");
            self.outbox.tell(Body::Crashed(Crashed { app_id: self.spec.app_id.clone(), error: error.clone() }));
        }
        result
    }

    /// As `guest`, without telling factoryd.
    async fn guest_quietly<T>(
        &mut self,
        what: &str,
        deadline: Duration,
        call: impl AsyncFnOnce(&App, &mut Store<State>) -> wasmtime::Result<T>,
    ) -> Result<T, String> {
        let Some(running) = self.running.as_mut() else { return Err("It isn't running.".to_owned()) };
        let outcome = tokio::time::timeout(deadline, call(&running.app, &mut running.store)).await;
        let error = match outcome {
            Ok(Ok(value)) => return Ok(value),
            Ok(Err(_)) if running.store.data().limits.ran_out => {
                format!("It ran out of memory ({} MB).", self.spec.memory_mb())
            }
            Ok(Err(error)) => format!("It stopped with an error in {what}: {}", first_line(&error)),
            Err(_) => format!("Its {what} took longer than {} seconds.", deadline.as_secs()),
        };
        self.running = None;
        Err(error)
    }
}

async fn sleep_until(at: Option<Instant>) {
    match at {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

/// An error's first line: wasmtime's carry a backtrace after it.
fn first_line(error: &wasmtime::Error) -> String {
    let text = format!("{error:#}");
    let line = text.lines().next().unwrap_or_default().trim();
    let line = line.trim_end_matches('.');
    format!("{line}.")
}
