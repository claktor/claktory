//! The link to claktory-apps, the host that runs apps: frames both ways
//! over its stdin and stdout (`proto/factory/apps/v1/host.proto`), requests
//! matched to their answers by id.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use factory_protocol::apps::{Frame, frame::Body};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

/// The most a frame may weigh.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// A running host's two ends, and its process when it's one.
pub struct AppsLink {
    pub reader: Box<dyn AsyncRead + Send + Unpin>,
    pub writer: Box<dyn AsyncWrite + Send + Unpin>,
    pub child: Option<tokio::process::Child>,
}

/// Starts a host; how is the launcher's (a process, or a stand-in in tests).
pub type Launcher = Arc<dyn Fn() -> std::io::Result<AppsLink> + Send + Sync>;

/// Where the host's program is: `FACTORY_APPS_HOST`, or `claktory-apps`
/// beside factoryd's own.
pub(crate) fn program() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FACTORY_APPS_HOST").filter(|path| !path.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let beside = std::env::current_exe().ok()?.parent()?.join("claktory-apps");
    beside.is_file().then_some(beside)
}

/// Starts the host's program as a child; its log goes to factoryd's.
pub(crate) fn launch_process() -> std::io::Result<AppsLink> {
    let Some(program) = program() else {
        return Err(std::io::Error::new(std::io::ErrorKind::NotFound, "Claktory's apps host isn't installed here."));
    };
    let mut child = tokio::process::Command::new(&program)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()?;
    let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return Err(std::io::Error::other("the apps host has no stdin or stdout"));
    };
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            use tokio::io::AsyncBufReadExt;
            let mut lines = tokio::io::BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::info!(target: "claktory_apps", "{line}");
            }
        });
    }
    Ok(AppsLink { reader: Box::new(stdout), writer: Box::new(stdin), child: Some(child) })
}

/// What the host sends that isn't an answer: its requests (answered with
/// `Host::answer`) and one-way messages.
pub(crate) type Handler = Arc<dyn Fn(Arc<Host>, u64, Body) + Send + Sync>;

pub(crate) struct Host {
    out: mpsc::UnboundedSender<Frame>,
    pending: Mutex<HashMap<u64, oneshot::Sender<Body>>>,
    next: AtomicU64,
    /// Cancelled once the host is gone.
    pub(crate) gone: CancellationToken,
    child: Mutex<Option<tokio::process::Child>>,
}

impl Host {
    /// Talks over `link`: `handle` gets what the host sends besides answers.
    pub(crate) fn connect(link: AppsLink, handle: Handler) -> Arc<Host> {
        let (out, mut outgoing) = mpsc::unbounded_channel::<Frame>();
        let host = Arc::new(Host {
            out,
            pending: Mutex::new(HashMap::new()),
            next: AtomicU64::new(1),
            gone: CancellationToken::new(),
            child: Mutex::new(link.child),
        });
        let (mut reader, mut writer) = (link.reader, link.writer);
        let gone = host.gone.clone();
        tokio::spawn(async move {
            loop {
                let frame = tokio::select! {
                    frame = outgoing.recv() => frame,
                    () = gone.cancelled() => None,
                };
                let Some(frame) = frame else { break };
                let bytes = frame.encode_to_vec();
                let length = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
                if writer.write_all(&length.to_be_bytes()).await.is_err()
                    || writer.write_all(&bytes).await.is_err()
                    || writer.flush().await.is_err()
                {
                    break;
                }
            }
            gone.cancel();
        });
        let weak = Arc::downgrade(&host);
        tokio::spawn(async move {
            loop {
                let mut length = [0u8; 4];
                if reader.read_exact(&mut length).await.is_err() {
                    break;
                }
                let length = u32::from_be_bytes(length) as usize;
                if length > MAX_FRAME {
                    tracing::error!(length, "the apps host sent a frame too big");
                    break;
                }
                let mut bytes = vec![0; length];
                if reader.read_exact(&mut bytes).await.is_err() {
                    break;
                }
                let Ok(frame) = Frame::decode(bytes.as_slice()) else {
                    tracing::error!("the apps host sent a frame that can't be read");
                    break;
                };
                let Some(host) = weak.upgrade() else { break };
                let Some(body) = frame.body else { continue };
                if frame.reply_to != 0 {
                    if let Some(waiting) = host.pending.lock().expect("pending").remove(&frame.reply_to) {
                        let _ = waiting.send(body);
                    }
                    continue;
                }
                handle(host, frame.id, body);
            }
            if let Some(host) = weak.upgrade() {
                host.gone.cancel();
                // Whoever waits hears it's gone.
                host.pending.lock().expect("pending").clear();
            }
        });
        host
    }

    /// Asks the host, waiting at most `deadline` for its answer.
    pub(crate) async fn ask(&self, body: Body, deadline: Duration) -> Result<Body, String> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tell, answer) = oneshot::channel();
        self.pending.lock().expect("pending").insert(id, tell);
        if self.out.send(Frame { id, reply_to: 0, body: Some(body) }).is_err() || self.gone.is_cancelled() {
            self.pending.lock().expect("pending").remove(&id);
            return Err("Claktory's apps host stopped.".to_owned());
        }
        let stopped = || Err("Claktory's apps host stopped.".to_owned());
        tokio::select! {
            answered = tokio::time::timeout(deadline, answer) => match answered {
                Ok(Ok(body)) => Ok(body),
                Ok(Err(_)) => stopped(),
                Err(_) => {
                    self.pending.lock().expect("pending").remove(&id);
                    Err("Claktory's apps host didn't answer in time.".to_owned())
                }
            },
            () = self.gone.cancelled() => stopped(),
        }
    }

    /// Answers the host's request `id`.
    pub(crate) fn answer(&self, id: u64, body: Body) {
        let _ = self.out.send(Frame { id: 0, reply_to: id, body: Some(body) });
    }

    /// Ends the host: its process too.
    pub(crate) fn close(&self) {
        self.gone.cancel();
        if let Some(mut child) = self.child.lock().expect("child").take() {
            let _ = child.start_kill();
        }
    }
}
