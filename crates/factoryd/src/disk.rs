//! Whether Claktory's disk still takes what it writes (friend bug #36): a
//! disk that turns read-only (ext4 does after an error) or keeps failing
//! stops everything Claktory keeps, while the backend runs on and its
//! devices hear nothing. Every `EVERY` the backend writes a small file in
//! its home and removes it; once that shows the disk read-only, or failing
//! twice in a row, it tells the person, once until the disk works again:
//! the connected apps, and a push to every registered device, with
//! notification counters saved ahead while the disk worked
//! (`notify::push::prepare`), since nothing can be saved now.

use std::io::Write;
use std::path::Path;
use std::sync::Weak;
use std::time::Duration;

use factory_protocol::v1::{self, server_message::Body};

use crate::factory::Factory;

/// How often the disk is checked.
const EVERY: Duration = Duration::from_secs(30);
/// The file written and removed to check it, in the home.
const CHECK_FILE: &str = ".disk-check";

/// What a failed write shows of the disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    /// It turned read-only (EROFS).
    ReadOnly,
    /// It answers with I/O errors (EIO).
    Failing,
}

/// Writes a small file in `root`, has it reach the disk, and removes it:
/// the error when the disk refuses.
pub fn check(root: &Path) -> std::io::Result<()> {
    let path = root.join(CHECK_FILE);
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).open(&path)?;
    file.write_all(b"ok\n")?;
    // A failing disk's errors often show only once the data goes to it.
    // On a Mac a sync is a full flush of the drive: not every half minute.
    #[cfg(not(target_os = "macos"))]
    file.sync_data()?;
    drop(file);
    std::fs::remove_file(&path)
}

/// The fault `error` shows, if it's one of a disk's (a full disk, or a
/// permission, is something else).
pub fn fault_of(error: &std::io::Error) -> Option<Fault> {
    match error.raw_os_error() {
        Some(libc::EROFS) => Some(Fault::ReadOnly),
        Some(libc::EIO) => Some(Fault::Failing),
        _ => None,
    }
}

/// Follows the checks: a fault to tell the person about, once, when one
/// is sure (read-only at once, I/O errors twice in a row).
#[derive(Default)]
pub struct Watch {
    failing: u32,
    told: Option<Fault>,
}

impl Watch {
    /// The fault to tell the person about now, if any.
    pub fn saw(&mut self, checked: &std::io::Result<()>) -> Option<Fault> {
        let fault = match checked {
            Ok(()) => {
                if let Some(fault) = self.told.take() {
                    tracing::info!(?fault, "the disk takes writes again");
                }
                self.failing = 0;
                return None;
            }
            Err(error) => match fault_of(error) {
                Some(Fault::ReadOnly) => Fault::ReadOnly,
                Some(Fault::Failing) => {
                    self.failing += 1;
                    if self.failing < 2 {
                        tracing::warn!(%error, "the disk failed a write; checking again");
                        return None;
                    }
                    Fault::Failing
                }
                None => {
                    tracing::warn!(%error, "could not check the disk");
                    return None;
                }
            },
        };
        if self.told.is_some() {
            return None;
        }
        self.told = Some(fault);
        Some(fault)
    }
}

/// Checks the disk while Claktory runs.
pub(crate) async fn monitor(factory: Weak<Factory>) {
    // Counters saved ahead now, while the disk works.
    {
        let Some(factory) = factory.upgrade() else { return };
        crate::notify::push::prepare(&factory);
    }
    let mut watch = Watch::default();
    loop {
        tokio::time::sleep(EVERY).await;
        let Some(factory) = factory.upgrade() else { return };
        let root = factory.home.root().to_owned();
        let checked = tokio::task::spawn_blocking(move || check(&root)).await.unwrap_or_else(|error| Err(std::io::Error::other(error)));
        if let Some(fault) = watch.saw(&checked) {
            tell(&factory, fault);
        }
    }
}

/// Tells the person their Claktory can't keep anything: the connected apps
/// and every registered device, saving nothing. How many devices it was
/// pushed to.
pub fn tell(factory: &Factory, fault: Fault) -> usize {
    tracing::error!(?fault, home = %factory.home.root().display(), "the disk can't be written: telling the person");
    let config = factory.lock().config.clone();
    let name = config.claktory_name();
    let body = match fault {
        Fault::ReadOnly => {
            "Its disk became read-only, so nothing your agents do is kept. Repair the disk on the computer it runs on, then start Claktory again."
        }
        Fault::Failing => "Its disk keeps failing, so what your agents do may not be kept. Check the disk on the computer it runs on.",
    };
    let title = format!("{name} can't save anything");
    // Kept in Needs you too, in memory while factory.db can't take it.
    let alert = crate::notify::alerts::keep(
        factory,
        v1::Alert { from_name: name.clone(), title: title.clone(), text: body.to_owned(), urgent: true, ..Default::default() },
    );
    let now = crate::transcript::now_ms();
    let notify = v1::Notify {
        id: format!("{now}-disk"),
        alert_id: alert,
        kind: v1::NotifyKind::Alert.into(),
        title,
        body: body.to_owned(),
        urgent: true,
        collapse: "disk".to_owned(),
        thread: "disk".to_owned(),
        sent_ms: now,
        ..Default::default()
    };
    factory.send(Body::Notify(notify.clone()));
    let settings = crate::notify::settings(factory);
    crate::notify::push::emergency(factory, &settings, &name, &notify, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(code: i32) -> std::io::Result<()> {
        Err(std::io::Error::from_raw_os_error(code))
    }

    #[test]
    fn a_read_only_disk_is_told_at_once_a_failing_one_the_second_time_and_each_once() {
        let mut watch = Watch::default();
        assert_eq!(watch.saw(&Ok(())), None);
        assert_eq!(watch.saw(&error(libc::EROFS)), Some(Fault::ReadOnly));
        assert_eq!(watch.saw(&error(libc::EROFS)), None, "once");
        assert_eq!(watch.saw(&Ok(())), None, "working again");
        assert_eq!(watch.saw(&error(libc::EIO)), None, "one error could be a passing one");
        assert_eq!(watch.saw(&Ok(())), None);
        assert_eq!(watch.saw(&error(libc::EIO)), None);
        assert_eq!(watch.saw(&error(libc::EIO)), Some(Fault::Failing));
        assert_eq!(watch.saw(&error(libc::EROFS)), None, "already told");
        // A full disk or a permission is something else.
        let mut watch = Watch::default();
        assert_eq!(watch.saw(&error(libc::ENOSPC)), None);
        assert_eq!(watch.saw(&error(libc::EACCES)), None);
    }

    #[test]
    fn the_check_writes_nothing_it_leaves_behind() {
        let root = std::env::temp_dir().join(format!("factoryd-disk-check-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("folder");
        check(&root).expect("a working disk");
        assert!(!root.join(CHECK_FILE).exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
