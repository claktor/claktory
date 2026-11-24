//! A pipe's liaison waits (#343 U18, Fugo's ruling): this side tells the
//! other its lead waits, and until when (a quarter of an hour), never why,
//! when it changes and at most once a minute; what the other side says
//! shows to the person, in this side's words, and reaches no agent.

use std::time::Duration;

use factoryd::pipe_link::{self, Liaison};
use factoryd::pipes;
use factoryd::transcript::now_ms;

use crate::pipes::{call, idle, pipe_of, setup};

const QUARTER_MS: i64 = 15 * 60 * 1000;

/// What the pipe's lines say, the latest last.
fn lines(factory: &factoryd::Factory, pipe: &str) -> Vec<String> {
    pipes::messages_for_test(factory, pipe)
        .into_iter()
        .filter(|message| message.author.as_ref().is_some_and(|author| author.kind() == factory_protocol::v1::PipeAuthorKind::Claktory))
        .map(|message| message.text)
        .collect()
}

#[tokio::test]
async fn the_other_sides_wait_shows_to_the_person_in_our_words_and_reaches_no_agent() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    let before = lines(&factory, &pipe).len();

    // Waiting until Friday evening: the pipe says so, with its own name.
    let until = now_ms() + 3 * 3_600_000;
    pipes::peer_liaison_for_test(&factory, &pipe, Liaison::Waiting { until_ms: until });
    view.follow(&mut client, |view| view.workspace.pipes.iter().any(|shown| shown.id == pipe && shown.peer_waiting)).await;
    assert_eq!(pipe_of(&view, &pipe).peer_waiting_until_ms, until);
    let at = factory_agent::accounts::local_time(u64::try_from(until).expect("a time")).expect("local time");
    let said = lines(&factory, &pipe);
    assert_eq!(said[before..], [format!("Website ⇄ Marketing is waiting until {at}.")]);

    // The same again: nothing new.
    pipes::peer_liaison_for_test(&factory, &pipe, Liaison::Waiting { until_ms: until });
    assert_eq!(lines(&factory, &pipe).len(), before + 1);

    // No end known: without a time.
    pipes::peer_liaison_for_test(&factory, &pipe, Liaison::Waiting { until_ms: 0 });
    view.follow(&mut client, |view| view.workspace.pipes.iter().any(|shown| shown.id == pipe && shown.peer_waiting_until_ms == 0)).await;
    assert_eq!(lines(&factory, &pipe).last().map(String::as_str), Some("Website ⇄ Marketing is waiting."));

    // No agent is told, and none reads it.
    assert!(idle(&view, "lea") && idle(&view, "bo"));
    assert!(crate::pipes::events(&view).is_empty());
    let read = call(&mut client, &mut view, "lea", "pipe_read", "{}").await;
    assert!(!read.contains("waiting"), "{read}");

    // Awake.
    pipes::peer_liaison_for_test(&factory, &pipe, Liaison::Awake);
    view.follow(&mut client, |view| view.workspace.pipes.iter().any(|shown| shown.id == pipe && !shown.peer_waiting)).await;
    assert_eq!(lines(&factory, &pipe).last().map(String::as_str), Some("Website ⇄ Marketing isn't waiting any more."));
}

#[tokio::test]
async fn our_liaisons_wait_is_told_by_its_time_alone_when_it_changes_at_most_once_a_minute() {
    pipe_link::set_liaison_every(Duration::from_millis(600));
    let (_dir, factory, _client, _view, pipe) = setup(false).await;

    // Awake, and nothing told yet: nothing to tell.
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), None);

    // Its lead waits on a limit: until then, rounded up to a quarter hour.
    let until = now_ms() + 3_600_000 + 7 * 60_000 + 1;
    pipes::set_usage_wait_for_test(&factory, "lea", Some(until));
    let Some(Liaison::Waiting { until_ms }) = pipes::liaison_due_for_test(&factory, &pipe) else { panic!("a wait to tell") };
    assert!(until_ms >= until && until_ms - until < QUARTER_MS && until_ms % QUARTER_MS == 0, "{until_ms} for {until}");

    // Out of credit: no end known, no time.
    pipes::set_usage_wait_for_test(&factory, "lea", Some(0));
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), Some(Liaison::Waiting { until_ms: 0 }));
    pipes::liaison_told_for_test(&factory, &pipe, Liaison::Waiting { until_ms: 0 });
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), None, "told already");

    // Awake again: not within the minute, then.
    pipes::set_usage_wait_for_test(&factory, "lea", None);
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), None, "within the minute");
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), Some(Liaison::Awake));

    // Another agent's wait isn't the liaison's.
    pipes::liaison_told_for_test(&factory, &pipe, Liaison::Awake);
    pipes::set_usage_wait_for_test(&factory, "bo", Some(until));
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(pipes::liaison_due_for_test(&factory, &pipe), None);
}
