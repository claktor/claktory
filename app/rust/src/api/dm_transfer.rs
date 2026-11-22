//! History transfer between the person's own devices (dms-e2ee.md §2, E8),
//! for Dart; the transfer itself is dm/transfer.rs.
//!
//! Both devices bind a [DmTransferEndpoint]: a fresh iroh endpoint for this
//! one add-device. Dart asks the account service for a device assertion for
//! its [DmTransferEndpoint::id_hex] (`POST /v1/devices/assertion`), which is
//! what admits it to the relays; a 401 or 429 there means the transfer
//! can't start.
//!
//! - **The new device** binds first and puts [DmTransferEndpoint::id] in
//!   its offer (`DmDevice::start_handoff`); once `finish_handoff` has run,
//!   [DmTransferEndpoint::receive] waits for the person's other device and
//!   reports each conversation as it comes. It can start at once: history
//!   for a group this device hasn't joined yet waits in the store until it
//!   does. "Try again" calls it again and sends `history_again` in the self
//!   group.
//! - **The device that added it** calls [DmTransferEndpoint::send_history]
//!   right after the hand-off, and again when the new device asks
//!   (`history_again` from that leaf, while [DmDevice::has_history_for]).
//!
//! The key lasts 30 minutes from the hand-off, on both sides; after that,
//! both refuse.

use std::sync::{Arc, Mutex, OnceLock};

use anyhow::anyhow;
use flutter_rust_bridge::frb;

use crate::api::dm::DmDevice;
use crate::dm::transfer::{self, End};
use crate::frb_generated::StreamSink;

/// The transfers' own runtime: a transfer outlives the call that starts it.
static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();

fn runtime() -> tokio::runtime::Handle {
    RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("dm-transfer")
                .enable_all()
                .build()
                .expect("the history transfer's runtime")
        })
        .handle()
        .clone()
}

/// How a history transfer ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DmTransferEnd {
    /// Everything came over.
    Complete,
    /// The person skipped it, or nothing came over.
    Skipped,
    /// Some came over, not all.
    CutShort,
}

impl From<End> for DmTransferEnd {
    fn from(end: End) -> DmTransferEnd {
        match end {
            End::Complete => DmTransferEnd::Complete,
            End::Skipped => DmTransferEnd::Skipped,
            End::CutShort => DmTransferEnd::CutShort,
        }
    }
}

/// How far the new device is: conversations done of all that are coming
/// ("3 of 12 chats"); the last one has [end].
#[derive(Debug, Clone)]
pub struct DmTransferProgress {
    pub chats_done: u32,
    pub chats_total: u32,
    pub end: Option<DmTransferEnd>,
}

/// One add-device's transfer endpoint, on either device.
#[frb(opaque)]
pub struct DmTransferEndpoint {
    inner: Arc<transfer::Endpoint>,
    runtime: tokio::runtime::Handle,
}

impl DmTransferEndpoint {
    /// A new endpoint, with a new key.
    pub async fn bind() -> anyhow::Result<DmTransferEndpoint> {
        #[cfg(target_os = "android")]
        crate::android::ready()?;
        let runtime = runtime();
        let inner = runtime.spawn(transfer::Endpoint::bind()).await??;
        Ok(DmTransferEndpoint { inner: Arc::new(inner), runtime })
    }

    /// Its id, 32 bytes: what the new device's offer carries.
    #[frb(sync)]
    pub fn id(&self) -> Vec<u8> {
        self.inner.id().to_vec()
    }

    /// Its id as lowercase hex: what the device assertion is asked for.
    #[frb(sync)]
    pub fn id_hex(&self) -> String {
        hex::encode(self.inner.id())
    }

    /// On the new device, after `finish_handoff`: waits for the person's
    /// other device and takes its history into [device]'s store. The stream
    /// says how far it is, and ends with the event that has `end`.
    pub fn receive(&self, device: &DmDevice, sink: StreamSink<DmTransferProgress>) -> anyhow::Result<()> {
        let (key, until) = device
            .transfers
            .receiving()
            .ok_or_else(|| anyhow!("there's no history to receive: this device wasn't just added, or its 30 minutes are over"))?;
        let (store, keys, inner) = (device.store(), device.transfers.clone(), self.inner.clone());
        self.runtime.spawn(async move {
            let counts = Mutex::new((0u32, 0u32));
            let mut progress = |done: u64, total: u64| {
                let (done, total) = (done.min(u32::MAX as u64) as u32, total.min(u32::MAX as u64) as u32);
                *counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = (done, total);
                let _ = sink.add(DmTransferProgress { chats_done: done, chats_total: total, end: None });
            };
            let end = match inner.receive(&store, &key, until, &mut progress).await {
                Ok(end) => end,
                Err(error) => {
                    eprintln!("history transfer: {error:#}");
                    End::Skipped
                }
            };
            if end == End::Complete {
                keys.forget_receiving();
            }
            let (done, total) = *counts.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
            let _ = sink.add(DmTransferProgress { chats_done: done, chats_total: total, end: Some(end.into()) });
        });
        Ok(())
    }

    /// On the new device: stops receiving, as skipped ("Skip").
    #[frb(sync)]
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// On the device that added [to_leaf]: sends it this device's history,
    /// dialling only the endpoint its QR code named.
    pub async fn send_history(&self, device: &DmDevice, to_leaf: String) -> anyhow::Result<DmTransferEnd> {
        let to = device
            .transfers
            .sending(&to_leaf)
            .ok_or_else(|| anyhow!("there's no history to send to that device: it wasn't added here, or its 30 minutes are over"))?;
        let (store, keys, inner) = (device.store(), device.transfers.clone(), self.inner.clone());
        let end = self
            .runtime
            .spawn(async move {
                let addr = transfer::endpoint_addr(&to.endpoint)?;
                let end = inner.send(&store, &to.key, to.until, addr).await?;
                if end == End::Complete {
                    keys.forget_sending(&to_leaf);
                }
                anyhow::Ok(end)
            })
            .await??;
        Ok(end.into())
    }

    /// Closes the endpoint (once the transfer is over, or abandoned).
    pub async fn close(&self) {
        let inner = self.inner.clone();
        let _ = self.runtime.spawn(async move { inner.close().await }).await;
    }
}

impl DmDevice {
    /// Whether this device can still send its history to the new device
    /// [leaf_id] (it added it in the last 30 minutes, and it hasn't all
    /// come over yet): what a `history_again` from that leaf is answered by.
    #[frb(sync)]
    pub fn has_history_for(&self, leaf_id: String) -> bool {
        self.transfers.can_send(&leaf_id)
    }
}
