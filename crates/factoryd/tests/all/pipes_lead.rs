//! A pipe's lead: its liaison, else an agent
//! limited to the pipe, never one that can run commands unless the person
//! chose it; a new lead is told at once of what came while none was; and
//! with none, what arrives waits for the person.

use crate::common::{apply_request, create_agent, create_project};
use crate::common::View;
use crate::pipes::{events, incoming, select, setup, shown};
use factory_protocol::v1::{self, client_message::Body as Request};
use factoryd::pipes;

fn update(pipe: &str, members: Option<&[&str]>, lead: Option<&str>, limited: Option<&[&str]>) -> Request {
    let ids = |ids: &[&str]| v1::PipeMembers { agent_ids: ids.iter().map(|id| (*id).to_owned()).collect() };
    Request::UpdatePipe(v1::UpdatePipe {
        pipe_id: pipe.into(),
        members: members.map(ids),
        lead_id: lead.map(str::to_owned),
        quarantined: limited.map(ids),
        ..Default::default()
    })
}

/// Its lead and how many messages wait unanswered, as the app shows them;
/// None until it shows the pipe.
fn lead(view: &View, pipe: &str) -> Option<(String, usize)> {
    shown(view, pipe).map(|pipe| (pipe.lead_id, pipe.unanswered_seqs.len()))
}

fn lines(factory: &factoryd::factory::Factory, pipe: &str) -> Vec<String> {
    pipes::messages_for_test(factory, pipe)
        .into_iter()
        .filter(|message| message.author.as_ref().is_some_and(|author| author.kind() == v1::PipeAuthorKind::Claktory))
        .map(|message| message.text)
        .collect()
}

fn agent(name: &str, role: &str, prompt: &str) -> Request {
    let Request::CreateAgent(mut create) = create_agent("site", name, prompt) else { unreachable!() };
    create.role = role.into();
    Request::CreateAgent(create)
}

fn told(view: &View, text: &str) -> bool {
    events(view).iter().any(|event| event.text.contains(text))
}

#[tokio::test]
async fn a_pipe_gets_a_contained_lead_told_at_once_or_none_and_waits_for_the_person() {
    let (_dir, factory, mut client, mut view, pipe) = setup(false).await;
    // "No lead" with an agent limited to the pipe: it leads.
    apply_request(&mut client, &mut view, update(&pipe, None, Some(""), Some(&["bo"]))).await;
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(lead, _)| lead == "bo")).await;
    assert!(lines(&factory, &pipe).contains(&"Bo answers this pipe now: it's told of each message from Ana.".to_owned()), "{:?}", lines(&factory, &pipe));

    // Unticked, with only an agent that can run commands left: never chosen
    // for the person. No lead; what arrives waits for them, said once.
    apply_request(&mut client, &mut view, select("lea")).await;
    apply_request(&mut client, &mut view, update(&pipe, Some(&["lea"]), None, Some(&[]))).await;
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(lead, _)| lead.is_empty())).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Anyone there?")).expect("received");
    pipes::receive_for_test(&factory, &pipe, incoming("m2", "Hello?")).expect("received");
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(_, unanswered)| unanswered == 2)).await;
    let said: Vec<String> = lines(&factory, &pipe).into_iter().filter(|line| line.starts_with("No agent answers this pipe now")).collect();
    assert_eq!(said, ["No agent answers this pipe now: messages from Ana wait for you here. Choose its lead in the pipe's settings."]);
    assert!(!told(&view, "Anyone there?"), "Lea isn't woken");
    let section = pipes::prompt_for_test(&factory, "lea").expect("a section");
    assert!(section.contains("No agent is told of what arrives"), "{section}");

    // The person chooses Lea: she's told of both now, not at the next message.
    apply_request(&mut client, &mut view, update(&pipe, None, Some("lea"), None)).await;
    view.follow(&mut client, |view| lead(view, &pipe) == Some(("lea".to_owned(), 0))).await;
    view.follow(&mut client, |view| told(view, "Anyone there?") && told(view, "Hello?")).await;
}

#[tokio::test]
async fn a_pipe_left_without_a_lead_gets_its_liaison_at_start_then_another_contained_agent() {
    let (dir, factory, mut client, mut view, _) = setup(false).await;
    apply_request(&mut client, &mut view, agent("Kai", "Liaison", "You are the liaison for the pipe \"Bugs\".")).await;
    apply_request(&mut client, &mut view, agent("Mo", "Liaison", "")).await;
    // As an early pipe was: its liaison limited to it, and no lead.
    let pipe = pipes::open_for_test(&factory, "site", "Bugs", &["lea", "kai", "mo"], "", false);
    pipes::upgrade_for_test(&factory, &pipe);
    pipes::unbound_for_test(&factory, &pipe);
    pipes::receive_for_test(&factory, &pipe, incoming("m1", "Edoardo says hi")).expect("received");
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(_, unanswered)| unanswered == 1)).await;

    // Repaired at start: its liaison leads, and hears what came meanwhile.
    apply_request(&mut client, &mut view, select("kai")).await;
    pipes::start_for_test(&factory);
    view.follow(&mut client, |view| lead(view, &pipe) == Some(("kai".to_owned(), 0))).await;
    view.follow(&mut client, |view| told(view, "Edoardo says hi")).await;
    let section = pipes::prompt_for_test(&factory, "lea").expect("a section");
    assert!(section.contains("Kai, its lead, is told of each message"), "{section}");

    // Its lead deleted: the other agent limited to it leads.
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "kai".into() })).await;
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(lead, _)| lead == "mo")).await;
    assert!(lines(&factory, &pipe).contains(&"Mo answers this pipe now: it's told of each message from Ana.".to_owned()));

    // Moved to another project, it isn't the pipe's any more; Lea can run
    // commands, so no one leads, and Lea isn't woken.
    apply_request(&mut client, &mut view, create_project("Other", &dir.join("other"))).await;
    let other = view.workspace.projects.iter().find(|project| project.name == "Other").map(|project| project.id.clone()).expect("other");
    let moved = v1::UpdateAgent { agent_id: "mo".into(), project_id: Some(other), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(moved)).await;
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(lead, _)| lead.is_empty())).await;
    apply_request(&mut client, &mut view, select("lea")).await;
    pipes::receive_for_test(&factory, &pipe, incoming("m2", "Still there?")).expect("received");
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(_, unanswered)| unanswered == 1)).await;
    assert!(!told(&view, "Still there?"));
}

#[tokio::test]
async fn the_other_agents_hear_who_leads_now_before_their_next_step() {
    use crate::common::send_message;
    let (_dir, factory, mut client, mut view, _) = setup(false).await;
    apply_request(&mut client, &mut view, agent("Kai", "Liaison", "")).await;
    apply_request(&mut client, &mut view, agent("Mo", "Liaison", "")).await;
    let pipe = pipes::open_for_test(&factory, "site", "Bugs", &["lea", "kai", "mo"], "lea", false);
    pipes::upgrade_for_test(&factory, &pipe);
    // Mo's context begins while Lea leads.
    apply_request(&mut client, &mut view, select("mo")).await;
    apply_request(&mut client, &mut view, send_message("mo", "hello")).await;
    view.follow(&mut client, |view| crate::pipes::idle(view, "mo") && view.entries.len() >= 2).await;
    apply_request(&mut client, &mut view, update(&pipe, None, Some(""), None)).await;
    view.follow(&mut client, |view| lead(view, &pipe).is_some_and(|(lead, _)| lead == "kai")).await;
    // Its next step starts with what changed: Kai leads now.
    let before = view.entries.len();
    apply_request(&mut client, &mut view, send_message("mo", "again")).await;
    view.follow(&mut client, |view| crate::pipes::idle(view, "mo") && view.entries.len() >= before + 2).await;
    let told = view.entries[before..].iter().any(|entry| match &entry.kind {
        Some(v1::entry::Kind::Event(event)) => {
            event.text.starts_with("A pipe of yours changed") && event.text.contains("Kai, its lead, is told of each message")
        }
        _ => false,
    });
    assert!(told, "{:?}", &view.entries[before..]);
}
