//! Who may send urgent alerts, and alerts folded together: admin agents and
//! agents that lead others send urgent alerts unless the person chose
//! otherwise in Settings → Notifications; anyone else's goes as a normal
//! one, and it's told who may. Another agent's alert about the same thing,
//! while the first is unread, folds into it: no notification of its own, but
//! once when it makes it urgent.

use factory_protocol::v1::{self, client_message, server_message::Body};

use crate::common::{Client, View, apply_request, create_agent, create_project, start, temp_dir};
use crate::notify::call;

/// A Claktory with a project "Ops" and its agents, by name.
async fn setup(names: &[&str]) -> (std::path::PathBuf, std::sync::Arc<factoryd::Factory>, Client, View) {
    let dir = temp_dir();
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Ops", &dir.join("ops"))).await;
    for name in names {
        apply_request(&mut client, &mut view, create_agent("ops", name, "")).await;
    }
    (dir, factory, client, view)
}

fn alert(title: &str, urgent: bool) -> serde_json::Value {
    serde_json::json!({"title": title, "text": "Pericolo is checking.", "urgent": urgent})
}

fn reports_to(agent: &str, lead: &str) -> client_message::Body {
    client_message::Body::UpdateAgent(v1::UpdateAgent { agent_id: agent.into(), lead_id: Some(lead.into()), ..Default::default() })
}

fn urgent_from(senders: v1::UrgentSenders) -> client_message::Body {
    let change = v1::NotificationSettingsChange { urgent_from: Some(senders), ..Default::default() };
    client_message::Body::UpdateSettings(v1::UpdateSettings { notification_change: Some(change), ..Default::default() })
}

/// Who may send urgent alerts, as the Workspace says, and whether it's the
/// person's own choice.
fn shown(view: &View) -> (Vec<String>, bool) {
    let settings = view.workspace.settings.clone().expect("settings").notifications.expect("notifications");
    (settings.urgent_from, settings.urgent_from_custom)
}

fn ids(ids: &[&str]) -> Vec<String> {
    ids.iter().map(|id| (*id).to_owned()).collect()
}

fn last_alert<'a>(view: &'a View, title: &str) -> &'a v1::Alert {
    view.workspace.alerts.iter().rev().find(|alert| alert.title == title).expect("the alert")
}

#[tokio::test]
async fn only_admins_and_agents_that_lead_send_urgent_alerts_unless_the_person_chooses() {
    let (dir, factory, mut client, mut view) = setup(&["Builder", "Tester"]).await;
    let mut notes = Vec::new();
    assert_eq!(shown(&view), (ids(&["rivet"]), false), "Rivet, the admin agent, and nobody leads yet");

    // Builder leads nobody: its urgent alert goes as a normal one.
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("The site is down", true)).await;
    assert_eq!(
        said,
        "Sent as a normal alert: only Rivet may send urgent ones. Sent: the person gets it as a notification, and it waits in their Needs you until they read it."
    );
    assert!(!last_alert(&view, "The site is down").urgent);

    // Tester reports to Builder: the default follows who leads.
    apply_request(&mut client, &mut view, reports_to("tester", "builder")).await;
    assert_eq!(shown(&view), (ids(&["rivet", "builder"]), false));
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("The database is full", true)).await;
    assert_eq!(said, "Sent: the person gets it as a notification, and it waits in their Needs you until they read it.");
    assert!(last_alert(&view, "The database is full").urgent);

    // The person's own choice: Tester, and Rivet always.
    apply_request(&mut client, &mut view, urgent_from(v1::UrgentSenders { agent_ids: ids(&["tester", "tester"]), ..Default::default() }))
        .await;
    assert_eq!(shown(&view), (ids(&["rivet", "tester"]), true));
    let said = call(&mut client, &mut view, &mut notes, "builder", "notify_person", alert("The cache is cold", true)).await;
    assert!(said.starts_with("Sent as a normal alert: only Rivet and Tester may send urgent ones. Sent:"), "{said}");
    assert!(!last_alert(&view, "The cache is cold").urgent);
    let said = call(&mut client, &mut view, &mut notes, "tester", "notify_person", alert("Tests stopped running", true)).await;
    assert!(said.starts_with("Sent: "), "{said}");
    assert!(last_alert(&view, "Tests stopped running").urgent);

    // An id that isn't an agent's is refused, nothing changed.
    let refused =
        apply_request(&mut client, &mut view, urgent_from(v1::UrgentSenders { agent_ids: ids(&["nobody"]), ..Default::default() })).await;
    assert!(matches!(&refused, Body::Error(error) if error.message.contains("nobody")), "{refused:?}");
    assert_eq!(shown(&view), (ids(&["rivet", "tester"]), true));

    // An older app's whole settings leave it as it is.
    let whole = v1::NotificationSettings { needs_you: true, alerts: true, replies: true, ..Default::default() };
    apply_request(
        &mut client,
        &mut view,
        client_message::Body::UpdateSettings(v1::UpdateSettings { notifications: Some(whole), ..Default::default() }),
    )
    .await;
    assert_eq!(shown(&view), (ids(&["rivet", "tester"]), true));

    // Kept across a restart.
    drop(client);
    drop(factory);
    let (addr, factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(shown(&view), (ids(&["rivet", "tester"]), true));

    // Back to the default.
    apply_request(&mut client, &mut view, urgent_from(v1::UrgentSenders { use_default: true, agent_ids: ids(&["tester"]) })).await;
    assert_eq!(shown(&view), (ids(&["rivet", "builder"]), false));
    drop(factory);
}

#[tokio::test]
async fn alerts_from_several_agents_about_the_same_thing_reach_the_person_as_one() {
    let (dir, factory, mut client, mut view) = setup(&["Fugo", "Bruno", "Pericolo", "Tester"]).await;
    let mut notes = Vec::new();
    // Real alerts.
    let down = "tprime2 stopped answering after my memory test";
    let said = call(&mut client, &mut view, &mut notes, "fugo", "notify_person", alert(down, false)).await;
    assert!(said.starts_with("Sent: "), "{said}");
    assert_eq!(notes.len(), 1);
    let first = last_alert(&view, down).clone();

    let same = "tprime2 stopped answering during Fugo's memory test";
    let said = call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(same, false)).await;
    assert_eq!(
        said,
        format!(
            "Folded into Fugo's alert about the same thing, \"{down}\", still unread: the person sees you named on it, and no notification of its own was sent. It waits in their Needs you until they read it."
        )
    );
    assert_eq!(notes.len(), 1, "no notification of its own");
    let [folded] = view.workspace.alerts.as_slice() else { panic!("{:?}", view.workspace.alerts) };
    assert_eq!((folded.id.as_str(), folded.also_from.as_slice()), (first.id.as_str(), ids(&["Bruno"]).as_slice()));
    assert_eq!((folded.title.as_str(), folded.from_name.as_str()), (down, "Fugo"), "the first one's words");

    // Another concern doesn't fold.
    let disk = "Low disk is holding up the team's tests";
    let said = call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(disk, false)).await;
    assert!(said.starts_with("Sent: "), "{said}");
    assert_eq!((view.workspace.alerts.len(), notes.len()), (2, 2));

    // A third agent, then Bruno again: each named once, in order.
    call(&mut client, &mut view, &mut notes, "pericolo", "notify_person", alert(down, false)).await;
    call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(same, false)).await;
    assert_eq!(last_alert(&view, down).also_from, ids(&["Bruno", "Pericolo"]));
    assert_eq!((view.workspace.alerts.len(), notes.len()), (2, 2));

    // Urgent from one who may send urgent alerts: the alert turns urgent
    // and notifies once, as Fugo's.
    apply_request(&mut client, &mut view, reports_to("tester", "bruno")).await;
    let said = call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(same, true)).await;
    assert!(said.contains("Your alert made it urgent. Sent: the person gets it as a notification"), "{said}");
    assert_eq!(notes.len(), 3);
    let upgraded = notes.last().expect("the notification");
    assert_eq!((upgraded.alert_id.as_str(), upgraded.title.as_str()), (first.id.as_str(), "Fugo"));
    assert_eq!(upgraded.body, format!("{down}\nPericolo is checking."));
    assert!(last_alert(&view, down).urgent);
    // Urgent already: no second notification.
    let said = call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(same, true)).await;
    assert!(said.ends_with("It waits in their Needs you until they read it."), "{said}");
    assert_eq!(notes.len(), 3);
    // Fugo's own second alert never folds into its first.
    call(&mut client, &mut view, &mut notes, "fugo", "notify_person", alert(down, false)).await;
    assert_eq!((view.workspace.alerts.len(), notes.len()), (3, 4));

    // Kept across a restart.
    drop(client);
    drop(factory);
    let (addr, _factory) = start(&dir).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let kept = view.workspace.alerts.iter().find(|alert| alert.id == first.id).expect("kept");
    assert!(kept.urgent && kept.also_from == ids(&["Bruno", "Pericolo"]), "{kept:?}");

    // Once read, nothing folds into it.
    let read = v1::ReadAlerts { ids: view.workspace.alerts.iter().map(|alert| alert.id.clone()).collect() };
    apply_request(&mut client, &mut view, client_message::Body::ReadAlerts(read)).await;
    let mut notes = Vec::new();
    let said = call(&mut client, &mut view, &mut notes, "bruno", "notify_person", alert(same, false)).await;
    assert!(said.starts_with("Sent: "), "{said}");
    assert!(view.workspace.alerts.iter().all(|alert| alert.also_from.is_empty()), "{:?}", view.workspace.alerts);
}
