//! A relay for this computer only (`--local FILE`): it takes registrations
//! and pushes like the real one, for every platform, but never calls Apple
//! or Google, so it needs no keys. Each push it would have sent is logged
//! and appended to FILE as one JSON line, with what Apple or Google would
//! have been sent (`payload`): a Simulator gets it with `xcrun simctl push`.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::json;

use crate::{BoxFuture, Delivery, Platform, Push, Transport, Urgency, apns, fcm};

pub struct Local {
    file: PathBuf,
    /// One push written at a time, so lines never interleave.
    writing: Mutex<()>,
}

impl Local {
    pub fn new(file: &Path) -> Local {
        Local { file: file.to_owned(), writing: Mutex::new(()) }
    }

    fn hold(&self, platform: Platform, token: &str, push: &Push) -> std::io::Result<()> {
        let payload = match platform {
            Platform::Apns | Platform::ApnsSandbox => apns::payload(push),
            Platform::Fcm => fcm::message(token, push),
        };
        let line = json!({
            "at": crate::now(),
            "platform": platform.as_str(),
            "token": token,
            "urgency": urgency(push.urgency),
            "collapse": push.collapse,
            "thread": push.thread,
            "ttl": push.ttl,
            "payload": payload,
        });
        let _writing = self.writing.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut options = std::fs::OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&self.file)?;
        file.write_all(format!("{line}\n").as_bytes())
    }
}

fn urgency(urgency: Urgency) -> &'static str {
    match urgency {
        Urgency::Normal => "normal",
        Urgency::Urgent => "urgent",
        Urgency::Low => "low",
    }
}

impl Transport for Local {
    fn send<'a>(&'a self, platform: Platform, token: &'a str, push: &'a Push) -> BoxFuture<'a, Delivery> {
        let delivery = match self.hold(platform, token, push) {
            Ok(()) => {
                tracing::info!(
                    platform = platform.as_str(),
                    urgency = urgency(push.urgency),
                    bytes = push.ciphertext.len(),
                    file = %self.file.display(),
                    "held a push (local: not sent to Apple or Google)"
                );
                Delivery::Sent
            }
            Err(error) => Delivery::Transient(format!("can't write {}: {error}", self.file.display())),
        };
        Box::pin(async move { delivery })
    }

    fn platforms(&self) -> (bool, bool) {
        (true, true)
    }
}
