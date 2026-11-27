use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use crate::support::{collect, event_names, luna, reply};
use factory_agent::stream::event_stream;
use factory_agent::{
    Agent, AgentEvent, AgentOptions, AssistantMessage, AssistantPart, Message, StopReason, StreamEvent, StreamFn, ThinkingLevel,
};
use parking_lot::Mutex;

/// Streams partial text, then fails with `failures[n]` on the n-th request while there is one, and succeeds after.
/// Records how many messages each request carried.
fn flaky(failures: &'static [&'static str]) -> (StreamFn, Arc<Mutex<Vec<usize>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let count = AtomicUsize::new(0);
    let stream: StreamFn = Arc::new(move |model, context, _options| {
        seen.lock().push(context.messages.len());
        let attempt = count.fetch_add(1, Ordering::SeqCst);
        let mut message = reply(model, vec![AssistantPart::Text { text: format!("attempt {attempt}"), text_signature: None }]);
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Start { partial: message.clone() });
        match failures.get(attempt) {
            Some(failure) => {
                message.stop_reason = StopReason::Error;
                message.error_message = Some((*failure).into());
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
            }
            None => sink.push(StreamEvent::Done { reason: StopReason::Stop, message }),
        }
        Ok(stream)
    });
    (stream, requests)
}

fn agent(stream: StreamFn) -> Agent {
    Agent::new(options(stream))
}

fn options(stream: StreamFn) -> AgentOptions {
    AgentOptions {
        system_prompt: "You are a coding agent.".into(),
        model: luna(),
        thinking_level: ThinkingLevel::Off,
        tools: Vec::new(),
        messages: Vec::new(),
        stream,
        session_id: None,
        transform_context: None,
        should_stop_after_turn: None,
        before_tool_call: None,
        compact_at: None,
        inbox: None,
        after_compaction: None,
        secrets: None,
        fallback_model: None,
    }
}

fn retries(events: &[AgentEvent]) -> Vec<(u32, u64)> {
    let mut found = Vec::new();
    for event in events {
        if let AgentEvent::Retrying { attempt, delay_ms, .. } = event {
            found.push((*attempt, *delay_ms));
        }
    }
    found
}

fn assistants(messages: &[Message]) -> Vec<&AssistantMessage> {
    let mut found = Vec::new();
    for message in messages {
        if let Message::Assistant(assistant) = message {
            found.push(assistant);
        }
    }
    found
}

#[tokio::test(start_paused = true)]
async fn a_transient_failure_is_retried_with_backoff_and_leaves_no_trace_in_the_context() {
    let (stream, requests) = flaky(&["503 Service Unavailable", "Response guard: no response events for 120000ms"]);
    let mut agent = agent(stream);
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    // Every attempt is sent the same context: the failed ones never enter it.
    assert_eq!(*requests.lock(), [1, 1, 1]);
    let events = events.lock().clone();
    assert_eq!(retries(&events), [(1, 1000), (2, 2000)]);
    let names = event_names(&events);
    assert_eq!(names.iter().filter(|name| *name == "retrying").count(), 2);
    assert_eq!(names.iter().filter(|name| *name == "message_start").count(), 4);
    assert_eq!(names.iter().filter(|name| *name == "message_end").count(), 2);
    let [answer] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(
        (answer.stop_reason, answer.content.clone()),
        (StopReason::Stop, vec![AssistantPart::Text { text: "attempt 2".into(), text_signature: None }])
    );
    assert_eq!(agent.state.error_message, None);
}

#[tokio::test(start_paused = true)]
async fn retries_stop_after_two_and_terminal_failures_are_not_retried() {
    let cases: [(&'static [&'static str], usize, &str); 2] = [
        (&["overloaded", "overloaded", "overloaded", "overloaded"], 3, "overloaded"),
        (&["401 invalid api key"], 1, "401 invalid api key"),
    ];
    for (failures, attempts, error) in cases {
        let (stream, requests) = flaky(failures);
        let mut agent = agent(stream);
        agent.prompt("hello").await.unwrap();
        assert_eq!(requests.lock().len(), attempts, "{error}");
        assert_eq!(agent.state.error_message.as_deref(), Some(error));
        let [failure] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
        assert_eq!(failure.stop_reason, StopReason::Error);
    }
}

#[tokio::test(start_paused = true)]
async fn aborting_during_the_backoff_ends_the_run_as_aborted() {
    let (stream, requests) = flaky(&["503", "503"]);
    let mut agent = agent(stream);
    let abort = agent.abort_handle();
    let events = collect(&mut agent);
    let run = tokio::spawn(async move {
        agent.prompt("hello").await.unwrap();
        agent
    });
    while !event_names(&events.lock()).iter().any(|name| name == "retrying") {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    abort.abort();
    let agent = run.await.unwrap();
    assert_eq!(requests.lock().len(), 1);
    let [aborted] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(aborted.stop_reason, StopReason::Aborted);
}

/// Fails `spent` times for want of an account with capacity (the first to
/// reset at `until`), then answers. Records how many messages each request carried.
fn no_account_left(spent: usize, until: Option<u64>) -> (StreamFn, Arc<Mutex<Vec<usize>>>) {
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = requests.clone();
    let count = AtomicUsize::new(0);
    let stream: StreamFn = Arc::new(move |model, context, _options| {
        seen.lock().push(context.messages.len());
        let attempt = count.fetch_add(1, Ordering::SeqCst);
        let mut message = reply(model, vec![AssistantPart::Text { text: format!("attempt {attempt}"), text_signature: None }]);
        let (sink, stream) = event_stream();
        if attempt < spent {
            message.content.clear();
            message.stop_reason = StopReason::Error;
            message.error_message = Some("Your Claude account reached its usage limit; it resets Tue 18:00 (in 3 h).".into());
            factory_agent::accounts::mark_none_left(&mut message, until);
            sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
        } else {
            sink.push(StreamEvent::Start { partial: message.clone() });
            sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        }
        Ok(stream)
    });
    (stream, requests)
}

/// A wait that records what it waited for and ends at once, or says not to
/// wait (`waits` false).
/// What each wait was for: until when, and why.
type Waited = Arc<Mutex<Vec<(Option<u64>, String)>>>;

fn waits(waits: bool) -> (factory_agent::WaitOut, Waited) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = seen.clone();
    let wait: factory_agent::WaitOut = Arc::new(move |until, why| {
        recorded.lock().push((until, why));
        Box::pin(async move { waits })
    });
    (wait, seen)
}

#[tokio::test(start_paused = true)]
async fn with_no_account_left_the_run_waits_and_asks_again_with_what_arrived_meanwhile() {
    let (stream, requests) = no_account_left(2, Some(1_900_000_000_000));
    let mut agent = Agent::new(AgentOptions {
        inbox: Some({
            let given = Arc::new(AtomicUsize::new(0));
            Arc::new(move || {
                let first = given.fetch_add(1, Ordering::SeqCst) == 0;
                Box::pin(async move {
                    let messages = if first { vec![factory_agent::UserMessage::text("meanwhile", 1)] } else { Vec::new() };
                    factory_agent::Arrived { messages, ..Default::default() }
                })
            })
        }),
        ..options(stream)
    });
    let (wait, waited) = waits(true);
    agent.wait_out_with(wait);
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    // Asked again after each wait; the message that came during the first went with the second request.
    assert_eq!(*requests.lock(), [1, 2, 2]);
    let why = "Your Claude account reached its usage limit; it resets Tue 18:00 (in 3 h).".to_owned();
    assert_eq!(*waited.lock(), [(Some(1_900_000_000_000), why.clone()), (Some(1_900_000_000_000), why)]);
    let events = events.lock().clone();
    let names = event_names(&events);
    assert_eq!(names.iter().filter(|name| *name == "waiting").count(), 2);
    // The failures never enter the context.
    let [answer] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(answer.stop_reason, StopReason::Stop);
    assert_eq!(agent.state.messages.len(), 3, "hello, meanwhile, the answer");
    assert_eq!(agent.state.error_message, None);
}

#[tokio::test(start_paused = true)]
async fn with_no_account_left_and_no_way_to_wait_the_run_fails_as_before() {
    for wait in [None, Some(waits(false).0)] {
        let declined = wait.is_some();
        let (stream, requests) = no_account_left(1, None);
        let mut agent = agent(stream);
        if let Some(wait) = wait {
            agent.wait_out_with(wait);
        }
        agent.prompt("hello").await.unwrap();
        assert_eq!(requests.lock().len(), 1, "declined: {declined}");
        let [failure] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
        assert_eq!(failure.stop_reason, StopReason::Error);
        assert!(factory_agent::accounts::none_left(failure).is_some(), "the mark stays");
    }
}

#[tokio::test(start_paused = true)]
async fn stopping_a_wait_ends_the_run_as_aborted() {
    let (stream, requests) = no_account_left(1, None);
    let mut agent = agent(stream);
    agent.wait_out_with(Arc::new(|_, _| Box::pin(std::future::pending())));
    let abort = agent.abort_handle();
    let events = collect(&mut agent);
    let run = tokio::spawn(async move {
        agent.prompt("hello").await.unwrap();
        agent
    });
    while !event_names(&events.lock()).iter().any(|name| name == "waiting") {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    abort.abort();
    let agent = run.await.unwrap();
    assert_eq!(requests.lock().len(), 1);
    let [aborted] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(aborted.stop_reason, StopReason::Aborted);
}

/// A wait for the provider that records why it waited and ends at
/// once (Ok), or gives up with `give_up` from the `n`-th wait on.
fn provider_waits(give_up: Option<(usize, &'static str)>) -> (factory_agent::WaitProvider, Arc<Mutex<Vec<String>>>) {
    let (wait, seen, _) = provider_waits_asked(give_up);
    (wait, seen)
}

/// `provider_waits`, with how long each wait was asked to last at least.
type Asked = Arc<Mutex<Vec<Option<u64>>>>;
fn provider_waits_asked(give_up: Option<(usize, &'static str)>) -> (factory_agent::WaitProvider, Arc<Mutex<Vec<String>>>, Asked) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let asked: Asked = Arc::default();
    let (recorded, recorded_asked) = (seen.clone(), asked.clone());
    let wait: factory_agent::WaitProvider = Arc::new(move |why, at_least| {
        recorded_asked.lock().push(at_least);
        let mut recorded = recorded.lock();
        recorded.push(why);
        let answer = match give_up {
            Some((from, said)) if recorded.len() >= from => Err(Some(said.to_owned())),
            _ => Ok(()),
        };
        Box::pin(async move { answer })
    });
    (wait, seen, asked)
}

/// Past the quick retries, a provider that keeps failing in a way
/// that passes is waited for, each wait followed by one request (no quick
/// retries again), until it answers.
#[tokio::test(start_paused = true)]
async fn a_failing_provider_is_waited_for_until_it_answers() {
    let (stream, requests) = flaky(&["529 Overloaded"; 6]);
    let mut agent = agent(stream);
    let (wait, waited) = provider_waits(None);
    agent.wait_provider_with(wait);
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    assert_eq!(requests.lock().len(), 7);
    assert_eq!(*waited.lock(), vec!["529 Overloaded".to_owned(); 4]);
    assert_eq!(retries(&events.lock()), [(1, 1000), (2, 2000)]);
    let [answer] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(answer.stop_reason, StopReason::Stop);
    assert_eq!(agent.state.error_message, None);
}

/// A wait that gives up ends the run with what it said; a failure that
/// won't pass (a refused key, a refused request) is never waited for.
#[tokio::test(start_paused = true)]
async fn a_provider_wait_that_gives_up_ends_the_run_and_lasting_failures_are_not_waited_for() {
    let (stream, requests) = flaky(&["503 Service Unavailable"; 10]);
    let mut gives_up = agent(stream);
    let (wait, waited) = provider_waits(Some((2, "OpenRouter kept failing to answer for 12 hours.")));
    gives_up.wait_provider_with(wait);
    gives_up.prompt("hello").await.unwrap();
    assert_eq!((requests.lock().len(), waited.lock().len()), (4, 2));
    assert_eq!(gives_up.state.error_message.as_deref(), Some("OpenRouter kept failing to answer for 12 hours."));

    for (failure, attempts) in [("401 invalid api key", 1), ("400 Invalid request: max_tokens 500 is too many", 3)] {
        let (stream, requests) = flaky(Box::leak(Box::new([failure; 3])));
        let mut agent = agent(stream);
        let (wait, waited) = provider_waits(None);
        agent.wait_provider_with(wait);
        agent.prompt("hello").await.unwrap();
        assert_eq!((requests.lock().len(), waited.lock().len()), (attempts, 0), "{failure}");
        assert_eq!(agent.state.error_message.as_deref(), Some(failure));
    }
}

/// An answer whose stream ends with no terminal event (the connection cut
/// mid-answer) is a failure that passes: retried, not the end of the run.
#[tokio::test(start_paused = true)]
async fn an_answer_cut_off_mid_stream_is_retried() {
    let count = Arc::new(AtomicUsize::new(0));
    let asked = count.clone();
    let stream: StreamFn = Arc::new(move |model, _context, _options| {
        let attempt = asked.fetch_add(1, Ordering::SeqCst);
        let message = reply(model, vec![AssistantPart::Text { text: format!("attempt {attempt}"), text_signature: None }]);
        let (sink, stream) = event_stream();
        sink.push(StreamEvent::Start { partial: message.clone() });
        if attempt > 0 {
            sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
        }
        Ok(stream)
    });
    let mut agent = agent(stream);
    let events = collect(&mut agent);
    agent.prompt("hello").await.unwrap();
    assert_eq!(count.load(Ordering::SeqCst), 2);
    assert_eq!(retries(&events.lock()), [(1, 1000)]);
    let [answer] = assistants(&agent.state.messages)[..] else { panic!("{:?}", agent.state.messages) };
    assert_eq!(
        (answer.stop_reason, answer.content.clone()),
        (StopReason::Stop, vec![AssistantPart::Text { text: "attempt 1".into(), text_signature: None }])
    );
}

/// A failure whose answer said how long to wait (Retry-After) is retried no
/// sooner, the quick retries and the long wait alike, at most 15 minutes
/// .
#[tokio::test(start_paused = true)]
async fn retry_after_holds_retries_and_waits_back() {
    for (after, held) in [(60_000, 60_000), (3_600_000, 900_000)] {
        let count = Arc::new(AtomicUsize::new(0));
        let asked_count = count.clone();
        let stream: StreamFn = Arc::new(move |model, _context, _options| {
            let attempt = asked_count.fetch_add(1, Ordering::SeqCst);
            let mut message = reply(model, vec![AssistantPart::Text { text: format!("attempt {attempt}"), text_signature: None }]);
            let (sink, stream) = event_stream();
            sink.push(StreamEvent::Start { partial: message.clone() });
            if attempt < 4 {
                message.stop_reason = StopReason::Error;
                message.error_message = Some("429 Too Many Requests".into());
                message.extra.insert(factory_agent::accounts::RETRY_AFTER.into(), serde_json::json!(after));
                sink.push(StreamEvent::Error { reason: StopReason::Error, error: message });
            } else {
                sink.push(StreamEvent::Done { reason: StopReason::Stop, message });
            }
            Ok(stream)
        });
        let mut agent = agent(stream);
        let (wait, waited, asked) = provider_waits_asked(None);
        agent.wait_provider_with(wait);
        let events = collect(&mut agent);
        agent.prompt("hello").await.unwrap();
        assert_eq!(retries(&events.lock()), [(1, held), (2, held)], "{after}");
        assert_eq!(waited.lock().len(), 2);
        assert_eq!(*asked.lock(), [Some(after), Some(after)], "the wait is told what the provider asked; it caps it itself");
        assert_eq!(agent.state.error_message, None);
    }
}

#[test]
fn retry_after_reads_seconds_and_dates() {
    let header = |value: &str| {
        let mut headers = http::HeaderMap::new();
        headers.insert(http::header::RETRY_AFTER, value.parse().unwrap());
        factory_agent::accounts::retry_after_header(&headers)
    };
    assert_eq!(header("60"), Some(60_000));
    assert_eq!(header(" 2 "), Some(2_000));
    assert_eq!(header("soon"), None);
    assert_eq!(header("Wed, 21 Oct 2015 07:28:00 GMT"), Some(0), "a date past");
    let later = httpdate::fmt_http_date(std::time::SystemTime::now() + Duration::from_secs(120));
    let ms = header(&later).expect("a date");
    assert!((115_000..=120_000).contains(&ms), "{ms}");
    assert_eq!(factory_agent::accounts::retry_after_header(&http::HeaderMap::new()), None);
}
