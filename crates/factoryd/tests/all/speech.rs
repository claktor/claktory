//! Voice messages (#197): the speech model, downloaded on request from a
//! local server here, resumed, checked and kept; and, until they're built,
//! voice messages themselves refused rather than sent on as empty ones.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use factory_protocol::v1::{self, SpeechModelState as State, client_message::Body as Request, server_message::Body};
use factoryd::speech::Artifact;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

use crate::common::{Client, Scripted, View, apply_request, create_agent, send_message, start, start_with, temp_dir};
use v1::entry::Kind;

/// A stand-in for `claktory-speech`: `check` says yes, or no with `why`.
fn program(dir: &Path, why: Option<&str>) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join(if why.is_some() { "speech-no" } else { "speech-yes" });
    let script = match why {
        None => "#!/bin/sh\necho '{\"supported\": true}'\n".to_owned(),
        Some(why) => format!("#!/bin/sh\necho '{{\"supported\": false, \"code\": \"unsupported_cpu\", \"error\": \"{why}\"}}'\nexit 1\n"),
    };
    std::fs::write(&path, script).expect("the program");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    path
}

/// Serves `content` at /model.gguf, with Range requests. The first `cut`
/// answers stop halfway, as a dropped connection does. Counts the requests,
/// and keeps the last Range asked for.
struct Server {
    url: String,
    requests: Arc<AtomicUsize>,
    ranges: Arc<std::sync::Mutex<Vec<String>>>,
}

async fn serve(content: Vec<u8>, cut: usize) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let url = format!("http://{}/model.gguf", listener.local_addr().expect("addr"));
    let requests = Arc::new(AtomicUsize::new(0));
    let ranges = Arc::new(std::sync::Mutex::new(Vec::new()));
    let content = Arc::new(content);
    let (counted, kept) = (requests.clone(), ranges.clone());
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else { return };
            let number = counted.fetch_add(1, Ordering::SeqCst);
            let (content, kept) = (content.clone(), kept.clone());
            tokio::spawn(async move {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && stream.read(&mut byte).await.unwrap_or(0) == 1 {
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head).to_lowercase();
                let from: usize = head
                    .lines()
                    .find_map(|line| line.strip_prefix("range: bytes="))
                    .and_then(|range| range.trim_end_matches('-').trim_end_matches("-\r").parse().ok())
                    .unwrap_or(0);
                kept.lock().unwrap().push(format!("{from}"));
                let body = &content[from.min(content.len())..];
                let status = if from > 0 { "206 Partial Content" } else { "200 OK" };
                let answer = format!("HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(answer.as_bytes()).await;
                let sent = if number < cut { &body[..body.len() / 2] } else { body };
                let _ = stream.write_all(sent).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    Server { url, requests, ranges }
}

fn artifact(url: &str, content: &[u8]) -> Artifact {
    use sha2::{Digest, Sha256};
    Artifact {
        url: url.to_owned(),
        file: "test-model.gguf".to_owned(),
        bytes: content.len() as u64,
        sha256: Sha256::digest(content).iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

fn model_of(workspace: &v1::Workspace) -> v1::SpeechModel {
    workspace.speech_model.clone().expect("the speech model's state")
}

/// Sends `request` and waits for the model to come to a state that's
/// `done`, counting what came before the request's answer too.
async fn until(client: &mut Client, request: Request, done: impl Fn(&v1::SpeechModel) -> bool) -> v1::SpeechModel {
    let seen = client.ok(request).await.into_iter().filter_map(|message| match message.body {
        Some(Body::Workspace(workspace)) if done(&model_of(&workspace)) => Some(model_of(&workspace)),
        _ => None,
    });
    match seen.last() {
        Some(shown) => shown,
        None => wait(client, done).await,
    }
}

async fn wait(client: &mut Client, done: impl Fn(&v1::SpeechModel) -> bool) -> v1::SpeechModel {
    client
        .wait_for(|body| match body {
            Body::Workspace(workspace) if done(&model_of(workspace)) => Some(model_of(workspace)),
            _ => None,
        })
        .await
}

fn refusal(body: &Body) -> String {
    match body {
        Body::Error(error) => error.message.clone(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn without_speech_recognition_the_model_is_unsupported_and_never_downloaded() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let shown = model_of(&workspace);
    assert_eq!((shown.state(), shown.error.as_str()), (State::Unsupported, "Speech recognition isn't installed with this Claktory."));
    assert_eq!(shown.name, "Parakeet v3 (25 languages)");
    assert_eq!(shown.total, 739_508_576);
    assert!(shown.attribution.contains("CC BY 4.0"), "{}", shown.attribution);
    let install = client.request(Request::InstallSpeechModel(v1::InstallSpeechModel {})).await;
    assert_eq!(refusal(&install.body), "Speech recognition isn't installed with this Claktory.");

    // A processor it can't run on says so.
    let server = serve(b"never".to_vec(), 0).await;
    factory.set_speech_source(Some(program(&dir, Some("It lacks AVX2."))), artifact(&server.url, b"never"));
    let shown = wait(&mut client, |model| model.state() == State::Unsupported && !model.error.contains("installed")).await;
    assert_eq!(shown.error, "It lacks AVX2.");
    let install = client.request(Request::InstallSpeechModel(v1::InstallSpeechModel {})).await;
    assert_eq!(refusal(&install.body), "It lacks AVX2.");
    assert_eq!(server.requests.load(Ordering::SeqCst), 0, "nothing downloaded");
}

#[tokio::test]
async fn a_model_download_carries_on_after_a_restart_and_is_checked_and_kept() {
    let dir = temp_dir();
    let content: Vec<u8> = (0..300_000u32).map(|n| (n % 251) as u8).collect();
    let server = serve(content.clone(), 1).await;
    let source = artifact(&server.url, &content);
    let speech = program(&dir, None);
    {
        let (addr, factory) = start(&dir).await;
        factory.set_speech_source(Some(speech.clone()), source.clone());
        let (mut client, workspace, _) = Client::connect(addr).await;
        let shown = model_of(&workspace);
        assert_eq!((shown.state(), shown.total), (State::Absent, 300_000));

        // The first answer stops halfway: it fails, saying where.
        let failed =
            until(&mut client, Request::InstallSpeechModel(v1::InstallSpeechModel {}), |model| model.state() == State::Failed).await;
        assert!(failed.error.starts_with("The download stopped at"), "{}", failed.error);
        assert!(dir.join("home/models/speech/test-model.gguf.partial").exists());
    }
    {
        // After a restart it carries on by itself, from there.
        let (addr, factory) = start(&dir).await;
        let (mut client, _, _) = Client::connect(addr).await;
        factory.set_speech_source(Some(speech.clone()), source.clone());
        wait(&mut client, |model| model.state() == State::Ready).await;
        assert_eq!(server.ranges.lock().unwrap().clone(), vec!["0".to_owned(), "150000".to_owned()]);
        let kept = dir.join("home/models/speech/test-model.gguf");
        assert_eq!(std::fs::read(&kept).expect("the model"), content);
        assert!(dir.join("home/models/speech/test-model.receipt").exists());
        assert!(!dir.join("home/models/speech/test-model.gguf.partial").exists());
        // Ready, installing again is nothing to do.
        client.ok(Request::InstallSpeechModel(v1::InstallSpeechModel {})).await;
    }
    // After a restart it's ready at once, from its receipt.
    let (addr, factory) = start(&dir).await;
    factory.set_speech_source(Some(speech), source);
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(model_of(&workspace).state(), State::Ready);
    assert_eq!(server.requests.load(Ordering::SeqCst), 2, "not downloaded again");

    // Removed, it's gone.
    until(&mut client, Request::RemoveSpeechModel(v1::RemoveSpeechModel {}), |model| model.state() == State::Absent).await;
    assert!(!dir.join("home/models/speech/test-model.gguf").exists());
    assert!(!dir.join("home/models/speech/test-model.receipt").exists());
}

#[tokio::test]
async fn a_damaged_download_is_thrown_away() {
    let dir = temp_dir();
    let server = serve(b"not the model at all".to_vec(), 0).await;
    let (addr, factory) = start(&dir).await;
    factory.set_speech_source(Some(program(&dir, None)), artifact(&server.url, b"the model, as it should be"));
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(model_of(&workspace).state(), State::Absent);
    let failed = until(&mut client, Request::InstallSpeechModel(v1::InstallSpeechModel {}), |model| model.state() == State::Failed).await;
    assert!(!failed.error.is_empty());
    assert!(!dir.join("home/models/speech/test-model.gguf").exists());
}

/// A stand-in for `claktory-speech` that serves: a "recording" is a text
/// file of what was said. "no speech" fails as the real one does, "die"
/// ends it, "long…" is a recording of 10 minutes and 1 ms (and then says
/// nothing more, as if transcribing it), "silent N" is N ms with no
/// speech in it, and "wait …" waits for a `.go` file beside it before
/// saying "…". Each start adds a line to `starts`.
fn serving(dir: &Path) -> PathBuf {
    use std::os::unix::fs::PermissionsExt as _;
    let path = dir.join("speech-serve");
    let script = r#"#!/bin/sh
case "$1" in
check) echo '{"supported": true}'; exit 0;;
serve) ;;
*) exit 2;;
esac
echo started >> "$(dirname "$0")/starts"
echo '{"ready": true, "device": "CPU", "load_ms": 1}'
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed 's/.*"id":\([0-9]*\).*/\1/')
  path=$(printf '%s' "$line" | sed 's/.*"transcribe":"\([^"]*\)".*/\1/')
  said=$(cat "$path")
  [ "$said" = die ] && exit 1
  case "$said" in
  long*) echo "{\"id\":$id,\"decoded\":true,\"duration_ms\":600001}"; continue;;
  silent*)
    echo "{\"id\":$id,\"decoded\":true,\"duration_ms\":${said#silent }}"
    echo "{\"id\":$id,\"code\":\"no_speech\",\"error\":\"Didn't hear any speech.\"}"
    continue;;
  esac
  echo "{\"id\":$id,\"decoded\":true,\"duration_ms\":41200}"
  case "$said" in
  wait*) while [ ! -f "$path.go" ]; do sleep 0.05; done; said=${said#wait };;
  esac
  if [ "$said" = "no speech" ]; then
    echo "{\"id\":$id,\"code\":\"no_speech\",\"error\":\"Didn't hear any speech.\"}"
  else
    echo "{\"id\":$id,\"text\":\"$said\",\"language\":\"en\",\"duration_ms\":41200,\"ms\":5}"
  fi
done
"#;
    std::fs::write(&path, script).expect("the program");
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).expect("executable");
    path
}

/// A "recording" in the agent's uploads, saying `said`.
fn recording(dir: &Path, agent: &str, name: &str, said: &str) -> v1::Attachment {
    let folder = dir.join(format!("home/agents/{agent}/uploads"));
    std::fs::create_dir_all(&folder).expect("uploads");
    let path = folder.join(name);
    std::fs::write(&path, said).expect("the recording");
    v1::Attachment { path: path.display().to_string(), name: name.to_owned(), size: said.len() as u64, mime: "audio/mp4".into() }
}

fn voice(agent: &str, recording: v1::Attachment) -> Request {
    Request::SendMessage(v1::SendMessage { agent_id: agent.into(), voice: true, attachments: vec![recording], ..Default::default() })
}

/// The selected agent's queue as (text, transcription state), oldest first.
fn queued(view: &View) -> Vec<(String, Option<v1::TranscriptionState>)> {
    let messages = view.queued.as_ref().map(|queued| queued.messages.clone()).unwrap_or_default();
    messages.iter().map(|message| (message.text.clone(), message.transcription.as_ref().map(|it| it.state()))).collect()
}

fn voice_entry(view: &View, text: &str) -> Option<v1::Transcription> {
    view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::User(user)) if user.text == text => Some(user.voice.clone()),
        _ => None,
    })?
}

/// A Claktory with the speech model ready, and an agent, Ops, selected.
async fn ready(dir: &Path, models: Scripted) -> (Arc<factoryd::Factory>, Client, View, Server) {
    let (_, factory, client, view, server) = ready_at(dir, models).await;
    (factory, client, view, server)
}

/// As `ready`, with where it listens, for another connection.
async fn ready_at(dir: &Path, models: Scripted) -> (std::net::SocketAddr, Arc<factoryd::Factory>, Client, View, Server) {
    let model = b"a speech model".to_vec();
    let server = serve(model.clone(), 0).await;
    let (addr, factory) = start_with(dir, models).await;
    factory.set_speech_source(Some(serving(dir)), artifact(&server.url, &model));
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(model_of(&workspace).state(), State::Absent);
    until(&mut client, Request::InstallSpeechModel(v1::InstallSpeechModel {}), |model| model.state() == State::Ready).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    (addr, factory, client, view, server)
}

fn idle(view: &View) -> bool {
    view.agent("ops").status() == v1::AgentStatus::Idle && view.agent("ops").queued_messages == 0
}

#[tokio::test]
async fn a_voice_message_reaches_the_agent_as_the_persons_words_in_the_order_sent() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (_factory, mut client, mut view, _server) = ready(&dir, models).await;

    // Said, transcribed, and read by the agent as the person's, saying so.
    apply_request(&mut client, &mut view, voice("ops", recording(&dir, "ops", "Voice.m4a", "Ship it."))).await;
    view.follow(&mut client, |view| voice_entry(view, "Ship it.").is_some() && idle(view)).await;
    let heard = voice_entry(&view, "Ship it.").expect("its transcription");
    assert_eq!((heard.state(), heard.duration_ms, heard.language.as_str()), (v1::TranscriptionState::Done, 41_200, "en"));
    let prompt = requests.lock().expect("lock").last().map(|request| request.prompt.clone()).expect("a request");
    assert!(prompt.starts_with("<sent from=\""), "{prompt}");
    assert!(prompt.ends_with(" voice=\"42s\">\nShip it.\n</sent>"), "no tag for the recording: {prompt}");
    let user = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::User(user)) if user.text == "Ship it." => Some(user.clone()),
        _ => None,
    });
    assert_eq!(user.expect("the message").attachments[0].name, "Voice.m4a", "the recording, to play back");

    // What the person types after one still being transcribed waits for it.
    let asked = requests.lock().expect("lock").len();
    let slow = recording(&dir, "ops", "Voice 2.m4a", "wait And deploy.");
    apply_request(&mut client, &mut view, voice("ops", slow.clone())).await;
    apply_request(&mut client, &mut view, send_message("ops", "Then tell me.")).await;
    view.follow(&mut client, |view| {
        view.queued.as_ref().is_some_and(|queued| {
            queued.messages.first().and_then(|first| first.transcription.as_ref()).is_some_and(|it| it.duration_ms == 41_200)
        })
    })
    .await;
    assert_eq!(queued(&view), [(String::new(), Some(v1::TranscriptionState::Pending)), ("Then tell me.".to_owned(), None)]);
    assert_eq!(requests.lock().expect("lock").len(), asked, "the agent hasn't been given either");
    std::fs::write(format!("{}.go", slow.path), "").expect("go");
    view.follow(&mut client, |view| voice_entry(view, "And deploy.").is_some() && idle(view)).await;
    let said: Vec<String> = view
        .entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::User(user)) => Some(user.text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(said, ["Ship it.", "And deploy.", "Then tell me."]);
}

#[tokio::test]
async fn a_voice_message_that_fails_holds_nothing_up_and_can_be_retried_or_discarded() {
    let dir = temp_dir();
    let (_factory, mut client, mut view, _server) = ready(&dir, Scripted::signed_in()).await;

    // Nothing heard: it fails, saying why, and what's typed after it goes on.
    let silent = recording(&dir, "ops", "Voice.m4a", "no speech");
    apply_request(&mut client, &mut view, voice("ops", silent.clone())).await;
    apply_request(&mut client, &mut view, send_message("ops", "Anyway, hi.")).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && view.agent("ops").status() == v1::AgentStatus::Idle).await;
    let failed = view.queued.as_ref().expect("the queue").messages[0].clone();
    let why = failed.transcription.clone().expect("a voice message");
    assert_eq!(
        (why.state(), why.error_code.as_str(), why.error.as_str()),
        (v1::TranscriptionState::Failed, "no_speech", "Didn't hear any speech.")
    );
    assert_eq!(view.agent("ops").queued_messages, 1, "it waits for the person");

    // Tried again, it goes through.
    std::fs::write(&silent.path, "Better now.").expect("rerecorded");
    let retry = |id| Request::RetryVoiceMessage(v1::RetryVoiceMessage { agent_id: "ops".into(), message_id: id });
    apply_request(&mut client, &mut view, retry(failed.id)).await;
    view.follow(&mut client, |view| voice_entry(view, "Better now.").is_some() && idle(view)).await;
    assert_eq!(refusal(&client.request(retry(failed.id)).await.body), "That voice message isn't waiting any more.");

    // Or it's discarded, undelivered; the recording stays.
    let silent = recording(&dir, "ops", "Voice 2.m4a", "no speech");
    apply_request(&mut client, &mut view, voice("ops", silent.clone())).await;
    view.follow(&mut client, |view| queued(view) == [(String::new(), Some(v1::TranscriptionState::Failed))]).await;
    let id = view.queued.as_ref().expect("the queue").messages[0].id;
    let discard = Request::DiscardVoiceMessage(v1::DiscardVoiceMessage { agent_id: "ops".into(), message_id: id });
    apply_request(&mut client, &mut view, discard).await;
    view.follow(&mut client, |view| queued(view).is_empty()).await;
    assert!(Path::new(&silent.path).exists());
    let typed = view.queued.as_ref().map(|queued| queued.agent_id.clone());
    assert_eq!(typed.as_deref(), Some("ops"));
}

#[tokio::test]
async fn a_short_voice_message_with_no_speech_vanishes_and_a_longer_one_says_so() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (_factory, mut client, mut view, _server) = ready(&dir, models).await;

    // Under 5 seconds of silence: dropped, never reaching the agent, and
    // what's typed after it goes on; the recording stays.
    let short = recording(&dir, "ops", "Voice.m4a", "silent 4999");
    apply_request(&mut client, &mut view, voice("ops", short.clone())).await;
    apply_request(&mut client, &mut view, send_message("ops", "Anyway, hi.")).await;
    view.follow(&mut client, |view| {
        view.entries.iter().any(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text == "Anyway, hi.")) && idle(view)
    })
    .await;
    assert!(queued(&view).is_empty(), "nothing left for the person: {:?}", queued(&view));
    let users: Vec<_> = view.entries.iter().filter(|entry| matches!(entry.kind, Some(Kind::User(_)))).collect();
    assert_eq!(users.len(), 1, "only the typed message is in the conversation");
    let prompts: Vec<String> = requests.lock().expect("lock").iter().map(|request| request.prompt.clone()).collect();
    assert_eq!(prompts.len(), 1, "{prompts:?}");
    assert!(prompts[0].contains("Anyway, hi.") && !prompts[0].contains("voice="), "{prompts:?}");
    assert!(Path::new(&short.path).exists());

    // 5 seconds or more: it may be the microphone, so the person sees it.
    apply_request(&mut client, &mut view, voice("ops", recording(&dir, "ops", "Voice 2.m4a", "silent 5000"))).await;
    view.follow(&mut client, |view| queued(view) == [(String::new(), Some(v1::TranscriptionState::Failed))]).await;
    let why = view.queued.as_ref().expect("the queue").messages[0].transcription.clone().expect("a voice message");
    assert_eq!((why.error_code.as_str(), why.duration_ms), ("no_speech", 5_000));
    assert_eq!(view.agent("ops").queued_messages, 1, "it waits for the person");
    assert_eq!(requests.lock().expect("lock").len(), 1, "the agent wasn't given it");
}

#[tokio::test]
async fn voice_messages_need_the_model_and_fail_without_it() {
    let dir = temp_dir();
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, _, _) = Client::connect(addr).await;
    client.ok(create_agent("", "Ops", "")).await;
    // No speech recognition here at all.
    let said = recording(&dir, "ops", "Voice.m4a", "Hello.");
    assert_eq!(refusal(&client.request(voice("ops", said.clone())).await.body), "Speech recognition isn't installed with this Claktory.");

    // Not downloaded: the app asks the person first.
    let server = serve(b"a speech model".to_vec(), 0).await;
    factory.set_speech_source(Some(serving(&dir)), artifact(&server.url, b"a speech model"));
    wait(&mut client, |model| model.state() == State::Absent).await;
    assert_eq!(refusal(&client.request(voice("ops", said.clone())).await.body), "Voice messages need the speech model. Download it first.");
    let typed = Request::SendMessage(v1::SendMessage {
        agent_id: "ops".into(),
        voice: true,
        text: "Hi".into(),
        attachments: vec![said.clone()],
        ..Default::default()
    });
    // Typed text with it (#425) changes nothing: it needs the model too.
    assert_eq!(refusal(&client.request(typed).await.body), "Voice messages need the speech model. Download it first.");

    // Removed while one waits: it fails, saying so.
    until(&mut client, Request::InstallSpeechModel(v1::InstallSpeechModel {}), |model| model.state() == State::Ready).await;
    client.ok(Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    let waiting = recording(&dir, "ops", "Voice 2.m4a", "wait Never mind.");
    client.ok(voice("ops", waiting)).await;
    let removed = client.ok(Request::RemoveSpeechModel(v1::RemoveSpeechModel {})).await;
    let failed = |body: &Body| match body {
        Body::QueuedMessages(queued) => queued
            .messages
            .first()
            .and_then(|message| message.transcription.clone())
            .filter(|it| it.state() == v1::TranscriptionState::Failed),
        _ => None,
    };
    let seen = removed.iter().find_map(|message| message.body.as_ref().and_then(failed));
    let failed = match seen {
        Some(failed) => failed,
        None => client.wait_for(failed).await,
    };
    assert_eq!(failed.error_code, "no_model");
}

#[tokio::test]
async fn a_voice_message_waiting_at_a_restart_is_transcribed_after_it() {
    let dir = temp_dir();
    let slow = {
        let (_factory, mut client, mut view, _server) = ready(&dir, Scripted::signed_in()).await;
        // Speech recognition ends midway: it fails, and the next starts it afresh.
        apply_request(&mut client, &mut view, voice("ops", recording(&dir, "ops", "Voice.m4a", "die"))).await;
        view.follow(&mut client, |view| queued(view) == [(String::new(), Some(v1::TranscriptionState::Failed))]).await;
        let id = view.queued.as_ref().expect("the queue").messages[0].id;
        let discard = Request::DiscardVoiceMessage(v1::DiscardVoiceMessage { agent_id: "ops".into(), message_id: id });
        apply_request(&mut client, &mut view, discard).await;
        let slow = recording(&dir, "ops", "Voice 2.m4a", "wait Carry on.");
        apply_request(&mut client, &mut view, voice("ops", slow.clone())).await;
        view.follow(&mut client, |view| queued(view) == [(String::new(), Some(v1::TranscriptionState::Pending))]).await;
        slow
    };
    let (addr, factory) = start_with(&dir, Scripted::signed_in()).await;
    let model = b"a speech model".to_vec();
    let server = serve(model.clone(), 0).await;
    let mut source = artifact(&server.url, &model);
    source.url = "http://127.0.0.1:9/never".into();
    factory.set_speech_source(Some(serving(&dir)), source);
    let (mut client, workspace, _) = Client::connect(addr).await;
    assert_eq!(model_of(&workspace).state(), State::Ready);
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    std::fs::write(format!("{}.go", slow.path), "").expect("go");
    view.follow(&mut client, |view| voice_entry(view, "Carry on.").is_some() && idle(view)).await;
    let starts = std::fs::read_to_string(dir.join("starts")).expect("starts");
    assert!(starts.lines().count() >= 3, "started afresh after it ended, and after the restart: {starts}");
}

/// With the real thing: `claktory-speech` built (target/debug) and the
/// speech model (CLAKTORY_SPEECH_MODEL: Parakeet v3's GGUF), a recording
/// made with macOS's `say` reaches the agent as what it said. Without them
/// there's nothing to check.
#[tokio::test]
async fn with_the_real_model_a_spoken_message_reads_as_said() {
    let Some(gguf) = std::env::var_os("CLAKTORY_SPEECH_MODEL") else { return };
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/claktory-speech");
    if !program.exists() {
        return;
    }
    let dir = temp_dir();
    let aiff = dir.join("said.aiff");
    let m4a = dir.join("Voice.m4a");
    let said = std::process::Command::new("say").arg("-o").arg(&aiff).arg("Please deploy the website today.").status();
    let made =
        std::process::Command::new("afconvert").args(["-f", "m4af", "-d", "aac", "-r", "16000", "-c", "1"]).arg(&aiff).arg(&m4a).status();
    if !(said.is_ok_and(|status| status.success()) && made.is_ok_and(|status| status.success())) {
        return;
    }
    // The model, as a download a restart stopped right at its end: it's checked, and kept.
    let parakeet = Artifact::parakeet();
    let models = dir.join("home/models/speech");
    std::fs::create_dir_all(&models).expect("models");
    std::fs::copy(&gguf, models.join(format!("{}.partial", parakeet.file))).expect("the model");

    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, factory) = start_with(&dir, models).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    factory.set_speech_source(Some(program), parakeet);
    wait(&mut client, |model| model.state() == State::Ready).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Ops", "")).await;
    apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    let mut recording = recording(&dir, "ops", "Voice.m4a", "");
    std::fs::copy(&m4a, &recording.path).expect("the recording");
    recording.size = std::fs::metadata(&m4a).expect("the recording").len();
    apply_request(&mut client, &mut view, voice("ops", recording)).await;
    view.follow(&mut client, |view| view.entries.len() >= 2 && idle(view)).await;
    let prompt = requests.lock().expect("lock").last().map(|request| request.prompt.clone()).expect("a request");
    assert!(prompt.to_lowercase().contains("deploy the website today"), "{prompt}");
    assert!(prompt.contains(" voice=\"2s\">") || prompt.contains(" voice=\"3s\">"), "{prompt}");
}

/// Uploads a dictation saying `said` as `upload_id` (a recording for no
/// agent).
async fn dictation(client: &mut Client, upload_id: &str, said: &str) {
    let start = v1::StartUpload {
        upload_id: upload_id.into(),
        name: "dictation.m4a".into(),
        size: said.len() as u64,
        mime: "audio/mp4".into(),
        purpose: v1::UploadPurpose::Dictation.into(),
        ..Default::default()
    };
    let answer = client.request(Request::StartUpload(start)).await;
    assert!(matches!(answer.body, Body::UploadProgress(_)), "{:?}", answer.body);
    let chunk = v1::UploadChunk { upload_id: upload_id.into(), offset: 0, data: said.as_bytes().to_vec() };
    assert!(matches!(client.request(Request::UploadChunk(chunk)).await.body, Body::UploadProgress(_)));
    let finished = client.request(Request::FinishUpload(v1::FinishUpload { upload_id: upload_id.into(), sha256: String::new() })).await;
    match finished.body {
        Body::UploadedFile(file) => assert_eq!((file.attachment, file.blob_sha256.as_str()), (None, "")),
        other => panic!("expected the upload, got {other:?}"),
    }
}

/// Asks for `upload_id` to be transcribed, and waits for what was heard.
async fn transcribe(client: &mut Client, request_id: &str, upload_id: &str) -> v1::AudioTranscribed {
    let request = v1::TranscribeAudio { request_id: request_id.into(), upload_id: upload_id.into() };
    let events = client.ok(Request::TranscribeAudio(request)).await;
    let heard = |body: &Body| match body {
        Body::AudioTranscribed(heard) if heard.request_id == request_id => Some(heard.clone()),
        _ => None,
    };
    match events.iter().find_map(|message| message.body.as_ref().and_then(heard)) {
        Some(heard) => heard,
        None => client.wait_for(heard).await,
    }
}

fn kept_dictations(dir: &Path) -> Vec<String> {
    std::fs::read_dir(dir.join("home/speech/dictation"))
        .map(|entries| entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect())
        .unwrap_or_default()
}

fn answer(text: &str, error: &str, duration_ms: u64) -> (String, String, u64) {
    (text.to_owned(), error.to_owned(), duration_ms)
}

fn of(heard: &v1::AudioTranscribed) -> (String, String, u64) {
    (heard.text.clone(), heard.error.clone(), heard.duration_ms)
}

/// Dictation: a recording for no agent is transcribed for the connection
/// that asked, ahead of voice messages, and neither it nor its words are
/// kept or given to an agent.
#[tokio::test]
async fn a_dictation_is_heard_by_the_asker_alone_first_and_nothing_is_kept() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (addr, _factory, mut client, mut view, _server) = ready_at(&dir, models).await;
    // Dictations from a connection of their own; `client` follows Ops.
    let (mut asker, _, _) = Client::connect(addr).await;
    let (mut other, _, _) = Client::connect(addr).await;

    dictation(&mut asker, "d1", "Ship it.").await;
    assert_eq!(kept_dictations(&dir).len(), 1, "kept until it's transcribed");
    let heard = transcribe(&mut asker, "r1", "d1").await;
    assert_eq!(of(&heard), answer("Ship it.", "", 41_200));
    assert!(kept_dictations(&dir).is_empty(), "the recording is deleted");
    // Once only: the upload is gone, to upload again.
    assert_eq!(of(&transcribe(&mut asker, "r2", "d1").await), answer("", "no_recording", 0));
    assert_eq!(of(&transcribe(&mut asker, "r3", "never-uploaded").await), answer("", "no_recording", 0));
    // Nothing heard, or too long (stopped once its length is known).
    dictation(&mut asker, "d2", "no speech").await;
    assert_eq!(of(&transcribe(&mut asker, "r4", "d2").await), answer("", "no_speech", 0));
    dictation(&mut asker, "d3", "long talk").await;
    assert_eq!(of(&transcribe(&mut asker, "r5", "d3").await), answer("", "too_long", 600_001));
    assert!(kept_dictations(&dir).is_empty());

    // Ahead of voice messages: one being transcribed finishes, then the
    // dictation goes before the next.
    let first = recording(&dir, "ops", "Voice.m4a", "wait First.");
    let second = recording(&dir, "ops", "Voice 2.m4a", "wait Second.");
    apply_request(&mut client, &mut view, voice("ops", first.clone())).await;
    view.follow(&mut client, |view| {
        view.queued.as_ref().is_some_and(|queued| {
            queued.messages.first().and_then(|first| first.transcription.as_ref()).is_some_and(|it| it.duration_ms == 41_200)
        })
    })
    .await;
    apply_request(&mut client, &mut view, voice("ops", second.clone())).await;
    dictation(&mut asker, "d4", "Meanwhile.").await;
    let request = v1::TranscribeAudio { request_id: "r6".into(), upload_id: "d4".into() };
    asker.ok(Request::TranscribeAudio(request)).await;
    std::fs::write(format!("{}.go", first.path), "").expect("go");
    let heard = asker
        .wait_for(|body| match body {
            Body::AudioTranscribed(heard) => Some(heard.clone()),
            _ => None,
        })
        .await;
    assert_eq!((heard.request_id.as_str(), heard.text.as_str()), ("r6", "Meanwhile."), "before the second, which still waits");
    std::fs::write(format!("{}.go", second.path), "").expect("go");
    view.follow(&mut client, |view| voice_entry(view, "Second.").is_some() && idle(view)).await;

    // Never an agent's: only the voice messages reached Ops.
    let prompts: Vec<String> = requests.lock().expect("lock").iter().map(|request| request.prompt.clone()).collect();
    assert!(prompts.iter().all(|prompt| !prompt.contains("Ship it.") && !prompt.contains("Meanwhile.")), "{prompts:?}");
    // And never another connection's.
    let events = other.ok(Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    assert!(!events.iter().any(|message| matches!(message.body, Some(Body::AudioTranscribed(_)))));

    // Only for no agent, and only audio.
    let refused = |agent: &str, mime: &str| {
        Request::StartUpload(v1::StartUpload {
            upload_id: "x".into(),
            agent_id: agent.into(),
            name: "dictation.m4a".into(),
            size: 10,
            mime: mime.into(),
            purpose: v1::UploadPurpose::Dictation.into(),
        })
    };
    assert_eq!(
        refusal(&asker.request(refused("ops", "audio/mp4")).await.body),
        "A dictation is uploaded for no agent: leave agent_id empty."
    );
    assert!(refusal(&asker.request(refused("", "image/png")).await.body).starts_with("A dictation is a recording"));
    let empty = asker.request(Request::TranscribeAudio(v1::TranscribeAudio { request_id: String::new(), upload_id: "d9".into() })).await;
    assert!(matches!(empty.body, Body::Error(_)));
}

/// Without the model a dictation is refused, saying why, and the model is
/// never downloaded for it: the app asks the person first.
#[tokio::test]
async fn a_dictation_needs_the_model_and_never_downloads_it() {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    dictation(&mut client, "d1", "Hello.").await;
    assert_eq!(of(&transcribe(&mut client, "r1", "d1").await), answer("", "unsupported", 0));

    let server = serve(b"a speech model".to_vec(), 0).await;
    factory.set_speech_source(Some(serving(&dir)), artifact(&server.url, b"a speech model"));
    wait(&mut client, |model| model.state() == State::Absent).await;
    dictation(&mut client, "d2", "Hello.").await;
    assert_eq!(of(&transcribe(&mut client, "r2", "d2").await), answer("", "no_model", 0));
    assert_eq!(server.requests.load(Ordering::SeqCst), 0, "nothing downloaded");
    assert!(kept_dictations(&dir).is_empty(), "refused, the recording is deleted all the same");

    // Once it's there, it's heard.
    until(&mut client, Request::InstallSpeechModel(v1::InstallSpeechModel {}), |model| model.state() == State::Ready).await;
    dictation(&mut client, "d3", "Hello.").await;
    assert_eq!(of(&transcribe(&mut client, "r3", "d3").await), answer("Hello.", "", 41_200));
}

/// A dictation left from before a restart isn't kept.
#[tokio::test]
async fn dictations_from_before_a_restart_are_deleted() {
    let dir = temp_dir();
    let stale = dir.join("home/speech/dictation");
    std::fs::create_dir_all(&stale).expect("dir");
    std::fs::write(stale.join("old-1.audio"), "Old words.").expect("old");
    let (addr, _factory) = start(&dir).await;
    let _client = Client::connect(addr).await;
    for _ in 0..100 {
        if !stale.join("old-1.audio").exists() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("a dictation from before the restart is still there");
}

/// With the real thing, as above: a dictation made with `say` is heard as
/// said. Without `claktory-speech` built and CLAKTORY_SPEECH_MODEL there's
/// nothing to check.
#[tokio::test]
async fn with_the_real_model_a_dictation_reads_as_said() {
    let Some(gguf) = std::env::var_os("CLAKTORY_SPEECH_MODEL") else { return };
    let program = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/debug/claktory-speech");
    if !program.exists() {
        return;
    }
    let dir = temp_dir();
    let aiff = dir.join("said.aiff");
    let m4a = dir.join("dictation.m4a");
    let said = std::process::Command::new("say").arg("-o").arg(&aiff).arg("Approve it, but check the logs first.").status();
    let made =
        std::process::Command::new("afconvert").args(["-f", "m4af", "-d", "aac", "-r", "16000", "-c", "1"]).arg(&aiff).arg(&m4a).status();
    if !(said.is_ok_and(|status| status.success()) && made.is_ok_and(|status| status.success())) {
        return;
    }
    let parakeet = Artifact::parakeet();
    let models = dir.join("home/models/speech");
    std::fs::create_dir_all(&models).expect("models");
    std::fs::copy(&gguf, models.join(format!("{}.partial", parakeet.file))).expect("the model");
    let (addr, factory) = start(&dir).await;
    let (mut client, _, _) = Client::connect(addr).await;
    factory.set_speech_source(Some(program), parakeet);
    wait(&mut client, |model| model.state() == State::Ready).await;
    let bytes = std::fs::read(&m4a).expect("the recording");
    let start = v1::StartUpload {
        upload_id: "real".into(),
        name: "dictation.m4a".into(),
        size: bytes.len() as u64,
        mime: "audio/mp4".into(),
        purpose: v1::UploadPurpose::Dictation.into(),
        ..Default::default()
    };
    assert!(matches!(client.request(Request::StartUpload(start)).await.body, Body::UploadProgress(_)));
    for (n, chunk) in bytes.chunks(1024 * 1024).enumerate() {
        let chunk = v1::UploadChunk { upload_id: "real".into(), offset: (n * 1024 * 1024) as u64, data: chunk.to_vec() };
        assert!(matches!(client.request(Request::UploadChunk(chunk)).await.body, Body::UploadProgress(_)));
    }
    client.request(Request::FinishUpload(v1::FinishUpload { upload_id: "real".into(), sha256: String::new() })).await;
    let heard = transcribe(&mut client, "r", "real").await;
    assert_eq!(heard.error, "", "{heard:?}");
    assert!(heard.text.to_lowercase().contains("check the logs first"), "{heard:?}");
    assert!((2_000..5_000).contains(&heard.duration_ms), "{heard:?}");
    assert!(kept_dictations(&dir).is_empty());
}

/// What a client sees of a chat: the Workspace and the shown chat's queue.
#[derive(Default)]
struct ChatSeen {
    workspace: v1::Workspace,
    queued: Vec<v1::QueuedMessage>,
}

impl ChatSeen {
    fn apply(&mut self, body: &Body) {
        match body {
            Body::Workspace(workspace) => self.workspace = workspace.clone(),
            Body::ChatQueuedMessages(queued) => self.queued = queued.messages.clone(),
            _ => {}
        }
    }

    async fn request(&mut self, client: &mut Client, body: Request) -> Body {
        let reply = client.request(body).await;
        for event in &reply.events {
            if let Some(body) = &event.body {
                self.apply(body);
            }
        }
        reply.body
    }

    async fn follow(&mut self, client: &mut Client, done: impl Fn(&ChatSeen) -> bool) {
        while !done(self) {
            if let Some(body) = client.receive().await.body {
                self.apply(&body);
            }
        }
    }

    fn chat(&self, id: &str) -> Option<&v1::Chat> {
        self.workspace.chats.iter().find(|chat| chat.id == id)
    }

    fn settled(&self, id: &str) -> bool {
        self.chat(id).is_some_and(|chat| chat.status() == v1::AgentStatus::Idle && chat.queued_messages == 0)
    }
}

/// Uploads `said` as a chat's recording, as the app does.
async fn upload_for_chat(client: &mut Client, chat: &str, name: &str, said: &str) -> v1::Attachment {
    let upload_id = format!("up-{name}");
    let start = v1::StartUpload {
        upload_id: upload_id.clone(),
        agent_id: chat.into(),
        name: name.into(),
        size: said.len() as u64,
        mime: "audio/mp4".into(),
        ..Default::default()
    };
    assert!(matches!(client.request(Request::StartUpload(start)).await.body, Body::UploadProgress(_)));
    let chunk = v1::UploadChunk { upload_id: upload_id.clone(), offset: 0, data: said.as_bytes().to_vec() };
    assert!(matches!(client.request(Request::UploadChunk(chunk)).await.body, Body::UploadProgress(_)));
    let finish = v1::FinishUpload { upload_id, sha256: String::new() };
    match client.request(Request::FinishUpload(finish)).await.body {
        Body::UploadedFile(uploaded) => uploaded.attachment.expect("the recording"),
        other => panic!("{other:?}"),
    }
}

fn chat_voice(chat: &str, recording: v1::Attachment) -> Request {
    Request::SendChatMessage(v1::SendChatMessage { chat_id: chat.into(), voice: true, attachments: vec![recording], ..Default::default() })
}

/// #367: a chat takes a voice message as an agent's conversation does.
#[tokio::test]
async fn a_chat_takes_a_voice_message_as_an_agent_does() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (_factory, mut client, view, _server) = ready(&dir, models).await;
    let mut seen = ChatSeen { workspace: view.workspace.clone(), ..ChatSeen::default() };
    let settings =
        v1::UpdateSettings { default_model: Some(crate::common::MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    seen.request(&mut client, Request::UpdateSettings(settings)).await;
    // A new chat for a first message that's spoken: made empty, then sent to.
    seen.request(&mut client, Request::CreateChat(v1::CreateChat::default())).await;
    let id = seen.workspace.chats.first().expect("the chat").id.clone();

    // Only recordings are uploaded for a chat, and only voice messages sent.
    let png = v1::StartUpload {
        upload_id: "png".into(),
        agent_id: id.clone(),
        name: "a.png".into(),
        size: 3,
        mime: "image/png".into(),
        ..Default::default()
    };
    assert!(refusal(&client.request(Request::StartUpload(png)).await.body).starts_with("A chat takes only voice messages"));
    let recorded = upload_for_chat(&mut client, &id, "Voice.m4a", "wait Ship it.").await;
    assert!(recorded.path.contains("/chats/"), "kept with the chat: {}", recorded.path);
    let as_file =
        v1::SendChatMessage { chat_id: id.clone(), text: "see".into(), attachments: vec![recorded.clone()], ..Default::default() };
    assert_eq!(refusal(&client.request(Request::SendChatMessage(as_file)).await.body), "A chat takes only voice messages, not files.");

    // Transcribed, it's read as the person's words, saying so; a chat left
    // meanwhile is kept, since its first message is on its way.
    seen.request(&mut client, chat_voice(&id, recorded.clone())).await;
    seen.follow(&mut client, |seen| {
        seen.queued.first().and_then(|queued| queued.transcription.as_ref()).is_some_and(|it| it.state() == v1::TranscriptionState::Pending)
    })
    .await;
    seen.request(&mut client, Request::SelectAgent(v1::SelectAgent { agent_id: "ops".into() })).await;
    assert!(seen.chat(&id).is_some(), "not deleted as empty");
    std::fs::write(format!("{}.go", recorded.path), "").expect("go");
    seen.follow(&mut client, |seen| seen.settled(&id) && seen.chat(&id).is_some_and(|chat| !chat.title.is_empty())).await;
    let prompt = requests
        .lock()
        .expect("lock")
        .iter()
        .find(|request| request.system_prompt.starts_with("You're a chat") && request.prompt.contains("Ship it."))
        .map(|request| request.prompt.clone())
        .expect("the chat's request");
    assert!(prompt.ends_with(" voice=\"42s\">\nShip it.\n</sent>"), "{prompt}");

    // A short one with nothing said vanishes; a failed one can be discarded.
    seen.request(&mut client, Request::SelectChat(v1::SelectChat { chat_id: id.clone() })).await;
    let asked = requests.lock().expect("lock").len();
    let short = upload_for_chat(&mut client, &id, "Voice 2.m4a", "silent 4999").await;
    seen.request(&mut client, chat_voice(&id, short.clone())).await;
    let silent = upload_for_chat(&mut client, &id, "Voice 3.m4a", "silent 5000").await;
    seen.request(&mut client, chat_voice(&id, silent.clone())).await;
    seen.follow(&mut client, |seen| {
        seen.queued.len() == 1 && seen.queued[0].transcription.as_ref().is_some_and(|it| it.state() == v1::TranscriptionState::Failed)
    })
    .await;
    let discard = Request::DiscardVoiceMessage(v1::DiscardVoiceMessage { agent_id: id.clone(), message_id: seen.queued[0].id });
    assert!(matches!(seen.request(&mut client, discard).await, Body::Ack(_)));
    seen.follow(&mut client, |seen| seen.queued.is_empty() && seen.settled(&id)).await;
    assert_eq!(requests.lock().expect("lock").len(), asked, "the chat was given neither");
    // Neither recording is kept: a chat has nowhere to show it (row 752).
    assert!(!Path::new(&short.path).exists() && !Path::new(&silent.path).exists());
    assert!(Path::new(&recorded.path).exists(), "the one heard stays, to play back");

    // Without the model, a chat's voice message is refused as an agent's is.
    let gone = seen.request(&mut client, Request::RemoveSpeechModel(v1::RemoveSpeechModel {})).await;
    assert!(matches!(gone, Body::Ack(_)), "{gone:?}");
    let later = upload_for_chat(&mut client, &id, "Voice 4.m4a", "Hello.").await;
    assert_eq!(refusal(&client.request(chat_voice(&id, later)).await.body), "Voice messages need the speech model. Download it first.");
}

/// #401: on a weak connection the app asks again for what it never heard
/// answered. A dictation's upload that finished answers as it did, and the
/// same TranscribeAudio from the next connection waits with the first for
/// its words, or gets them at once: never uploaded or transcribed twice.
#[tokio::test]
async fn a_dictation_asked_again_after_a_drop_gets_its_words_without_uploading_again() {
    let dir = temp_dir();
    let (addr, _factory, _client, _view, _server) = ready_at(&dir, Scripted::signed_in()).await;
    let said = "wait Five minutes of words.";
    let (mut first, _, _) = Client::connect(addr).await;
    dictation(&mut first, "d1", said).await;

    // FinishUpload's answer lost: started again, it's all there, and
    // finishing again answers as before.
    let start = v1::StartUpload {
        upload_id: "d1".into(),
        name: "dictation.m4a".into(),
        size: said.len() as u64,
        mime: "audio/mp4".into(),
        purpose: v1::UploadPurpose::Dictation.into(),
        ..Default::default()
    };
    match first.request(Request::StartUpload(start)).await.body {
        Body::UploadProgress(progress) => assert_eq!(progress.received, said.len() as u64, "all there"),
        other => panic!("{other:?}"),
    }
    let finish = v1::FinishUpload { upload_id: "d1".into(), sha256: String::new() };
    assert!(matches!(first.request(Request::FinishUpload(finish)).await.body, Body::UploadedFile(_)));
    assert_eq!(kept_dictations(&dir).len(), 1, "kept once");

    // The connection drops while it's transcribed; the next asks again.
    let request = v1::TranscribeAudio { request_id: "r1".into(), upload_id: "d1".into() };
    first.ok(Request::TranscribeAudio(request.clone())).await;
    drop(first);
    let (mut second, _, _) = Client::connect(addr).await;
    let events = second.ok(Request::TranscribeAudio(request.clone())).await;
    assert!(!events.iter().any(|message| matches!(message.body, Some(Body::AudioTranscribed(_)))), "still being heard");
    let recording = dir.join("home/speech/dictation").join(&kept_dictations(&dir)[0]);
    std::fs::write(format!("{}.go", recording.display()), "").expect("go");
    let heard = second
        .wait_for(|body| match body {
            Body::AudioTranscribed(heard) if heard.request_id == "r1" => Some(heard.clone()),
            _ => None,
        })
        .await;
    assert_eq!(of(&heard), answer("Five minutes of words.", "", 41_200));

    // Asked again later, from yet another: the words at once, not heard again.
    let (mut third, _, _) = Client::connect(addr).await;
    assert_eq!(of(&transcribe(&mut third, "r1", "d1").await), answer("Five minutes of words.", "", 41_200));
    let starts = std::fs::read_to_string(dir.join("starts")).expect("starts");
    assert!(!recording.exists(), "the recording is deleted");
    // Another request for the same recording: it's gone, to upload again.
    assert_eq!(of(&transcribe(&mut third, "r2", "d1").await), answer("", "no_recording", 0));
    assert_eq!(std::fs::read_to_string(dir.join("starts")).expect("starts"), starts);
}

/// #401: a message sent again with the same client_id (its answer lost on
/// a weak connection) is answered as sent and delivered once: typed or
/// spoken, to an agent or a chat, and a new chat isn't made twice.
#[tokio::test]
async fn a_message_sent_again_is_delivered_once() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (_factory, mut client, mut view, _server) = ready(&dir, models).await;
    let delivered = |text: &str| -> usize {
        // What reached the agent or the chat; not a chat's title, which
        // quotes its first exchange.
        let requests = requests.lock().expect("lock");
        let read = requests.iter().filter(|request| !request.system_prompt.starts_with("Write a title"));
        read.map(|request| request.prompt.matches(text).count()).sum()
    };
    let user = |view: &View, text: &str| {
        view.entries.iter().filter(|entry| matches!(&entry.kind, Some(Kind::User(user)) if user.text == text)).count()
    };

    // Typed.
    let typed = Request::SendMessage(v1::SendMessage {
        agent_id: "ops".into(),
        text: "Deploy at five.".into(),
        client_id: "c1".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, typed.clone()).await;
    apply_request(&mut client, &mut view, typed).await;
    view.follow(&mut client, |view| user(view, "Deploy at five.") == 1 && idle(view)).await;

    // Spoken: its recording is the agent's already, and isn't needed again.
    let recorded = recording(&dir, "ops", "Voice.m4a", "Call me back.");
    let spoken = Request::SendMessage(v1::SendMessage {
        agent_id: "ops".into(),
        voice: true,
        attachments: vec![recorded.clone()],
        client_id: "c2".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, spoken.clone()).await;
    view.follow(&mut client, |view| voice_entry(view, "Call me back.").is_some() && idle(view)).await;
    std::fs::remove_file(&recorded.path).expect("moved away by the agent, say");
    match client.request(spoken).await.body {
        Body::Ack(_) => {}
        other => panic!("a repeat is answered as sent: {other:?}"),
    }
    // Another message after them settles what reached the agent.
    apply_request(&mut client, &mut view, send_message("ops", "Thanks.")).await;
    view.follow(&mut client, |view| user(view, "Thanks.") == 1 && idle(view)).await;
    assert_eq!((user(&view, "Deploy at five."), user(&view, "Call me back.")), (1, 1));
    assert_eq!((delivered("Deploy at five."), delivered("Call me back.")), (1, 1));
    assert_eq!(view.queued.as_ref().map_or(0, |queued| queued.messages.len()), 0);

    // A chat: made once, and its message sent once.
    let mut seen = ChatSeen { workspace: view.workspace.clone(), ..ChatSeen::default() };
    let settings =
        v1::UpdateSettings { default_model: Some(crate::common::MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    seen.request(&mut client, Request::UpdateSettings(settings)).await;
    let create = Request::CreateChat(v1::CreateChat { text: "Plan the week.".into(), client_id: "c3".into() });
    seen.request(&mut client, create.clone()).await;
    let id = seen.workspace.chats.first().expect("the chat").id.clone();
    seen.follow(&mut client, |seen| seen.settled(&id)).await;
    seen.request(&mut client, create).await;
    assert_eq!(seen.workspace.chats.len(), 1, "no second chat");
    let more = Request::SendChatMessage(v1::SendChatMessage {
        chat_id: id.clone(),
        text: "And Friday.".into(),
        client_id: "c4".into(),
        ..Default::default()
    });
    seen.request(&mut client, more.clone()).await;
    seen.request(&mut client, more).await;
    seen.follow(&mut client, |seen| seen.settled(&id)).await;
    seen.request(&mut client, Request::SendChatMessage(v1::SendChatMessage { chat_id: id.clone(), text: "Done.".into(), ..Default::default() }))
        .await;
    seen.follow(&mut client, |seen| seen.settled(&id)).await;
    assert_eq!((delivered("Plan the week."), delivered("And Friday."), delivered("Done.")), (1, 1, 1));
}

/// #425: what the person typed or pasted and a voice note go as one
/// message: it waits with the typed text, the agent reads the typed text,
/// then the words heard after "Voice note: ", and the transcript keeps the
/// two apart. Failed, nothing typed is lost; a chat takes them alike.
#[tokio::test]
async fn typed_text_and_a_voice_note_go_as_one_message() {
    let dir = temp_dir();
    let models = Scripted::signed_in();
    let requests = models.requests.clone();
    let (_factory, mut client, mut view, _server) = ready(&dir, models).await;
    let typed = "Notes from the call:\n- budget is fine";
    let said = "Ship it on Friday.";
    let whole = format!("{typed}\n\nVoice note: {said}");

    // It waits with what was typed, the recording first and a file after.
    let recorded = recording(&dir, "ops", "Voice.m4a", &format!("wait {said}"));
    let notes = v1::Attachment { mime: "text/plain".into(), ..recording(&dir, "ops", "notes.txt", "agenda") };
    let send = Request::SendMessage(v1::SendMessage {
        agent_id: "ops".into(),
        text: format!("  {typed}\n"),
        voice: true,
        attachments: vec![recorded.clone(), notes],
        client_id: "c1".into(),
        ..Default::default()
    });
    apply_request(&mut client, &mut view, send.clone()).await;
    view.follow(&mut client, |view| view.queued.as_ref().is_some_and(|queued| !queued.messages.is_empty())).await;
    let waiting = view.queued.as_ref().expect("the queue").messages[0].clone();
    assert_eq!((waiting.text.as_str(), waiting.typed.as_str()), (typed, typed));
    assert_eq!(waiting.transcription.as_ref().map(|it| it.state()), Some(v1::TranscriptionState::Pending));
    assert_eq!(waiting.attachments.len(), 2);

    // Heard: one message, the typed text first; the entry keeps both parts.
    std::fs::write(format!("{}.go", recorded.path), "").expect("go");
    view.follow(&mut client, |view| voice_entry(view, &whole).is_some() && idle(view)).await;
    let user = view
        .entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Some(Kind::User(user)) if user.text == whole => Some(user.clone()),
            _ => None,
        })
        .expect("the message");
    assert_eq!((user.typed.as_str(), user.heard.as_str()), (typed, said));
    assert_eq!(user.voice.as_ref().map(|it| (it.state(), it.duration_ms)), Some((v1::TranscriptionState::Done, 41_200)));
    assert_eq!(user.attachments.iter().map(|it| it.name.as_str()).collect::<Vec<_>>(), ["Voice.m4a", "notes.txt"]);
    let prompt = requests.lock().expect("lock").last().map(|request| request.prompt.clone()).expect("a request");
    assert!(prompt.contains(&format!(" voice=\"42s\">\n{whole}")), "{prompt}");
    assert!(prompt.contains("notes.txt"), "the file comes as usual: {prompt}");
    // Sent again (its answer lost): delivered once.
    assert!(matches!(client.request(send).await.body, Body::Ack(_)));
    assert_eq!(requests.lock().expect("lock").iter().filter(|request| request.prompt.contains(said)).count(), 1);

    // Nothing heard: it fails, keeping what was typed, even when it's short
    // (alone, that would vanish); tried again, it goes through.
    let silent = recording(&dir, "ops", "Voice 2.m4a", "silent 1000");
    let send = Request::SendMessage(v1::SendMessage {
        agent_id: "ops".into(),
        text: "Keep this.".into(),
        voice: true,
        attachments: vec![silent.clone()],
        ..Default::default()
    });
    apply_request(&mut client, &mut view, send).await;
    view.follow(&mut client, |view| queued(view) == [("Keep this.".to_owned(), Some(v1::TranscriptionState::Failed))]).await;
    let failed = view.queued.as_ref().expect("the queue").messages[0].clone();
    assert_eq!(failed.typed, "Keep this.");
    let db = rusqlite::Connection::open(dir.join("home/factory.db")).expect("factory.db");
    let (text, kept): (String, String) =
        db.query_row("SELECT text, voice FROM messages WHERE id = ?1", [failed.id], |row| Ok((row.get(0)?, row.get(1)?))).expect("kept");
    assert_eq!(text, "Keep this.");
    assert!(kept.contains("\"typed\":\"Keep this.\"") && kept.contains("\"failed\""), "{kept}");
    std::fs::write(&silent.path, "And this.").expect("rerecorded");
    let retry = Request::RetryVoiceMessage(v1::RetryVoiceMessage { agent_id: "ops".into(), message_id: failed.id });
    apply_request(&mut client, &mut view, retry).await;
    view.follow(&mut client, |view| voice_entry(view, "Keep this.\n\nVoice note: And this.").is_some() && idle(view)).await;

    // A voice message alone is as before: its text is what was heard.
    apply_request(&mut client, &mut view, voice("ops", recording(&dir, "ops", "Voice 3.m4a", "Just this."))).await;
    view.follow(&mut client, |view| voice_entry(view, "Just this.").is_some() && idle(view)).await;
    let alone = view.entries.iter().find_map(|entry| match &entry.kind {
        Some(Kind::User(user)) if user.text == "Just this." => Some(user.clone()),
        _ => None,
    });
    assert_eq!(alone.map(|user| (user.typed, user.heard)), Some((String::new(), String::new())));

    // A chat: the same, and still no files but the recording.
    let mut seen = ChatSeen { workspace: view.workspace.clone(), ..ChatSeen::default() };
    let settings =
        v1::UpdateSettings { default_model: Some(crate::common::MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    seen.request(&mut client, Request::UpdateSettings(settings)).await;
    seen.request(&mut client, Request::CreateChat(v1::CreateChat::default())).await;
    let id = seen.workspace.chats.first().expect("the chat").id.clone();
    let recorded = upload_for_chat(&mut client, &id, "Voice.m4a", "Plan it.").await;
    let extra = upload_for_chat(&mut client, &id, "Voice 2.m4a", "More.").await;
    let both = v1::SendChatMessage {
        chat_id: id.clone(),
        text: "From my notes.".into(),
        voice: true,
        attachments: vec![recorded.clone(), extra],
        ..Default::default()
    };
    assert_eq!(refusal(&client.request(Request::SendChatMessage(both)).await.body), "A voice message carries one recording.");
    let one = v1::SendChatMessage {
        chat_id: id.clone(),
        text: "From my notes.".into(),
        voice: true,
        attachments: vec![recorded],
        ..Default::default()
    };
    seen.request(&mut client, Request::SendChatMessage(one)).await;
    seen.follow(&mut client, |seen| seen.settled(&id) && seen.chat(&id).is_some_and(|chat| !chat.title.is_empty())).await;
    let read = requests
        .lock()
        .expect("lock")
        .iter()
        .any(|request| request.system_prompt.starts_with("You're a chat") && request.prompt.contains("From my notes.\n\nVoice note: Plan it."));
    assert!(read, "the chat reads both");
}
