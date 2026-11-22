//! Asks (#208): an agent blocked on something only the person can give asks
//! them with ask_person, a card the person answers or dismisses; the agent
//! is told which. Pending asks wait across restarts, at most 3 at once.

use crate::common::{Client, View, apply_request, create_agent, create_project, send_message, start, temp_dir};
use factory_protocol::v1::{self, AgentStatus, AskPersonState, client_message, entry::Kind, server_message::Body};

fn select(agent: &str) -> client_message::Body {
    client_message::Body::SelectAgent(v1::SelectAgent { agent_id: agent.into() })
}

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == AgentStatus::Idle && agent.queued_messages == 0
}

fn answer(seq: u64, dismiss: bool, text: &str) -> client_message::Body {
    client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, dismiss, text: text.into(), ..Default::default() })
}

/// A Factory with a project "Site" and its agent Builder.
async fn setup(dir: &std::path::Path) -> (std::sync::Arc<factoryd::Factory>, Client, View) {
    let (addr, factory) = start(dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    apply_request(&mut client, &mut view, create_agent("site", "Builder", "")).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    (factory, client, view)
}

/// Builder asks; returns what the tool said and the card it shows (with its
/// seq), if any.
async fn ask(client: &mut Client, view: &mut View, title: &str, wants_text: bool) -> (String, Option<(u64, v1::AskPerson)>) {
    let before = view.entries.len();
    let args = serde_json::json!({"title": title, "body": "The release is **ready**; only you can say when.", "wants_text": wants_text});
    apply_request(client, view, send_message("builder", &format!("call ask_person {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    let new = &view.entries[before..];
    let output = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let card = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
        _ => None,
    });
    (output.expect("a tool call"), card)
}

fn card(view: &View, seq: u64) -> v1::AskPerson {
    view.entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::AskPerson(ask)) if entry.seq == seq => Some(ask.clone()),
            _ => None,
        })
        .expect("the card")
}

/// The text of the last event from Claktory, once Builder has run on it.
async fn told(client: &mut Client, view: &mut View, count: usize) -> String {
    let told = |view: &View| {
        view.entries
            .iter()
            .filter_map(|entry| match &entry.kind {
                Some(Kind::Event(event)) if event.source() == v1::EventSource::Factory => Some(event.text.clone()),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    view.follow(client, |view| told(view).len() >= count && idle(view, "builder")).await;
    told(view).pop().expect("an event")
}

#[tokio::test]
async fn an_ask_waits_for_the_person_who_answers_it() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let (said, shown) = ask(&mut client, &mut view, "Which color for the launch page?", true).await;
    assert!(said.starts_with("Asked the person: they see it as a card"), "{said}");
    let (seq, shown) = shown.expect("a card after the call");
    assert_eq!(
        (shown.title.as_str(), shown.wants_text, shown.state()),
        ("Which color for the launch page?", true, AskPersonState::Pending)
    );
    assert!(shown.body.contains("**ready**"));
    // It waits for the person, as every card does.
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);

    // A written answer is needed when it wants one.
    let empty = apply_request(&mut client, &mut view, answer(seq, false, " ")).await;
    let Body::Error(error) = empty else { panic!("{empty:?}") };
    assert_eq!(error.message, "Write an answer, or dismiss the question.");
    let reply = apply_request(&mut client, &mut view, answer(seq, false, "Blue, the dark one.")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let decided = card(&view, seq);
    assert_eq!((decided.state(), decided.answer.as_str()), (AskPersonState::Answered, "Blue, the dark one."));
    assert!(decided.decided_ms > 0);
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.starts_with("The person answered your question \"Which color for the launch page?\":\n\nBlue, the dark one."), "{text}");
    assert!(view.agent("builder").waiting_seqs.is_empty());

    // Answered once.
    let again = apply_request(&mut client, &mut view, answer(seq, true, "")).await;
    let Body::Error(error) = again else { panic!("{again:?}") };
    assert_eq!(error.message, "This question was already answered.");
    let nothing = apply_request(&mut client, &mut view, answer(1, false, "x")).await;
    assert!(matches!(nothing, Body::Error(_)), "{nothing:?}");
}

#[tokio::test]
async fn a_dismissed_ask_tells_the_agent_to_use_its_judgment_and_a_yes_needs_no_words() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    let (_, first) = ask(&mut client, &mut view, "Deploy to production tonight?", false).await;
    let (seq, _) = first.expect("a card");
    apply_request(&mut client, &mut view, answer(seq, true, "")).await;
    assert_eq!(card(&view, seq).state(), AskPersonState::Dismissed);
    let text = told(&mut client, &mut view, 1).await;
    assert!(text.starts_with("The person dismissed your question \"Deploy to production tonight?\""), "{text}");
    assert!(text.contains("Use your best judgment"), "{text}");

    let (_, second) = ask(&mut client, &mut view, "Delete the old bucket?", false).await;
    let (seq, _) = second.expect("a card");
    let reply = apply_request(&mut client, &mut view, answer(seq, false, "")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let text = told(&mut client, &mut view, 2).await;
    assert!(text.starts_with("The person answered your question \"Delete the old bucket?\": yes, go ahead."), "{text}");
}

#[tokio::test]
async fn at_most_three_wait_and_they_survive_a_restart() {
    let dir = temp_dir();
    let (factory, mut client, mut view) = setup(&dir).await;
    let mut seqs = Vec::new();
    for title in ["One?", "Two?", "Three?"] {
        let (_, shown) = ask(&mut client, &mut view, title, true).await;
        seqs.push(shown.expect("a card").0);
    }
    let (said, shown) = ask(&mut client, &mut view, "Four?", true).await;
    assert!(shown.is_none());
    assert!(said.contains("You already have 3 questions waiting for the person"), "{said}");
    assert_eq!(view.agent("builder").waiting_seqs, seqs);
    apply_request(&mut client, &mut view, answer(seqs[0], false, "Yes.")).await;
    told(&mut client, &mut view, 1).await;
    // Room for one more now.
    let (_, shown) = ask(&mut client, &mut view, "Four?", true).await;
    seqs.push(shown.expect("a card").0);
    drop(client);
    drop(factory);

    // After a restart: decided ones show decided, pending ones still wait,
    // without the conversation open, and can be answered.
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs == seqs[1..]).await;
    apply_request(&mut client, &mut view, select("builder")).await;
    let first = card(&view, seqs[0]);
    assert_eq!((first.state(), first.answer.as_str()), (AskPersonState::Answered, "Yes."));
    assert_eq!(card(&view, seqs[1]).state(), AskPersonState::Pending);
    let reply = apply_request(&mut client, &mut view, answer(seqs[1], false, "No.")).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    assert_eq!(view.agent("builder").waiting_seqs, seqs[2..]);

    // /clear dismisses what waits, so new questions have room.
    let clear = v1::RunCommand { agent_id: "builder".into(), name: "clear".into(), args: String::new() };
    apply_request(&mut client, &mut view, client_message::Body::RunCommand(clear)).await;
    view.follow(&mut client, |view| view.agent("builder").waiting_seqs.is_empty()).await;
    for title in ["A?", "B?", "C?"] {
        let (_, shown) = ask(&mut client, &mut view, title, true).await;
        assert!(shown.is_some(), "{title}");
    }
}

/// Builder calls ask_person with `args`; what the tool said, and the card.
async fn ask_with(client: &mut Client, view: &mut View, args: serde_json::Value) -> (String, Option<(u64, v1::AskPerson)>) {
    let before = view.entries.len();
    apply_request(client, view, send_message("builder", &format!("call ask_person {args}"))).await;
    view.follow(client, |view| view.entries.len() >= before + 3 && idle(view, "builder")).await;
    let new = &view.entries[before..];
    let output = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::ToolCall(call)) => Some(call.output.clone()),
        _ => None,
    });
    let card = new.iter().find_map(|entry| match &entry.kind {
        Some(Kind::AskPerson(ask)) => Some((entry.seq, ask.clone())),
        _ => None,
    });
    (output.expect("a tool call"), card)
}

fn comment(path: &str, exact: &str, body: &str, send_now: bool) -> client_message::Body {
    let quote = serde_json::json!({"type": "TextQuoteSelector", "exact": exact, "prefix": "", "suffix": ""}).to_string();
    client_message::Body::CreateThread(v1::CreateThread {
        agent_id: "builder".into(),
        path: path.into(),
        kind: "markdown".into(),
        version_sha256: String::new(),
        selectors: vec![v1::Selector { json: quote }],
        body: body.into(),
        send_now,
        suggestion: None,
        ink: None,
    })
}

#[tokio::test]
async fn a_review_ask_names_a_file_it_can_read_and_is_told_how_many_comments_the_person_left() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    std::fs::create_dir_all(dir.join("site")).expect("site");
    std::fs::write(dir.join("site/plan.md"), "# Plan\n\nShip on Monday.\n\nThen tell everyone.\n").expect("plan");
    std::fs::write(dir.join("site/blob.bin"), [0u8, 1, 2, 3, 0, 9]).expect("blob");
    std::fs::write(dir.join("elsewhere.md"), "# Not yours\n").expect("elsewhere");

    // Only a file it can read, and one comments work on.
    let refused = |said: &str, card: &Option<(u64, v1::AskPerson)>| said.ends_with("Nothing was asked.") && card.is_none();
    let args = |review: &str| serde_json::json!({"title": "Review the plan", "body": "Is Monday right?", "review": review});
    let (said, made) = ask_with(&mut client, &mut view, args("missing.md")).await;
    assert!(refused(&said, &made) && said.starts_with("There's no file"), "{said}");
    let outside = dir.join("elsewhere.md").display().to_string();
    let (said, made) = ask_with(&mut client, &mut view, args(&outside)).await;
    assert!(refused(&said, &made) && said.contains("isn't in your folders"), "{said}");
    let (said, made) = ask_with(&mut client, &mut view, args("blob.bin")).await;
    assert!(refused(&said, &made) && said.contains("isn't a file comments work on"), "{said}");

    // Relative to its folder; the card names it in full.
    let (said, made) = ask_with(&mut client, &mut view, args("plan.md")).await;
    let path = std::fs::canonicalize(dir.join("site/plan.md")).expect("real").display().to_string();
    assert!(said.starts_with(&format!("Asked the person to review {path}")), "{said}");
    let (seq, shown) = made.expect("a card");
    assert_eq!((shown.review_path.as_str(), shown.state()), (path.as_str(), AskPersonState::Pending));
    assert_eq!(view.agent("builder").waiting_seqs, [seq]);

    // The person comments on it, one comment sent at once and one not yet;
    // approving sends the one left, and says how many.
    let sent = apply_request(&mut client, &mut view, comment(&path, "Ship on Monday.", "Tuesday, after the review.", true)).await;
    assert!(matches!(sent, Body::Thread(_)), "{sent:?}");
    let draft = apply_request(&mut client, &mut view, comment(&path, "Then tell everyone.", "Only the team first.", false)).await;
    assert!(matches!(draft, Body::Thread(_)), "{draft:?}");
    assert!(matches!(apply_request(&mut client, &mut view, answer(seq, false, "")).await, Body::Ack(_)));
    assert_eq!(card(&view, seq).review_path, path);
    let answered = |view: &View| {
        view.entries.iter().find_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) if event.text.contains("answered your question \"Review the plan\"") => Some(event.text.clone()),
            _ => None,
        })
    };
    view.follow(&mut client, |view| answered(view).is_some() && idle(view, "builder")).await;
    let text = answered(&view).expect("told");
    assert!(text.contains(&format!("They left 2 comments on {path}; they came to you as comments")), "{text}");
    let unsent =
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::Event(event)) if event.text.contains("Only the team first.")));
    assert!(unsent, "the comment not sent yet came to Builder");

    // A review ask with no comments says so.
    let (_, made) =
        ask_with(&mut client, &mut view, serde_json::json!({"title": "Look again", "body": "Anything else?", "review": path})).await;
    let (seq, _) = made.expect("a card");
    apply_request(&mut client, &mut view, answer(seq, true, "")).await;
    let again = told(&mut client, &mut view, 1).await;
    let dismissed = view.entries.iter().rev().find_map(|entry| match &entry.kind {
        Some(Kind::Event(event)) if event.text.contains("dismissed your question \"Look again\"") => Some(event.text.clone()),
        _ => None,
    });
    let dismissed = dismissed.unwrap_or(again);
    assert!(dismissed.contains(&format!("They left no comments on {path}.")), "{dismissed}");
}

#[tokio::test]
async fn a_pdf_review_is_answered_with_drawings_of_its_pages_which_the_agent_is_told_of() {
    let dir = temp_dir();
    let (_factory, mut client, mut view) = setup(&dir).await;
    std::fs::create_dir_all(dir.join("site")).expect("site");
    std::fs::write(dir.join("site/paper.pdf"), b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n1 0 obj\n<<>>\nendobj\n%%EOF\n").expect("pdf");

    // A PDF is a file to review.
    let args = serde_json::json!({"title": "Mark up the paper", "body": "Circle what's wrong.", "review": "paper.pdf"});
    let (said, made) = ask_with(&mut client, &mut view, args.clone()).await;
    let path = std::fs::canonicalize(dir.join("site/paper.pdf")).expect("real").display().to_string();
    assert!(said.starts_with(&format!("Asked the person to review {path}")), "{said}");
    let (seq, _) = made.expect("a card");

    // Drawings are the agent's uploads, or the answer is refused.
    let uploads = dir.join("home/agents/builder/uploads/2026-11-22");
    std::fs::create_dir_all(&uploads).expect("uploads");
    let page = |name: &str| {
        let path = uploads.join(name);
        std::fs::write(&path, b"\x89PNG\r\n\x1a\n").expect("png");
        v1::Attachment { path: path.display().to_string(), name: name.into(), size: 8, mime: "image/png".into() }
    };
    let (third, seventh) = (page("paper-p3.png"), page("paper-p7.png"));
    std::fs::write(dir.join("stray.png"), b"\x89PNG\r\n\x1a\n").expect("stray");
    let stray = v1::Attachment { path: dir.join("stray.png").display().to_string(), name: "stray.png".into(), ..Default::default() };
    let drawn = |attachments: Vec<v1::Attachment>, dismiss: bool| {
        client_message::Body::AnswerAsk(v1::AnswerAsk { agent_id: "builder".into(), seq, dismiss, text: String::new(), attachments })
    };
    let refused = apply_request(&mut client, &mut view, drawn(vec![third.clone(), stray], false)).await;
    let Body::Error(error) = refused else { panic!("{refused:?}") };
    assert!(error.message.ends_with("isn't one of this agent's uploads."), "{}", error.message);
    assert_eq!(card(&view, seq).state(), AskPersonState::Pending, "nothing was decided");

    // Answered with two pages: the card keeps them, and the agent is told
    // which pages, by path.
    let reply = apply_request(&mut client, &mut view, drawn(vec![third.clone(), seventh.clone()], false)).await;
    assert!(matches!(reply, Body::Ack(_)), "{reply:?}");
    let decided = card(&view, seq);
    let kept: Vec<&str> = decided.answer_attachments.iter().map(|attachment| attachment.name.as_str()).collect();
    assert_eq!((decided.state(), kept), (AskPersonState::Answered, vec!["paper-p3.png", "paper-p7.png"]));
    let text = told(&mut client, &mut view, 1).await;
    assert!(
        text.contains(&format!(
            "They left no comments on {path}.\n\nThe person drew on 2 pages (pages 3 and 7); look at them with read_image: {}, {}",
            third.path, seventh.path
        )),
        "{text}"
    );

    // Dismissed with one: the same line after the dismissal's.
    let (_, made) = ask_with(&mut client, &mut view, args).await;
    let (again, _) = made.expect("a card");
    let one = client_message::Body::AnswerAsk(v1::AnswerAsk {
        agent_id: "builder".into(),
        seq: again,
        dismiss: true,
        text: String::new(),
        attachments: vec![third.clone()],
    });
    assert!(matches!(apply_request(&mut client, &mut view, one).await, Body::Ack(_)));
    let text = told(&mut client, &mut view, 2).await;
    assert!(text.starts_with("The person dismissed your question \"Mark up the paper\""), "{text}");
    assert!(text.contains(&format!("The person drew on 1 page (page 3); look at it with read_image: {}", third.path)), "{text}");
}
