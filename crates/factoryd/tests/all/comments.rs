//! Comments on files: the person comments on text in a viewer, sends the
//! comments to an agent as one event, and the agent answers with its tools;
//! the threads follow their text as the file is rewritten.

use crate::common::{Client, Scripted, View, apply_request, create_agent, create_project, describe, send_message, start_with, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AnchorState, ErrorCode, ThreadStatus, client_message, server_message::Body};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

/// What `agent`'s tool call said.
async fn call(client: &mut Client, view: &mut View, agent: &str, text: &str) -> String {
    let before = view.entries.len();
    apply_request(client, view, send_message(agent, text)).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, agent)).await;
    describe(&view.entries[before + 1..before + 2]).remove(0)
}

/// Applies what comes to `view` until `pick` finds what it's looking for.
async fn until<T>(client: &mut Client, view: &mut View, mut pick: impl FnMut(&Body) -> Option<T>) -> T {
    loop {
        let message = client.receive().await;
        if let Some(body) = &message.body {
            view.apply(body);
            if let Some(found) = pick(body) {
                return found;
            }
        }
    }
}

async fn turn_on_comments(client: &mut Client, view: &mut View) {
    let on = v1::ExtensionChange { id: "comments".into(), enabled: Some(true), ..Default::default() };
    let change = v1::UpdateAgent { agent_id: "writer".into(), extension: Some(on), ..Default::default() };
    apply_request(client, view, client_message::Body::UpdateAgent(change)).await;
}

fn quote(exact: &str, prefix: &str, suffix: &str) -> v1::Selector {
    v1::Selector { json: serde_json::json!({"type": "TextQuoteSelector", "exact": exact, "prefix": prefix, "suffix": suffix}).to_string() }
}

fn position(start: usize, end: usize) -> v1::Selector {
    v1::Selector { json: serde_json::json!({"type": "TextPosition", "start": start, "end": end}).to_string() }
}

fn create(path: &str, kind: &str, version: &str, selectors: Vec<v1::Selector>, body: &str) -> client_message::Body {
    client_message::Body::CreateThread(v1::CreateThread {
        agent_id: "writer".into(),
        path: path.into(),
        kind: kind.into(),
        version_sha256: version.into(),
        selectors,
        body: body.into(),
        send_now: false,
        suggestion: None,
        ink: None,
    })
}

fn list(path: &str) -> client_message::Body {
    client_message::Body::ListThreads(v1::ListThreads { path: path.into() })
}

async fn threads(client: &mut Client, path: &str) -> v1::ThreadList {
    match client.request(list(path)).await.body {
        Body::ThreadList(list) => list,
        other => panic!("expected threads, got {other:?}"),
    }
}

fn find<'a>(list: &'a v1::ThreadList, id: &str) -> &'a v1::Thread {
    list.threads.iter().find(|thread| thread.id == id).expect("thread")
}

fn utf16(text: &str) -> usize {
    text.encode_utf16().count()
}

#[tokio::test]
async fn the_person_comments_on_a_file_and_the_agent_answers_and_revises() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert!(view.workspace.extensions.iter().any(|extension| extension.id == "comments"));
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    let essay =
        "# Essay\n\nThe industrial revolution was **not** a single event but a long series of changes.\n\nIt was, arguably, slow.\n";
    std::fs::write(dir.join("site/essay.md"), essay).expect("write");
    let path = std::fs::canonicalize(dir.join("site/essay.md")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    assert_eq!(content.sha256.len(), 64, "the version comments are made on");

    // In the formatted view: a quote of what it shows, placed on the source.
    let made = client
        .request(create(&path, "markdown", &content.sha256, vec![quote("not a single event", "revolution was ", " but")], "Too vague."))
        .await;
    let Body::Thread(first) = made.body else { panic!("expected the thread, got {:?}", made.body) };
    assert_eq!((first.id.as_str(), first.anchor(), first.status()), ("t-1", AnchorState::Anchored, ThreadStatus::Open));
    let source = first.source.clone().expect("source");
    assert_eq!((source.exact.as_str(), source.first_line, source.last_line), ("**not** a single event", 3, 3));
    assert_eq!(first.rendered.as_ref().map(|rendered| rendered.exact.as_str()), Some("not a single event"));
    assert!(first.comments[0].draft && first.comments[0].author_agent_id.is_empty(), "the person's draft");
    assert!(
        made.events.iter().any(|event| matches!(&event.body, Some(Body::ThreadList(list)) if list.path == path)),
        "every device hears of it"
    );

    // In the plain view: the file's own text, by position.
    let at = utf16(&essay[..essay.find("arguably").expect("arguably")]);
    let made = client
        .request(create(&path, "text", &content.sha256, vec![quote("arguably", "was, ", ", slow"), position(at, at + 8)], "Cut hedges."))
        .await;
    assert!(matches!(made.body, Body::Thread(ref thread) if thread.id == "t-2"), "{:?}", made.body);
    match client.request(create(&path, "text", &content.sha256, vec![quote("nowhere in it at all", "", "")], "?")).await.body {
        Body::Error(error) => {
            assert_eq!((error.code(), error.message.as_str()), (ErrorCode::InvalidRequest, "The text changed; select it again."))
        }
        other => panic!("expected an error, got {other:?}"),
    }

    // Sent as one event, which the agent reads with where each is.
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::SendComments(v1::SendComments { agent_id: "writer".into(), path: path.clone(), ..Default::default() }),
    )
    .await;
    view.follow(&mut client, |view| idle(view, "writer")).await;
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    for part in [
        "left 2 comments on essay.md",
        "[t-1]",
        "essay.md, line 3: “not a single event”",
        "Too vague.",
        "[t-2]",
        "“arguably”",
        "Cut hedges.",
        "comments_reply",
    ] {
        assert!(prompt.contains(part), "{part:?} in {prompt}");
    }
    let sent = threads(&mut client, &path).await;
    assert!(sent.threads.iter().all(|thread| thread.comments.iter().all(|comment| !comment.draft)), "sent, they're drafts no more");
    match client
        .request(client_message::Body::SendComments(v1::SendComments {
            agent_id: "writer".into(),
            path: path.clone(),
            ..Default::default()
        }))
        .await
        .body
    {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("nothing left to send: {other:?}"),
    }

    // The agent answers, resolves and lists.
    let replied = call(&mut client, &mut view, "writer", r#"call comments_reply {"thread":"t-1","body":"Made it concrete."}"#).await;
    assert!(replied.starts_with("tool comments_reply: Replied on t-1"), "{replied}");
    let resolved = call(&mut client, &mut view, "writer", r#"call comments_resolve {"thread":"t-2","note":"Cut it."}"#).await;
    assert!(resolved.starts_with("tool comments_resolve: Resolved t-2"), "{resolved}");
    let listed = call(&mut client, &mut view, "writer", r#"call comments_list {}"#).await;
    assert!(listed.contains("[t-1]") && listed.contains("You: Made it concrete.") && !listed.contains("[t-2]"), "{listed}");
    let all = call(&mut client, &mut view, "writer", r#"call comments_list {"status":"all"}"#).await;
    assert!(all.contains("[t-2]") && all.contains("(resolved)"), "{all}");
    let answered = threads(&mut client, &path).await;
    let t1 = find(&answered, "t-1");
    assert_eq!((t1.comments.len(), t1.comments[1].author_agent_id.as_str()), (2, "writer"));
    assert_eq!(find(&answered, "t-2").status(), ThreadStatus::Resolved);

    // The agent rewrites the file: comments follow their text, and apps hear who changed it.
    let rewritten =
        "# Essay\n\nA new opening.\n\nThe industrial revolution was **not** a single event but a long series of changes.\n\nIt was slow.\n";
    let write = serde_json::json!({"path": "essay.md", "content": rewritten}).to_string();
    let before = view.entries.len();
    apply_request(&mut client, &mut view, send_message("writer", &format!("call write_file {write}"))).await;
    let changed = until(&mut client, &mut view, |body| match body {
        Body::FileChanged(changed) if changed.path == path => Some(changed.clone()),
        _ => None,
    })
    .await;
    assert_eq!(changed.by_agent_id, "writer");
    view.follow(&mut client, |view| view.entries.len() >= before + 3 && idle(view, "writer")).await;
    let after = threads(&mut client, &path).await;
    let t1 = find(&after, "t-1");
    assert_eq!((t1.anchor(), t1.source.as_ref().map(|source| source.first_line)), (AnchorState::Anchored, Some(5)));
    assert_eq!(find(&after, "t-2").anchor(), AnchorState::Detached, "its text is gone, the thread isn't");

    // Someone else rewords it, in a pane: it's checked, and the thread moves with the text.
    let pane = v1::ViewNode {
        id: "pane-1".into(),
        node: Some(v1::view_node::Node::Leaf(v1::ViewLeaf {
            kind: "text".into(),
            path: path.clone(),
            title: String::new(),
            ..Default::default()
        })),
    };
    let chat = v1::ViewNode {
        id: "chat".into(),
        node: Some(v1::view_node::Node::Leaf(v1::ViewLeaf {
            kind: "chat".into(),
            path: String::new(),
            title: String::new(),
            ..Default::default()
        })),
    };
    let split = v1::ViewSplit { axis: v1::ViewAxis::Row.into(), children: vec![chat, pane], weights: Vec::new() };
    let root = v1::ViewNode { id: "split".into(), node: Some(v1::view_node::Node::Split(split)) };
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::SetLayout(v1::SetLayout { agent_id: "writer".into(), root: Some(root), ..Default::default() }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    std::fs::write(&path, rewritten.replace("**not** a single event", "**not** one single event")).expect("write");
    let moved = client
        .wait_for(|body| match body {
            Body::ThreadList(list) if list.path == path && find(list, "t-1").anchor() == AnchorState::Moved => Some(list.clone()),
            _ => None,
        })
        .await;
    let t1 = find(&moved, "t-1");
    assert_eq!(t1.original_exact, "not a single event");
    assert_eq!(t1.rendered.as_ref().map(|rendered| rendered.exact.as_str()), Some("not one single event"));

    // Only the person's own comments are theirs to change; a thread left without comments goes.
    let agents = &find(&moved, "t-1").comments[1].id;
    match client.request(client_message::Body::EditComment(v1::EditComment { comment_id: agents.clone(), body: "x".into() })).await.body {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    let mine = find(&moved, "t-1").comments[0].id.clone();
    client.ok(client_message::Body::EditComment(v1::EditComment { comment_id: mine.clone(), body: "Still vague.".into() })).await;
    client
        .ok(client_message::Body::ReplyThread(v1::ReplyThread { thread_id: "t-2".into(), body: "Thanks.".into(), ..Default::default() }))
        .await;
    let now = threads(&mut client, &path).await;
    assert_eq!(find(&now, "t-1").comments[0].body, "Still vague.");
    assert!(find(&now, "t-1").comments[0].edited_ms > 0);
    assert_eq!(find(&now, "t-2").status(), ThreadStatus::Open, "a reply reopens it");
    let made = client.request(create(&path, "text", "", vec![quote("A new opening.", "", "")], "Nice.")).await;
    let Body::Thread(third) = made.body else { panic!("expected the thread") };
    client.ok(client_message::Body::DeleteComment(v1::DeleteComment { comment_id: third.comments[0].id.clone() })).await;
    assert!(threads(&mut client, &path).await.threads.iter().all(|thread| thread.id != third.id), "a thread with no comments goes");
    match client
        .request(client_message::Body::ReaddressThread(v1::ReaddressThread { thread_id: "t-1".into(), agent_id: "nobody".into() }))
        .await
        .body
    {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("expected an error, got {other:?}"),
    }

    // Kept across a restart.
    drop(client);
    let (restarted, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut again, _, _) = Client::connect(restarted).await;
    assert_eq!(threads(&mut again, &path).await.threads.len(), 2);
}

#[tokio::test]
async fn agents_comment_on_files_for_the_person_and_send_now_goes_at_once() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    std::fs::write(dir.join("site/notes.txt"), "one\ntwo\nthree\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/notes.txt")).expect("path").display().to_string();

    let added = call(
        &mut client,
        &mut view,
        "writer",
        r#"call comments_add {"path":"notes.txt","quote":{"exact":"two"},"body":"I cut the rest."}"#,
    )
    .await;
    assert!(added.contains("line 2 (t-1)"), "{added}");
    let missing =
        call(&mut client, &mut view, "writer", r#"call comments_add {"path":"notes.txt","quote":{"exact":"four"},"body":"?"}"#).await;
    assert!(missing.contains("isn't in"), "{missing}");
    let list = threads(&mut client, &path).await;
    let thread = find(&list, "t-1");
    assert_eq!((thread.comments[0].author_agent_id.as_str(), thread.comments[0].draft), ("writer", false));

    // The person's reply, sent at once.
    let before = view.entries.len();
    let reply = v1::ReplyThread { thread_id: "t-1".into(), body: "Why?".into(), send_now: true, ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::ReplyThread(reply)).await, Body::Ack(_)));
    view.follow(&mut client, |view| view.entries.len() >= before + 2 && idle(view, "writer")).await;
    let prompt = requests.lock().expect("lock").last().expect("a request").prompt.clone();
    assert!(prompt.contains("commented on notes.txt") && prompt.contains("Reply: Why?"), "{prompt}");
}

#[tokio::test]
async fn the_persons_drafts_reach_no_agent_until_sent_not_even_through_claktorys_records() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    // Writer reads Claktory's records too, as admin agents do.
    let records = v1::ExtensionChange { id: "database:factory".into(), enabled: Some(true), ..Default::default() };
    let change = v1::UpdateAgent { agent_id: "writer".into(), extension: Some(records), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdateAgent(change)).await;
    std::fs::write(dir.join("site/notes.txt"), "one\ntwo\nthree\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/notes.txt")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let made = client.request(create(&path, "text", &content.sha256, vec![quote("two", "one\n", "\nthree")], "Half-formed thought.")).await;
    assert!(matches!(made.body, Body::Thread(ref thread) if thread.comments[0].draft), "{:?}", made.body);

    // A draft: not in the Comments tools, and no table of comments in the records.
    assert_eq!(call(&mut client, &mut view, "writer", "call comments_list {}").await, "tool comments_list: No open comment threads.");
    for sql in ["SELECT body FROM thread_comments", "SELECT quote FROM threads", "SELECT text FROM thread_files"] {
        let query = serde_json::json!({ "sql": sql }).to_string();
        let refused = call(&mut client, &mut view, "writer", &format!("call db_query {query}")).await;
        assert!(refused.contains("comments on files (threads, thread_), quiet copies (quiet_copies), ink (ink_, presentations), the Map's history (map_history) and what commands in requests printed (ask_runs) aren't readable"), "{sql}: {refused}");
        assert!(!refused.contains("Half-formed"), "{refused}");
    }
    let tables = call(&mut client, &mut view, "writer", "call db_schema {}").await;
    assert!(tables.contains("messages (") && !tables.contains("thread"), "{tables}");

    // Sent, it reaches the agent as before, and the Comments tools have it.
    let send = v1::SendComments { agent_id: "writer".into(), path: path.clone(), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::SendComments(send)).await;
    view.follow(&mut client, |view| idle(view, "writer")).await;
    let listed = call(&mut client, &mut view, "writer", "call comments_list {}").await;
    assert!(listed.contains("[t-1]") && listed.contains("Half-formed thought."), "{listed}");
}

fn decide(thread: &str, accept: bool) -> client_message::Body {
    client_message::Body::DecideSuggestion(v1::DecideSuggestion { thread_id: thread.into(), accept, ..Default::default() })
}

/// The latest prompt `agent` was given once it's done with what came.
async fn last_prompt(
    client: &mut Client,
    view: &mut View,
    requests: &std::sync::Arc<std::sync::Mutex<Vec<crate::common::Request>>>,
    before: usize,
) -> String {
    view.follow(client, |view| view.entries.len() >= before + 2 && idle(view, "writer")).await;
    requests.lock().expect("lock").last().expect("a request").prompt.clone()
}

#[tokio::test]
async fn suggested_changes_are_accepted_or_rejected_and_the_agent_hears_which() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    std::fs::write(dir.join("site/notes.txt"), "one\ntwo\nthree\n").expect("write");
    let path = std::fs::canonicalize(dir.join("site/notes.txt")).expect("path").display().to_string();

    // The agent suggests; the person accepts, and Claktory makes the change.
    let said = call(
        &mut client,
        &mut view,
        "writer",
        r#"call comments_suggest {"path":"notes.txt","quote":{"exact":"two"},"replacement":"2","body":"Shorter."}"#,
    )
    .await;
    assert!(said.contains("Suggested the change") && said.contains("(t-1)"), "{said}");
    let suggested = threads(&mut client, &path).await;
    let suggestion = find(&suggested, "t-1").suggestion.clone().expect("a suggestion");
    assert_eq!(
        (suggestion.replacement.as_str(), suggestion.author_agent_id.as_str(), suggestion.decision()),
        ("2", "writer", v1::SuggestionDecision::Unspecified)
    );
    let before = view.entries.len();
    assert!(matches!(apply_request(&mut client, &mut view, decide("t-1", true)).await, Body::Ack(_)));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\n2\nthree\n");
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("accepted your suggested change t-1 on notes.txt") && prompt.contains("“two” → “2”"), "{prompt}");
    let accepted = threads(&mut client, &path).await;
    let t1 = find(&accepted, "t-1");
    assert_eq!(
        (t1.status(), t1.suggestion.as_ref().map(v1::Suggestion::decision)),
        (ThreadStatus::Resolved, Some(v1::SuggestionDecision::Accepted))
    );
    assert_eq!(t1.source.as_ref().map(|source| source.exact.as_str()), Some("2"), "on the new text");
    match client.request(decide("t-1", false)).await.body {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("decided once: {other:?}"),
    }

    // Text changed since: accepting is refused; rejecting leaves the file be.
    call(&mut client, &mut view, "writer", r#"call comments_suggest {"path":"notes.txt","quote":{"exact":"three"},"replacement":"3"}"#)
        .await;
    std::fs::write(&path, "one\n2\nfour\n").expect("write");
    match client.request(decide("t-2", true)).await.body {
        Body::Error(error) => assert_eq!(error.message, "The text changed after this was suggested."),
        other => panic!("expected an error, got {other:?}"),
    }
    let before = view.entries.len();
    // With the person's comment, as theirs.
    let reject = v1::DecideSuggestion { thread_id: "t-2".into(), accept: false, comment: "Keep the words.".into(), ..Default::default() };
    assert!(matches!(apply_request(&mut client, &mut view, client_message::Body::DecideSuggestion(reject)).await, Body::Ack(_)));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "one\n2\nfour\n");
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("rejected your suggested change t-2"), "{prompt}");
    assert!(prompt.contains("\n\nThe person's note: Keep the words."), "{prompt}");

    // The person's own suggestion: sent, then made by them.
    let mine = v1::CreateThread {
        agent_id: "writer".into(),
        path: path.clone(),
        kind: "text".into(),
        version_sha256: String::new(),
        selectors: vec![quote("one", "", "\n2")],
        body: String::new(),
        send_now: true,
        suggestion: Some("uno".into()),
        ink: None,
    };
    let before = view.entries.len();
    let Body::Thread(made) = apply_request(&mut client, &mut view, client_message::Body::CreateThread(mine)).await else {
        panic!("expected the thread")
    };
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("suggesting “uno” instead") && !prompt.contains("Reply:"), "a suggestion needs no words: {prompt}");
    let before = view.entries.len();
    assert!(matches!(apply_request(&mut client, &mut view, decide(&made.id, true)).await, Body::Ack(_)));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "uno\n2\nfour\n");
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("made the change they suggested") && prompt.contains("themselves"), "{prompt}");

    // Their own, never sent: decided, its comment is no draft to send.
    let unsent = v1::CreateThread {
        agent_id: "writer".into(),
        path: path.clone(),
        kind: "text".into(),
        version_sha256: String::new(),
        selectors: vec![quote("four", "2\n", "\n")],
        body: String::new(),
        send_now: false,
        suggestion: Some("4".into()),
        ink: None,
    };
    let Body::Thread(unsent) = apply_request(&mut client, &mut view, client_message::Body::CreateThread(unsent)).await else {
        panic!("expected the thread")
    };
    assert!(unsent.comments[0].draft, "a draft until decided");
    assert!(matches!(apply_request(&mut client, &mut view, decide(&unsent.id, true)).await, Body::Ack(_)));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "uno\n2\n4\n");
    let decided = threads(&mut client, &path).await;
    assert!(!find(&decided, &unsent.id).comments[0].draft, "kept with the thread, not sent");
    let send = v1::SendComments { agent_id: "writer".into(), path: path.clone(), ..Default::default() };
    match client.request(client_message::Body::SendComments(send)).await.body {
        Body::Error(error) => assert_eq!(error.message, "There are no comments to send."),
        other => panic!("expected an error, got {other:?}"),
    }

    // Submit: a comment instead of Accept or Reject is a reply on
    // the thread, sent at once; the suggestion stays open, the file as it is.
    call(&mut client, &mut view, "writer", r#"call comments_suggest {"path":"notes.txt","quote":{"exact":"uno"},"replacement":"one"}"#)
        .await;
    let listed = threads(&mut client, &path).await;
    let open = listed.threads.iter().rev().find(|thread| thread.suggestion.is_some()).expect("the suggestion").id.clone();
    let reply = |comment: &str| {
        client_message::Body::DecideSuggestion(v1::DecideSuggestion {
            thread_id: open.clone(),
            accept: true,
            comment: comment.into(),
            reply: true,
        })
    };
    match client.request(reply(" ")).await.body {
        Body::Error(error) => assert_eq!(error.message, "Write a reply, or accept or reject the change."),
        other => panic!("expected an error, got {other:?}"),
    }
    let before = view.entries.len();
    assert!(matches!(apply_request(&mut client, &mut view, reply("Say 'One', capitalized.")).await, Body::Ack(_)));
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "uno\n2\n4\n", "nothing changed in the file");
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("Say 'One', capitalized."), "{prompt}");
    let replied = threads(&mut client, &path).await;
    let thread = find(&replied, &open);
    assert_eq!(
        (thread.status(), thread.suggestion.as_ref().map(v1::Suggestion::decision)),
        (ThreadStatus::Open, Some(v1::SuggestionDecision::Unspecified))
    );
    let last = thread.comments.last().expect("the reply");
    assert!(last.body == "Say 'One', capitalized." && !last.draft, "{last:?}");
}

#[tokio::test]
async fn a_region_of_an_image_is_told_in_words_and_pixels() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
    png.extend_from_slice(&1280u32.to_be_bytes());
    png.extend_from_slice(&1080u32.to_be_bytes());
    png.extend_from_slice(b"pixels");
    std::fs::write(dir.join("site/hero.png"), &png).expect("write");
    let path = std::fs::canonicalize(dir.join("site/hero.png")).expect("path").display().to_string();
    let read = client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let region = serde_json::json!({"type": "FragmentSelector", "conformsTo": "http://www.w3.org/TR/media-frags/", "value": "xywh=percent:12.5,8,30,20"});
    let client_message::Body::CreateThread(mut made) =
        create(&path, "image", &content.sha256, vec![v1::Selector { json: region.to_string() }], "Brighter here.")
    else {
        unreachable!()
    };
    made.send_now = true;
    let before = view.entries.len();
    apply_request(&mut client, &mut view, client_message::Body::CreateThread(made)).await;
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    let words = "a region of the image, 30% wide and 20% tall, its top left 12.5% across and 8% down (that's 384 × 216 px at 160, 86 of 1280 × 1080)";
    assert!(prompt.contains(&format!("hero.png: {words}\nBrighter here.")), "{prompt}");
    let listed = call(&mut client, &mut view, "writer", r#"call comments_list {}"#).await;
    assert!(listed.contains(&format!("(open): {words}")), "{listed}");

    // The image changed: still the region, on an earlier version, without pixels.
    png.extend_from_slice(b"more");
    std::fs::write(dir.join("site/hero.png"), &png).expect("write");
    let listed = call(&mut client, &mut view, "writer", r#"call comments_list {}"#).await;
    assert!(
        listed.contains(
            "(open, the image changed since): a region of the image, 30% wide and 20% tall, its top left 12.5% across and 8% down\n"
        ),
        "{listed}"
    );
}

#[tokio::test]
async fn a_moment_of_a_video_is_told_in_words() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    turn_on_comments(&mut client, &mut view).await;
    // Bigger than text can be: only its hash is read.
    let mut video = b"\0\0\0\x18ftypmp42\0\0\0\0".to_vec();
    video.resize(3_000_000, 7);
    std::fs::write(dir.join("site/clip.mp4"), &video).expect("write");
    let path = std::fs::canonicalize(dir.join("site/clip.mp4")).expect("path").display().to_string();
    let read =
        client.request(client_message::Body::ReadFile(v1::ReadFile { path: path.clone(), max_bytes: 0, ..Default::default() })).await;
    let Body::FileContent(content) = read.body else { panic!("expected the file") };
    let moment = |value: &str| v1::Selector {
        json: serde_json::json!({"type": "FragmentSelector", "conformsTo": "http://www.w3.org/TR/media-frags/", "value": value})
            .to_string(),
    };
    let send = |version: &str, value: &str, body: &str| {
        let client_message::Body::CreateThread(mut made) = create(&path, "video", version, vec![moment(value)], body) else {
            unreachable!()
        };
        made.send_now = true;
        client_message::Body::CreateThread(made)
    };
    let before = view.entries.len();
    apply_request(&mut client, &mut view, send(&content.sha256, "t=31.2,45&xywh=percent:10,20,30,25", "Cut this.")).await;
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    let words = "at 0:31–0:45, in a region of the frame, 30% wide and 25% tall, its top left 10% across and 20% down";
    assert!(prompt.contains(&format!("clip.mp4: {words}\nCut this.")), "{prompt}");
    match client.request(create(&path, "video", &content.sha256, Vec::new(), "Where?")).await.body {
        Body::Error(error) => assert_eq!(error.message, "A video's comment needs a time."),
        other => panic!("expected an error, got {other:?}"),
    }

    // Made on an earlier version, or the video changed since: it says so.
    let before = view.entries.len();
    apply_request(&mut client, &mut view, send("an-older-version", "t=75", "And this.")).await;
    let prompt = last_prompt(&mut client, &mut view, &requests, before).await;
    assert!(prompt.contains("clip.mp4: at 1:15 (on an earlier version of the video)\nAnd this."), "{prompt}");
    // Shown in a pane, it's checked: a new version makes its comments outdated.
    let leaf = |id: &str, kind: &str, path: &str| v1::ViewNode {
        id: id.into(),
        node: Some(v1::view_node::Node::Leaf(v1::ViewLeaf {
            kind: kind.into(),
            path: path.into(),
            title: String::new(),
            ..Default::default()
        })),
    };
    let split = v1::ViewSplit {
        axis: v1::ViewAxis::Row.into(),
        children: vec![leaf("chat", "chat", ""), leaf("pane-1", "video", &path)],
        weights: Vec::new(),
    };
    let root = v1::ViewNode { id: "split".into(), node: Some(v1::view_node::Node::Split(split)) };
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::SetLayout(v1::SetLayout { agent_id: "writer".into(), root: Some(root), ..Default::default() }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    video.extend_from_slice(b"more");
    std::fs::write(dir.join("site/clip.mp4"), &video).expect("write");
    let changed = until(&mut client, &mut view, |body| match body {
        Body::FileChanged(changed) if changed.path == path => Some(changed.clone()),
        _ => None,
    })
    .await;
    assert!(changed.sha256 != content.sha256 && changed.by_agent_id.is_empty(), "{changed:?}");
    let outdated = threads(&mut client, &path).await;
    assert!(outdated.threads.iter().all(|thread| thread.anchor() == AnchorState::Outdated), "{outdated:?}");
    let listed = call(&mut client, &mut view, "writer", r#"call comments_list {}"#).await;
    assert!(listed.contains(&format!("(open, the video changed since): {words}\n")), "{listed}");

    // Made again just as it was (or caught mid-write): back on its version.
    video.truncate(3_000_000);
    std::fs::write(dir.join("site/clip.mp4"), &video).expect("write");
    until(&mut client, &mut view, |body| match body {
        Body::FileChanged(changed) if changed.path == path && changed.sha256 == content.sha256 => Some(()),
        _ => None,
    })
    .await;
    let back = threads(&mut client, &path).await;
    let anchors: Vec<AnchorState> = back.threads.iter().map(v1::Thread::anchor).collect();
    assert_eq!(anchors, [AnchorState::Anchored, AnchorState::Outdated], "the one made on an earlier version stays so");
}

/// A Markdown file shown in a pane is watched with the pictures it
/// shows in place, so a picture that changes while the file doesn't is read
/// again (FileChanged for the picture's own path).
#[tokio::test]
async fn a_picture_a_shown_markdown_file_shows_is_watched_with_it() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Writer", "")).await;
    let png = |tail: &[u8]| [&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A][..], tail].concat();
    std::fs::create_dir_all(dir.join("site/img")).expect("folder");
    std::fs::write(dir.join("site/img/shot.png"), png(b"one")).expect("write");
    std::fs::write(dir.join("site/notes.md"), "# Notes\n\nSee ![shot](img/shot.png) here.\n").expect("write");
    // As the app names them: on Windows with `\`, and without canonicalize's `\\?\`.
    #[cfg(unix)]
    let site = std::fs::canonicalize(dir.join("site")).expect("path");
    #[cfg(windows)]
    let site = factoryd::windows::fs::canonical(&dir.join("site")).expect("path");
    let notes = site.join("notes.md").display().to_string();
    let picture = site.join("img").join("shot.png").display().to_string();
    let leaf = |id: &str, kind: &str, path: &str| v1::ViewNode {
        id: id.into(),
        node: Some(v1::view_node::Node::Leaf(v1::ViewLeaf { kind: kind.into(), path: path.into(), ..Default::default() })),
    };
    let split = v1::ViewSplit {
        axis: v1::ViewAxis::Row.into(),
        children: vec![leaf("chat", "chat", ""), leaf("pane-1", "text", &notes)],
        weights: Vec::new(),
    };
    let root = v1::ViewNode { id: "split".into(), node: Some(v1::view_node::Node::Split(split)) };
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::SetLayout(v1::SetLayout { agent_id: "writer".into(), root: Some(root), ..Default::default() }),
    )
    .await;
    tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
    std::fs::write(dir.join("site/img/shot.png"), png(b"two, bigger")).expect("write");
    let changed = until(&mut client, &mut view, |body| match body {
        Body::FileChanged(changed) if changed.path == picture => Some(changed.clone()),
        Body::FileChanged(changed) if changed.path == notes => panic!("the file itself didn't change: {changed:?}"),
        _ => None,
    })
    .await;
    assert!(!changed.sha256.is_empty() && changed.by_agent_id.is_empty(), "{changed:?}");
}
