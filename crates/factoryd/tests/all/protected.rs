//! macOS's protected places (permissions.md P2, item 12): a folder added in
//! one is read at once, the person's check reads places again, a refusal
//! raises one Needs you item until its OK or a check that gets in, and each
//! ended item is in History. macOS stands in for itself here: the places
//! are under a temporary home, and what reading one finds is the test's,
//! so no test reads the real Documents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, client_message::Body as Request, server_message::Body};
use factoryd::protected::Places;

/// macOS as the test says: places under `home` (and, for an agent's bash on
/// a Mac, under the real home's names, never read), each found as set.
struct Mac {
    home: PathBuf,
    found: Mutex<HashMap<PathBuf, Option<bool>>>,
    reads: Mutex<Vec<PathBuf>>,
}

impl Mac {
    fn set(&self, root: &Path, found: Option<bool>) {
        self.found.lock().unwrap().insert(root.to_path_buf(), found);
    }

    fn reads(&self) -> Vec<PathBuf> {
        self.reads.lock().unwrap().clone()
    }
}

impl Places for Mac {
    fn root(&self, path: &Path) -> Option<PathBuf> {
        let real = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        factory_agent::tools::protected::protected_root_in(&self.home, path)
            .or_else(|| factory_agent::tools::protected::protected_root_in(&real, path))
    }

    fn read(&self, root: &Path) -> Option<bool> {
        self.reads.lock().unwrap().push(root.to_path_buf());
        self.found.lock().unwrap().get(root).copied().unwrap_or(Some(true))
    }

    fn all(&self) -> Vec<PathBuf> {
        vec![self.home.join("Documents"), self.home.join("Library/Mobile Documents"), PathBuf::from("/Volumes/Backup")]
    }
}

async fn setup(dir: &Path) -> (Arc<factoryd::Factory>, Arc<Mac>, Client, View) {
    let (addr, factory) = start(dir).await;
    let mac = Arc::new(Mac { home: dir.join("user"), found: Mutex::default(), reads: Mutex::default() });
    std::fs::create_dir_all(dir.join("user/Documents")).unwrap();
    factory.set_protected_places(mac.clone());
    let (client, workspace, _) = Client::connect(addr).await;
    (factory, mac, client, View { workspace, ..View::default() })
}

fn places(view: &View) -> &[v1::ProtectedPlace] {
    &view.workspace.protected_places
}

fn create_folder(project: &str, name: &str, path: &Path) -> Request {
    Request::CreateFolder(v1::CreateFolder { project_id: project.into(), name: name.into(), path: path.display().to_string() })
}

async fn history(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    let list = v1::ListNeedsYouHistory { query: String::new(), before_ms: 0, limit: 0 };
    let Body::NeedsYouHistory(history) = apply_request(client, view, Request::ListNeedsYouHistory(list)).await else { panic!("history") };
    history.records.into_iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::ProtectedPlace).collect()
}

/// Illuso 49904: the app tells a path typed is in a protected place before
/// anything is saved, from every place there is, used or not.
#[tokio::test]
async fn the_workspace_lists_every_protected_place_with_its_name() {
    let dir = temp_dir();
    let (_factory, _mac, _client, view) = setup(&dir).await;
    let roots: Vec<(String, String)> = view.workspace.protected_roots.iter().map(|root| (root.root.clone(), root.name.clone())).collect();
    let user = dir.join("user");
    assert_eq!(
        roots,
        [
            (user.join("Documents").display().to_string(), "Documents".to_owned()),
            (user.join("Library/Mobile Documents").display().to_string(), "iCloud Drive".to_owned()),
            ("/Volumes/Backup".to_owned(), "Backup".to_owned()),
        ]
    );
    assert!(places(&view).is_empty(), "none in use");
}

#[tokio::test]
async fn a_folder_in_documents_is_read_at_once_and_a_refusal_waits_in_needs_you_until_ok() {
    let dir = temp_dir();
    let (_factory, mac, mut client, mut view) = setup(&dir).await;
    let documents = dir.join("user/Documents");
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    assert!(places(&view).is_empty(), "no project in a protected place: {:?}", places(&view));

    // macOS refuses: the folder is added all the same, the place is read
    // at once and found refused, and its item waits.
    mac.set(&documents, Some(false));
    let reply = apply_request(&mut client, &mut view, create_folder("site", "nixos-site", &documents.join("nixos-site"))).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(mac.reads(), vec![documents.clone()]);
    view.follow(&mut client, |view| places(view).iter().any(|place| place.needs_you)).await;
    let place = &places(&view)[0];
    assert_eq!(place.root, documents.display().to_string());
    assert_eq!(place.name, "Documents");
    assert_eq!(place.state(), v1::ProtectedPlaceState::Refused);
    assert_eq!(place.used_by, ["nixos-site"]);
    assert_eq!(place.title, "Your agents can't work in Documents");
    assert_eq!(
        place.text,
        "macOS hasn't let Claktory Agents into Documents, so commands in nixos-site fail. Turn it on for Claktory Agents in System Settings."
    );
    assert!(place.checked_ms > 0 && place.agent_id.is_empty());

    // The person's OK: the item goes to History, the place stays refused,
    // and a check refused again raises nothing.
    let root = place.root.clone();
    let reply =
        apply_request(&mut client, &mut view, Request::DismissProtectedPlace(v1::DismissProtectedPlace { root: root.clone() })).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| places(view).iter().all(|place| !place.needs_you)).await;
    assert_eq!(places(&view)[0].state(), v1::ProtectedPlaceState::Refused);
    let check = || Request::CheckProtectedPlaces(v1::CheckProtectedPlaces { roots: vec![root.clone()] });
    apply_request(&mut client, &mut view, check()).await;
    assert!(places(&view).iter().all(|place| !place.needs_you), "once per refusal");
    let records = history(&mut client, &mut view).await;
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].outcome(), v1::NeedsYouOutcome::Dismissed);
    assert_eq!(records[0].title, "Your agents can't work in Documents");
    assert_eq!(
        records[0].excerpt,
        "macOS hasn't let Claktory Agents into Documents, so commands in nixos-site fail. Turn it on for Claktory Agents in System Settings."
    );

    // Let in from System Settings: the app's check finds it so.
    mac.set(&documents, Some(true));
    apply_request(&mut client, &mut view, check()).await;
    view.follow(&mut client, |view| places(view)[0].state() == v1::ProtectedPlaceState::Allowed).await;
    assert!(places(&view)[0].title.is_empty());

    // Refused again later: a new item, which a check that gets in ends.
    mac.set(&documents, Some(false));
    apply_request(&mut client, &mut view, check()).await;
    view.follow(&mut client, |view| places(view)[0].needs_you).await;
    mac.set(&documents, Some(true));
    apply_request(&mut client, &mut view, check()).await;
    view.follow(&mut client, |view| !places(view)[0].needs_you).await;
    let records = history(&mut client, &mut view).await;
    let outcomes: Vec<_> = records.iter().map(v1::NeedsYouRecord::outcome).collect();
    assert_eq!(outcomes, [v1::NeedsYouOutcome::Changed, v1::NeedsYouOutcome::Dismissed], "newest first");
}

#[tokio::test]
async fn the_app_checks_only_protected_places_and_a_place_never_checked_has_no_state() {
    let dir = temp_dir();
    let (_factory, mac, mut client, mut view) = setup(&dir).await;
    let documents = dir.join("user/Documents");
    // A project already in Documents, as before this version: listed, never
    // checked (the app's one-time sheet shows it), and not read until the
    // app asks.
    std::fs::create_dir_all(documents.join("blog")).unwrap();
    apply_request(&mut client, &mut view, create_project("Blog", &documents.join("blog"))).await;
    let place = &places(&view)[0];
    assert_eq!(
        (place.state(), place.needs_you, place.used_by.as_slice()),
        (v1::ProtectedPlaceState::Unspecified, false, &["Blog".to_owned()][..])
    );
    assert!(mac.reads().is_empty(), "nothing read before the person said so");

    // The sheet's Continue: the place is read; anything else named isn't.
    let roots = vec![documents.display().to_string(), dir.join("site").display().to_string(), "/etc".to_owned()];
    let reply = apply_request(&mut client, &mut view, Request::CheckProtectedPlaces(v1::CheckProtectedPlaces { roots })).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(mac.reads(), vec![documents.clone()]);
    view.follow(&mut client, |view| places(view)[0].state() == v1::ProtectedPlaceState::Allowed).await;

    // A place macOS can't tell about (a volume not there) keeps its state.
    mac.set(&documents, None);
    let roots = vec![documents.display().to_string()];
    apply_request(&mut client, &mut view, Request::CheckProtectedPlaces(v1::CheckProtectedPlaces { roots })).await;
    assert_eq!(places(&view)[0].state(), v1::ProtectedPlaceState::Allowed);
}

/// An agent's bash refused under the real home's Documents (only on a Mac,
/// where the tools recognise it; the place is never read): the place is
/// listed refused with its item naming the agent, and a tool's claim of a
/// place that isn't a protected root is ignored.
#[cfg(target_os = "macos")]
#[tokio::test]
async fn an_agent_refused_in_a_protected_place_raises_its_item() {
    let dir = temp_dir();
    let (_factory, mac, mut client, mut view) = setup(&dir).await;
    let site = dir.join("site");
    std::fs::create_dir_all(&site).unwrap();
    apply_request(&mut client, &mut view, create_project("Site", &site)).await;
    apply_request(&mut client, &mut view, create_agent("site", "Mista", "")).await;
    let documents = PathBuf::from(std::env::var_os("HOME").unwrap()).join("Documents");
    let line = format!("ls: {}: Operation not permitted", documents.join("notes").display());
    let call = format!(r#"call bash {{"command":"echo '{line}' >&2; exit 1"}}"#);
    apply_request(&mut client, &mut view, send_message("mista", &call)).await;
    view.follow(&mut client, |view| places(view).iter().any(|place| place.needs_you)).await;
    let place = places(&view).iter().find(|place| place.needs_you).unwrap();
    assert_eq!(place.root, documents.display().to_string());
    assert_eq!((place.state(), place.agent_id.as_str()), (v1::ProtectedPlaceState::Refused, "mista"));
    assert_eq!(
        place.text,
        "macOS hasn't let Claktory Agents into Documents, so Mista's commands there fail. Turn it on for Claktory Agents in System Settings."
    );
    assert!(place.used_by.is_empty());
    assert!(mac.reads().is_empty(), "an agent's refusal reads nothing");
    view.follow(&mut client, |view| view.agent("mista").status() == AgentStatus::Idle).await;
}
