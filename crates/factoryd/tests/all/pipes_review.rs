//! Review incoming: every message from the other side waits for the
//! person's Let in before any agent reads anything of it (the lead's
//! event, pipe_read, pipe_status, the files), whoever the other side
//! says wrote it.

use crate::common::apply_request;
use crate::pipes::{call, events, file_in, inbox, incoming, setup, shown};
use factory_protocol::v1::{self, client_message};
use factoryd::pipe_link::Via;
use factoryd::pipes;

const HELD_NOTE: &str = "Waiting for you: let it in, or discard it.";

fn review_incoming(pipe: &str, on: bool) -> client_message::Body {
    client_message::Body::UpdatePipe(v1::UpdatePipe { pipe_id: pipe.into(), review_incoming: Some(on), ..Default::default() })
}

fn state_of(factory: &factoryd::factory::Factory, pipe: &str, text: &str) -> (u64, v1::PipeMessageState, String) {
    let message = pipes::messages_for_test(factory, pipe).into_iter().find(|message| message.text == text).expect("the message");
    (message.seq, message.state(), message.note)
}

#[tokio::test]
async fn review_incoming_holds_every_message_until_it_is_let_in() {
    let (dir, factory, mut client, mut view, pipe) = setup(false).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| !pipe.review_incoming)).await;
    apply_request(&mut client, &mut view, review_incoming(&pipe, true)).await;
    view.follow(&mut client, |view| shown(view, &pipe).is_some_and(|pipe| pipe.review_incoming)).await;
    assert!(pipes::prompt_for_test(&factory, "lea").expect("a section").contains("Review incoming is on"));

    // Their person's word can't skip it, nor can a file reach the inbox.
    let mut first = incoming("m1", "FIRST-SECRET");
    first.via = Via::Person { name: "Ana-SECRET-NAME".into() };
    first.files = vec![file_in(&dir, "brief-secret.txt", 10)];
    pipes::receive_for_test(&factory, &pipe, first).expect("received");
    let mut second = incoming("m2", "SECOND-SECRET");
    second.via = Via::Agent { name: "Mallory".into(), role: "Injector".into() };
    pipes::receive_for_test(&factory, &pipe, second).expect("received");
    let (first_seq, state, note) = state_of(&factory, &pipe, "FIRST-SECRET");
    assert_eq!((state, note.as_str()), (v1::PipeMessageState::HeldIncoming, HELD_NOTE));
    let (second_seq, state, _) = state_of(&factory, &pipe, "SECOND-SECRET");
    assert_eq!(state, v1::PipeMessageState::HeldIncoming);
    assert!(!inbox(&dir).join("brief-secret.txt").exists());

    // The lead hears nothing of them; what it can ask shows counts only.
    let secrets = ["FIRST-SECRET", "SECOND-SECRET", "Ana-SECRET-NAME", "Mallory", "Injector", "brief-secret"];
    let status = call(&mut client, &mut view, "lea", "pipe_status", "{}").await;
    assert!(status.contains("2 messages from the other side wait for the person."), "{status}");
    assert!(status.contains("Review incoming: on."), "{status}");
    let read = call(&mut client, &mut view, "lea", "pipe_read", "{}").await;
    assert!(read.contains(&format!("#{first_seq} A message from the other side waits for the person.")), "{read}");
    for text in [status.as_str(), read.as_str()] {
        assert!(!secrets.iter().any(|secret| text.contains(secret)), "{text}");
    }
    assert!(events(&view).is_empty(), "the lead was told: {:?}", events(&view));

    // Let in, the later one first: the lead hears each, the earlier one too.
    for seq in [second_seq, first_seq] {
        let allow = v1::DecidePipeMessage { pipe_id: pipe.clone(), seq, allow: true, ..Default::default() };
        apply_request(&mut client, &mut view, client_message::Body::DecidePipeMessage(allow)).await;
    }
    view.follow(&mut client, |view| {
        let told: String = events(view).iter().map(|event| event.text.clone()).collect();
        told.contains("FIRST-SECRET") && told.contains("SECOND-SECRET")
    })
    .await;
    assert!(inbox(&dir).join("brief-secret.txt").exists());

    // Turned off, what waits still waits; what comes next goes in.
    pipes::receive_for_test(&factory, &pipe, incoming("m3", "THIRD")).expect("received");
    apply_request(&mut client, &mut view, review_incoming(&pipe, false)).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m4", "FOURTH")).expect("received");
    assert_eq!(state_of(&factory, &pipe, "THIRD").1, v1::PipeMessageState::HeldIncoming);
    assert_eq!(state_of(&factory, &pipe, "FOURTH").1, v1::PipeMessageState::Received);
    let lines: Vec<String> = pipes::messages_for_test(&factory, &pipe)
        .into_iter()
        .filter(|message| message.author.as_ref().is_some_and(|author| author.kind() == v1::PipeAuthorKind::Claktory))
        .map(|message| message.text)
        .collect();
    assert!(lines.iter().any(|line| line.starts_with("Review incoming is on:")), "{lines:?}");
    assert!(lines.iter().any(|line| line.starts_with("Review incoming is off:")), "{lines:?}");
}

// Held for review counts against the 20 that may wait, like the flood.
#[tokio::test]
async fn at_most_twenty_wait_for_review() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    apply_request(&mut client, &mut view, review_incoming(&pipe, true)).await;
    let limits = v1::PipeLimits { messages_per_hour: 1000, burst: 1000, file_mb_per_day: 10, ..Default::default() };
    let update = v1::UpdatePipe { pipe_id: pipe.clone(), limits: Some(limits), ..Default::default() };
    apply_request(&mut client, &mut view, client_message::Body::UpdatePipe(update)).await;
    let mut refused = None;
    for number in 1..=21 {
        if number % 8 == 0 {
            // The person writes now and then, so it never pauses as a loop.
            let say = v1::SendPipeMessage { pipe_id: pipe.clone(), text: format!("Noted {number}") };
            apply_request(&mut client, &mut view, client_message::Body::SendPipeMessage(say)).await;
        }
        let mut message = incoming(&format!("m{number}"), &format!("Message {number}"));
        message.via = Via::Person { name: "Ana".into() };
        refused = pipes::receive_for_test(&factory, &pipe, message).err();
        if number <= 20 {
            assert!(refused.is_none(), "{number}: {refused:?}");
        }
    }
    assert_eq!(refused.as_deref(), Some("Too many messages already wait for this side's person."));
}
