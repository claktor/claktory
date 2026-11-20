//! Frames between factoryd and the host: a u32 length (big-endian), then a
//! `Frame`'s protobuf bytes (proto/factory/apps/v1/host.proto).

use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use factory_protocol::apps::{Frame, frame::Body};
use prost::Message;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};

/// The largest frame either side sends or takes.
pub const MAX_FRAME: usize = 16 << 20;

/// The next frame, or None at the end of the stream.
pub async fn read(input: &mut (impl AsyncRead + Unpin)) -> io::Result<Option<Frame>> {
    let mut length = [0; 4];
    match input.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let length = u32::from_be_bytes(length) as usize;
    if length > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {length} bytes, more than {MAX_FRAME}")));
    }
    let mut bytes = vec![0; length];
    input.read_exact(&mut bytes).await?;
    Frame::decode(bytes.as_slice()).map(Some).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

/// Writes one frame and flushes it.
pub async fn write(output: &mut (impl AsyncWrite + Unpin), frame: &Frame) -> io::Result<()> {
    let bytes = frame.encode_to_vec();
    if bytes.len() > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("a frame of {} bytes, more than {MAX_FRAME}", bytes.len())));
    }
    let mut framed = Vec::with_capacity(4 + bytes.len());
    framed.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    framed.extend_from_slice(&bytes);
    output.write_all(&framed).await?;
    output.flush().await
}

/// What the host sends factoryd: answers, one-way frames, and its own
/// requests (GetSecret), whose answers come back through `answered`.
pub struct Outbox {
    frames: mpsc::UnboundedSender<Frame>,
    next_id: AtomicU64,
    waiting: Mutex<HashMap<u64, oneshot::Sender<Frame>>>,
}

impl Outbox {
    /// An outbox whose frames go to `frames`, in order.
    pub fn new(frames: mpsc::UnboundedSender<Frame>) -> Outbox {
        Outbox { frames, next_id: AtomicU64::new(1), waiting: Mutex::new(HashMap::new()) }
    }

    /// Answers factoryd's request `reply_to`.
    pub fn answer(&self, reply_to: u64, body: Body) {
        let _ = self.frames.send(Frame { id: 0, reply_to, body: Some(body) });
    }

    /// Sends a one-way frame.
    pub fn tell(&self, body: Body) {
        let _ = self.frames.send(Frame { id: 0, reply_to: 0, body: Some(body) });
    }

    /// Asks factoryd and waits for its answer: None when it's gone.
    pub async fn ask(&self, body: Body) -> Option<Body> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (send, answer) = oneshot::channel();
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner).insert(id, send);
        if self.frames.send(Frame { id, reply_to: 0, body: Some(body) }).is_err() {
            self.waiting.lock().unwrap_or_else(PoisonError::into_inner).remove(&id);
            return None;
        }
        answer.await.ok()?.body
    }

    /// Hands factoryd's answer to whoever asked; false when nobody did.
    pub fn answered(&self, frame: Frame) -> bool {
        let waiting = self.waiting.lock().unwrap_or_else(PoisonError::into_inner).remove(&frame.reply_to);
        waiting.is_some_and(|waiting| waiting.send(frame).is_ok())
    }

    /// Ends every wait: factoryd is gone.
    pub fn close(&self) {
        self.waiting.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}
