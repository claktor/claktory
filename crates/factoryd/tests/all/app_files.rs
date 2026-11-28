//! Files apps read: a CSV or TSV file in
//! the workspace or a project's folder, given only on a card when Rivet
//! asks (whatever folder it's in), read as one table at every query, so
//! it's live. Every read walks its path again: no symbolic link anywhere in
//! it, no hidden name, never inside Claktory's own folder but its
//! workspace, only a regular file with one name, at most 10 MB.

use std::path::Path;

use factory_protocol::apps::DataError;
use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};
use serde_json::{Value, json};

use crate::app_access::{app, call, card, charts, decide, set};
use crate::app_data::Stand;
use crate::common::{Client, View, apply_request, create_project, temp_dir};

const BUGS: &str = "file:web-site:reports/bugs.csv";

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().expect("a folder")).expect("its folder");
    std::fs::write(path, text).expect("written");
}

async fn rows(stand: &Stand, source: &str, sql: &str) -> Result<Value, (DataError, String)> {
    let answer = stand.query(source, sql, "").await;
    if answer.error() != DataError::None {
        return Err((answer.error(), answer.message));
    }
    let json: Value = serde_json::from_str(&answer.json).expect("JSON");
    Ok(json["rows"].clone())
}

/// The person gives Charts `source` (Marketplace → Apps): what's said.
async fn give(client: &mut Client, view: &mut View, source: &str) -> Result<(), String> {
    let request = Request::GrantAppSource(v1::GrantAppSource { app_id: "charts".into(), source_id: source.into(), granted: true });
    match apply_request(client, view, request).await {
        Body::Ack(_) => Ok(()),
        Body::Error(error) => Err(error.message),
        other => panic!("expected Ack or Error, got {other:?}"),
    }
}

/// A project "Web Site" at `dir/site`, with reports/bugs.csv.
async fn site(client: &mut Client, view: &mut View, dir: &Path) {
    let site = dir.join("site");
    write(&site.join("reports/bugs.csv"), "severity,count\nhigh,3\nlow,5\n");
    apply_request(client, view, create_project("Web Site", &site)).await;
    apply_request(client, view, Request::SelectAgent(v1::SelectAgent { agent_id: "rivet".into() })).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_is_given_only_on_a_card_and_read_live() {
    let dir = temp_dir();
    let (mut client, mut view, _factory, stand, _) = charts(&dir).await;
    site(&mut client, &mut view, &dir).await;
    assert!(app(&view).reads_files, "its manifest asks for files");

    let listed = call(&mut client, &mut view, "apps_list", &json!({"app": "charts"})).await;
    assert!(listed.contains("- Files: it reads CSV or TSV files you name as file:<place>:<path>"), "{listed}");

    // Never at once, wherever it is: on a card, with the exact path.
    let reason = "Matt wants a chart of Lea's bugs by severity.";
    let said = set(&mut client, &mut view, &json!({"app": "charts", "sources": {BUGS: true}, "reason": reason})).await;
    assert!(
        said.contains("reports/bugs.csv in Web Site holds what people wrote, so letting Charts read it needs your approval."),
        "{said}"
    );
    let (seq, shown) = card(&view);
    let source = shown.sources.first().expect("the file");
    assert_eq!((source.id.as_str(), source.label.as_str(), source.kind()), (BUGS, "reports/bugs.csv in Web Site", v1::AppDataKind::File));
    let full = dir.join("site").join("reports").join("bugs.csv");
    assert!(source.holds.contains(&full.display().to_string()) && source.private, "{source:?}");
    assert!(rows(&stand, BUGS, "SELECT 1").await.is_err(), "nothing given yet");

    // Approved: a table, and live.
    let reply = decide(&mut client, &mut view, seq, true, &shown.digest).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    view.follow(&mut client, |view| app(view).sources.iter().any(|source| source.id == BUGS && source.granted)).await;
    assert_eq!(rows(&stand, BUGS, "SELECT severity, count FROM rows ORDER BY 1").await.expect("read"), json!([["high", 3], ["low", 5]]));
    write(&full, "severity,count\nhigh,3\nlow,5\nmedium,4\n");
    assert_eq!(rows(&stand, BUGS, "SELECT sum(count) FROM rows").await.expect("read again"), json!([[12]]));
    let (error, message) = rows(&stand, BUGS, "SELECT * FROM sqlite_master").await.expect_err("only rows");
    assert_eq!(error, DataError::Invalid, "{message}");
    assert!(message.contains("one table, rows"), "{message}");

    // On the Map, and another path is another card.
    let map = view.workspace.map.clone().expect("map");
    assert!(map.edges.iter().any(|edge| edge.from_id == "app:charts" && edge.to_id == format!("data:{BUGS}")), "{:?}", map.edges);
    write(&dir.join("site/reports/old.csv"), "a\n1\n");
    let said =
        set(&mut client, &mut view, &json!({"app": "charts", "sources": {"file:web-site:reports/old.csv": true}, "reason": reason})).await;
    assert!(said.contains("reports/old.csv in Web Site holds what people wrote"), "{said}");
    assert_ne!(card(&view).0, seq);

    // What Rivet names that no app may read is refused, saying why.
    for (id, why) in [
        ("file:web-site:reports/none.csv", "There's no file reports/none.csv in Web Site."),
        ("file:web-site:.secret/x.csv", "a hidden name"),
        ("file:nowhere:x.csv", "There's no project \"nowhere\""),
        ("file:web-site:notes.txt", "isn't a .csv or .tsv"),
    ] {
        let said = set(&mut client, &mut view, &json!({"app": "charts", "sources": {id: true}, "reason": reason})).await;
        assert!(said.contains(why) && said.ends_with("Nothing was changed."), "{id}: {said}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn only_a_plain_file_with_one_name_outside_claktorys_own_folder_is_read_and_every_read_checks_again() {
    let dir = temp_dir();
    let (mut client, mut view, _factory, stand, _) = charts(&dir).await;
    site(&mut client, &mut view, &dir).await;
    let site = dir.join("site");
    let home = dir.join("home");
    write(&dir.join("outside/x.csv"), "a\n1\n");
    write(&dir.join("outside/reports/bugs.csv"), "severity,count\nswapped,1\n");
    crate::common::symlink(&site.join("reports/bugs.csv"), &site.join("link.csv")).expect("a link inside");
    crate::common::symlink(&dir.join("outside"), &site.join("out")).expect("a link outside");
    // On Windows factory.db is held open without sharing, so a second name
    // for it couldn't even be opened: one for a file of the home's no one
    // holds shows the refusal itself.
    let records = if cfg!(windows) { home.join("records.db") } else { home.join("factory.db") };
    if cfg!(windows) {
        write(&records, "a\n1\n");
    }
    std::fs::hard_link(&records, site.join("records.csv")).expect("a hard link to the records");
    // Windows has no FIFO; a named pipe isn't in a folder.
    #[cfg(unix)]
    {
        let fifo = std::ffi::CString::new(site.join("pipe.csv").display().to_string()).expect("a path");
        // SAFETY: a valid C string.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0, "a FIFO");
    }
    let big = std::fs::File::create(site.join("big.csv")).expect("big");
    big.set_len(10 * 1024 * 1024 + 1).expect("over 10 MB");
    // Closed: Windows opens a file for an app only while no one writes to it.
    drop(big);
    // A project at the folder Claktory's own is in, as one at the home
    // folder would be.
    write(&home.join("secret.csv"), "a\n1\n");
    write(&home.join("agents/rivet/x.csv"), "a\n1\n");
    write(&home.join("workspace/ok.csv"), "a\n1\n");
    write(&dir.join(".ssh/x.csv"), "a\n1\n");
    apply_request(&mut client, &mut view, create_project("Above", &dir)).await;

    for (id, why) in [
        ("file:web-site:link.csv", "is reached through a link (link.csv)"),
        ("file:web-site:out/x.csv", "is reached through a link (out)"),
        ("file:web-site:records.csv", "has more than one name on this computer"),
        ("file:web-site:pipe.csv", "isn't a file apps can read"),
        ("file:web-site:big.csv", "is over 10 MB"),
        ("file:above:home/secret.csv", "is in Claktory's own folder, which apps can't read"),
        ("file:above:home/agents/rivet/x.csv", "is in Claktory's own folder, which apps can't read"),
        ("file:above:.ssh/x.csv", "a hidden name"),
    ]
    .into_iter()
    // No FIFO on Windows.
    .filter(|(id, _)| cfg!(unix) || !id.ends_with("pipe.csv"))
    {
        let said = give(&mut client, &mut view, id).await.expect_err(id);
        assert!(said.contains(why), "{id}: {said}");
    }
    assert!(app(&view).sources.iter().all(|source| !source.id.starts_with("file:") || !source.granted));

    // "Add a file…" lists each place's files once, by their place, newest
    // first: never through a link, a hidden name or Claktory's own folder,
    // and says why one can't be given.
    let list = |query: &str| Request::ListAppFiles(v1::ListAppFiles { app_id: "charts".into(), query: query.into() });
    let Body::AppFiles(files) = apply_request(&mut client, &mut view, list("")).await else { panic!("files") };
    let mut shown: Vec<(&str, &str)> = files.files.iter().map(|file| (file.id.as_str(), file.why.as_str())).collect();
    shown.sort();
    assert_eq!(
        shown,
        [
            ("file:above:outside/reports/bugs.csv", ""),
            ("file:above:outside/x.csv", ""),
            ("file:web-site:big.csv", "It's over 10 MB, more than an app can read."),
            ("file:web-site:records.csv", "It has more than one name on this computer, so apps can't read it."),
            ("file:web-site:reports/bugs.csv", ""),
            ("file:workspace:ok.csv", ""),
        ]
    );
    assert!(!files.truncated);
    assert!(files.files.windows(2).all(|pair| pair[0].modified_ms >= pair[1].modified_ms), "newest first");
    let Body::AppFiles(files) = apply_request(&mut client, &mut view, list("BUGS web")).await else { panic!("files") };
    let labels: Vec<&str> = files.files.iter().map(|file| file.label.as_str()).collect();
    assert_eq!(labels, ["reports/bugs.csv in Web Site"]);

    // The workspace is Claktory's, and apps may read it: as itself, or
    // through a project above it.
    for id in ["file:workspace:ok.csv", "file:above:home/workspace/ok.csv"] {
        give(&mut client, &mut view, id).await.expect(id);
        assert_eq!(rows(&stand, id, "SELECT a FROM rows").await.expect(id), json!([[1]]));
    }

    // Given, then changed under it: each read walks the path again.
    give(&mut client, &mut view, BUGS).await.expect("bugs");
    assert_eq!(rows(&stand, BUGS, "SELECT count(*) FROM rows").await.expect("read"), json!([[2]]));
    std::fs::rename(site.join("reports"), site.join("reports-old")).expect("moved away");
    crate::common::symlink(&dir.join("outside/reports"), &site.join("reports")).expect("a link in its place");
    let (_, message) = rows(&stand, BUGS, "SELECT * FROM rows").await.expect_err("a link now");
    assert!(message.contains("is reached through a link (reports)"), "{message}");
    // A link to a folder is a folder on Windows.
    #[cfg(unix)]
    std::fs::remove_file(site.join("reports")).expect("the link");
    #[cfg(windows)]
    std::fs::remove_dir(site.join("reports")).expect("the link");
    std::fs::rename(site.join("reports-old"), site.join("reports")).expect("back");
    std::fs::hard_link(site.join("reports/bugs.csv"), dir.join("outside/second-name.csv")).expect("a second name");
    let (_, message) = rows(&stand, BUGS, "SELECT * FROM rows").await.expect_err("two names now");
    assert!(message.contains("more than one name"), "{message}");
    std::fs::remove_file(dir.join("outside/second-name.csv")).expect("one name again");
    std::fs::File::options().write(true).open(site.join("reports/bugs.csv")).expect("bugs").set_len(10 * 1024 * 1024 + 1).expect("grown");
    let (_, message) = rows(&stand, BUGS, "SELECT * FROM rows").await.expect_err("too big now");
    assert!(message.contains("is over 10 MB"), "{message}");
    // Still given: the person takes it back.
    assert!(app(&view).sources.iter().any(|source| source.id == BUGS && source.granted));
}
