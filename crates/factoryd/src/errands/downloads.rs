//! Downloads in an errand (#435 R9, agent-browser.md R9; Fugo 36893,
//! 36912): a file Chrome downloads while the errand's agent drives it, from
//! a page of the errand's sites, with `browser_download` or a click. It's a
//! hostile page's bytes landing inside the boundary: Chrome saves it under
//! its GUID in the runner's download folder; P4's host (`land`) reads it
//! over C1, counts and hashes it itself, keeps it in a store by its hash
//! and copies it, named by Claktory and inert, into the errand agent's
//! folder. Anyone else gets it only with `errand_file`, which marks them as
//! having read the page it came from.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::json;
use tokio::sync::broadcast;

use super::cdp::Cdp;

/// A download Chrome finished for an errand: what P4's host lands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Download {
    /// The errand, and its agent, whose folder gets the copy.
    pub errand: String,
    pub agent: String,
    /// Chrome's GUID for it: the runner's file name.
    pub guid: String,
    /// The name the page suggested: a hint only, cleaned by the host.
    pub suggested: String,
    /// Where the file came from, after redirects.
    pub url: String,
    /// The origin of the errand's page it started from.
    pub page_origin: String,
}

/// A download landed in the errand agent's folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Landed {
    /// The name Claktory gave it.
    pub name: String,
    /// The agent's copy, absolute.
    pub path: PathBuf,
    pub bytes: u64,
    /// Its SHA-256, in hex, as factoryd computed it.
    pub sha256: String,
    /// "" or what it is that could run ("an installer", "a script"): kept
    /// all the same, with no execute bit.
    pub flag: String,
}

/// A landed download as the store keeps it: what `errand_file` copies.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct StoredRow {
    /// The origin of the errand's page it started from, and the host it
    /// came from.
    pub origin: String,
    pub file_host: String,
    pub bytes: u64,
    pub sha256: String,
    pub flag: String,
}

/// What landing a download comes to: the copy, or why not in a line the
/// agent may read.
pub(crate) type Landing = Pin<Box<dyn std::future::Future<Output = Result<Landed, String>> + Send>>;

/// How long a download may take to begin after what started it.
pub(crate) const BEGINS: Duration = Duration::from_secs(20);
/// How long a step waits for downloads to finish before telling the agent
/// they're still going.
pub(crate) const FINISHES: Duration = Duration::from_secs(10 * 60);
/// After a click: how long a download it starts may take to begin.
pub(crate) const AFTER_CLICK: Duration = Duration::from_secs(3);

/// Where a download is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Progress {
    Going,
    Completed,
    Canceled,
}

/// A download Chrome began for the errand's agent.
#[derive(Clone, Debug)]
pub(crate) struct Begun {
    pub guid: String,
    pub url: String,
    pub suggested: String,
    pub progress: Progress,
}

/// The downloads of the page an errand drives: only those that begin while
/// the agent asked for one (browser_download, a click) are its; any other
/// is cancelled and deleted unread.
#[derive(Default)]
pub(crate) struct Tracker {
    state: Mutex<Tracked>,
    changed: tokio::sync::Notify,
}

#[derive(Default)]
struct Tracked {
    /// Until when a download that begins is the agent's.
    expecting: Option<Instant>,
    /// Its downloads not reported yet, oldest first.
    begun: Vec<Begun>,
    /// How many have begun, ever.
    count: usize,
}

impl Tracker {
    /// A download that begins until `until` is the agent's.
    pub(crate) fn expect(&self, until: Instant) {
        crate::locking::lock(&self.state).expecting = Some(until);
    }

    /// No more downloads are the agent's.
    pub(crate) fn expect_none(&self) {
        crate::locking::lock(&self.state).expecting = None;
    }

    /// How many have begun so far.
    pub(crate) fn count(&self) -> usize {
        crate::locking::lock(&self.state).count
    }

    /// Chrome began `begun`: whether it's the agent's (else it's cancelled).
    fn begin(&self, begun: Begun) -> bool {
        let mine = {
            let mut state = crate::locking::lock(&self.state);
            let mine = state.expecting.is_some_and(|until| Instant::now() <= until) && state.begun.len() < MAX_BEGUN;
            if mine {
                state.begun.push(begun);
                state.count += 1;
            }
            mine
        };
        self.changed.notify_waiters();
        mine
    }

    fn progress(&self, guid: &str, progress: Progress) {
        {
            let mut state = crate::locking::lock(&self.state);
            if let Some(begun) = state.begun.iter_mut().find(|begun| begun.guid == guid) {
                begun.progress = progress;
            }
        }
        self.changed.notify_waiters();
    }

    /// Waits until more than `count` have begun, or `until`: whether any did.
    pub(crate) async fn begun_since(&self, count: usize, until: Instant) -> bool {
        loop {
            let changed = self.changed.notified();
            if self.count() > count {
                return true;
            }
            if tokio::time::timeout_at(until.into(), changed).await.is_err() {
                return self.count() > count;
            }
        }
    }

    /// Waits until none is going, or `until`; then takes the finished ones
    /// (completed or cancelled), leaving those still going.
    pub(crate) async fn settled(&self, until: Instant) -> (Vec<Begun>, usize) {
        loop {
            let changed = self.changed.notified();
            let going = crate::locking::lock(&self.state).begun.iter().any(|begun| begun.progress == Progress::Going);
            if !going || tokio::time::timeout_at(until.into(), changed).await.is_err() {
                break;
            }
        }
        let mut state = crate::locking::lock(&self.state);
        let (finished, going): (Vec<Begun>, Vec<Begun>) =
            std::mem::take(&mut state.begun).into_iter().partition(|begun| begun.progress != Progress::Going);
        let still = going.len();
        state.begun = going;
        (finished, still)
    }
}

/// The most downloads not reported yet an errand keeps track of.
const MAX_BEGUN: usize = 50;

/// Whether `guid` is a download GUID as Chrome writes them.
fn is_guid(guid: &str) -> bool {
    guid.len() == 36 && guid.bytes().all(|byte| byte.is_ascii_hexdigit() || byte == b'-')
}

/// Follows the downloads of `cdp`'s Chrome until it closes: a download
/// that isn't the agent's is cancelled and deleted unread.
pub(crate) fn watch(cdp: Arc<Cdp>, tracker: Arc<Tracker>, browser: Arc<dyn super::Browser>) {
    let mut events = cdp.events();
    tokio::spawn(async move {
        loop {
            let event = match events.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            };
            let guid = event.params["guid"].as_str().filter(|guid| is_guid(guid)).map(str::to_owned);
            let Some(guid) = guid else { continue };
            match event.method.as_str() {
                "Browser.downloadWillBegin" => {
                    let text = |key: &str| event.params[key].as_str().unwrap_or_default().chars().take(2048).collect::<String>();
                    let begun = Begun { guid: guid.clone(), url: text("url"), suggested: text("suggestedFilename"), progress: Progress::Going };
                    if !tracker.begin(begun) {
                        let _ = cdp.call("Browser.cancelDownload", json!({"guid": guid}), None).await;
                        browser.drop_download(&guid);
                    }
                }
                "Browser.downloadProgress" => match event.params["state"].as_str() {
                    Some("completed") => tracker.progress(&guid, Progress::Completed),
                    Some("canceled") => tracker.progress(&guid, Progress::Canceled),
                    _ => {}
                },
                _ => {}
            }
        }
    });
}

/// A file's size for the agent: "351 KB", "4.2 MB".
pub(crate) fn size(bytes: u64) -> String {
    match bytes {
        0..1_000 => format!("{bytes} bytes"),
        1_000..1_000_000 => format!("{} KB", bytes / 1_000),
        _ => format!("{:.1} MB", bytes as f64 / 1_000_000.0),
    }
}

/// A file tool (read_file, read_image, the edit tools, show) used `path`
/// (relative to `folder`, `agent`'s): if it's a download, in an errand
/// agent's folder or a copy errand_file made, `agent` has read the pages
/// of its errand (R9, Fugo 4: depth on top of errand_file; bash isn't
/// matched on paths).
pub(crate) fn touched(factory: &crate::factory::Factory, agent: &str, folder: &Path, path: &str) {
    let path = Path::new(path);
    let full = if path.is_absolute() { path.to_path_buf() } else { folder.join(path) };
    let full = std::fs::canonicalize(&full).unwrap_or(full);
    let root = std::fs::canonicalize(factory.home.root()).unwrap_or_else(|_| factory.home.root().to_path_buf());
    let Some(errand) = errand_of(&root, &full).and_then(|id| factory.errands.get(&id)) else { return };
    for site in &errand.sites {
        crate::pipes::agents::taint(factory, agent, &crate::pipes::agents::Kind::Browser(site).key());
    }
}

/// The errand whose download `path` is, by where it is under Claktory's
/// folder `root`: <root>/errands/<agent>/downloads/<errand>/… (its agent's
/// copy) or <root>/agents/<agent>/downloads/<errand>/… (errand_file's).
fn errand_of(root: &Path, path: &Path) -> Option<String> {
    let rest = path.strip_prefix(root).ok()?;
    let parts: Vec<&str> = rest.components().map(|part| part.as_os_str().to_str()).collect::<Option<_>>()?;
    match parts.as_slice() {
        ["errands" | "agents", _, "downloads", errand, ..] if errand.starts_with("errand-") => Some((*errand).to_owned()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downloads_are_known_by_where_they_are() {
        let root = Path::new("/h/.factory");
        let of = |path: &str| errand_of(root, Path::new(path));
        assert_eq!(of("/h/.factory/errands/runner/downloads/errand-1/a.zip").as_deref(), Some("errand-1"));
        assert_eq!(of("/h/.factory/agents/lead/downloads/errand-1/sub/a.zip").as_deref(), Some("errand-1"));
        assert_eq!(of("/h/.factory/agents/lead/downloads/errand-1").as_deref(), Some("errand-1"));
        for elsewhere in [
            "/h/.factory/agents/lead/uploads/a.zip",
            "/h/.factory/agents/lead/downloads/mine.zip",
            "/h/.factory/workspace/downloads/errand-1/a.zip",
            "/elsewhere/errands/runner/downloads/errand-1/a.zip",
        ] {
            assert_eq!(of(elsewhere), None, "{elsewhere}");
        }
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(size(512), "512 bytes");
        assert_eq!(size(351_200), "351 KB");
        assert_eq!(size(4_200_000), "4.2 MB");
    }
}

#[cfg(test)]
mod chrome_tests {
    use super::*;

    /// A download the page makes reaches the tracker, begun and completed,
    /// when it was expected; the file is in the folder under its GUID.
    #[tokio::test]
    async fn chrome_reports_a_download_it_was_expecting() {
        let Some((_local, page, profile)) = super::super::reader::tests::page_with(
            r#"<a id=file href="data:text/plain;base64,aGVsbG8gd29ybGQ=" download="hello.txt">x</a>"#,
        )
        .await
        else {
            return;
        };
        let folder = profile.with_extension("downloads");
        std::fs::create_dir_all(&folder).unwrap();
        let behavior = json!({"behavior": "allowAndName", "downloadPath": folder.display().to_string(), "eventsEnabled": true});
        page.cdp().call("Browser.setDownloadBehavior", behavior, None).await.unwrap();
        let tracker = Arc::new(Tracker::default());
        let browser: Arc<dyn super::super::Browser> = Arc::new(NoBrowser);
        watch(page.cdp().clone(), tracker.clone(), browser);
        tracker.expect(Instant::now() + Duration::from_secs(10));
        page.call("Runtime.evaluate", json!({"expression": "document.getElementById('file').click()"})).await.unwrap();
        assert!(tracker.begun_since(0, Instant::now() + Duration::from_secs(10)).await, "it began");
        let (finished, still) = tracker.settled(Instant::now() + Duration::from_secs(10)).await;
        assert_eq!((finished.len(), still), (1, 0));
        assert_eq!((finished[0].progress, finished[0].suggested.as_str()), (Progress::Completed, "hello.txt"));
        assert_eq!(std::fs::read_to_string(folder.join(&finished[0].guid)).unwrap(), "hello world");
    }

    struct NoBrowser;

    impl super::super::cdp::Transport for NoBrowser {
        fn connect(&self) -> Result<super::super::cdp::Pipe, String> {
            Err("none".to_owned())
        }
    }

    impl super::super::Browser for NoBrowser {}
}
