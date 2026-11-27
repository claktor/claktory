//! Report types as data: the person and an
//! untainted Rivet define types within the rules in code; each pipe forwards
//! its own types, and an agent limited to several pipes can forward each one's;
//! an edit makes a new version, and a report stays checked and shown
//! against its own; a reader answers only what its report's type allows;
//! and every change shows in Needs you's history.

use std::sync::Arc;

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_project, start_with_models, temp_dir};
use crate::pipes::{call, idle, select, shown};
use factory_protocol::v1::{self, client_message::Body as Request, entry::Kind, server_message};
use factoryd::factory::Factory;
use factoryd::pipes;

/// Site with Lea, Ingrid, Scout, Dev (the Pipes extension, not an admin) and
/// Rivet (the admin agent); a pipe with its liaison, and Scout limited to it
/// and the only member of a second pipe; reports' wakes at once.
async fn setup() -> (Arc<Factory>, Client, View, String, String) {
    let dir = temp_dir();
    std::fs::create_dir_all(dir.join("home")).expect("home");
    let config = serde_json::json!({
        "agents": [
            {"id": "rivet", "name": "Rivet", "model": MODEL, "effort": "off", "extensions": {"agents": {"admin": true}}},
            {"id": "dev", "name": "Dev", "model": MODEL, "effort": "off", "extensions": {"pipes": {"enabled": true}}},
        ],
    });
    std::fs::write(dir.join("home/config.json"), config.to_string()).expect("config");
    let (addr, factory) = start_with_models(&dir, Arc::new(Scripted::signed_in())).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Lea", "Ingrid", "Scout"] {
        apply_request(&mut client, &mut view, crate::common::create_agent("site", name, "")).await;
    }
    let settings = v1::UpdateSettings { default_model: Some(MODEL.into()), default_effort: Some("off".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateSettings(settings)).await;
    pipes::set_wake_every(&factory, std::time::Duration::ZERO);
    pipes::handoff::set_wake_every(&factory, std::time::Duration::ZERO);
    let first = pipes::open_for_test(&factory, "site", "Website", &["lea"], "lea", false);
    let add = v1::AddPipeLiaison { pipe_id: first.clone(), name: "Liaison".into() };
    apply_request(&mut client, &mut view, Request::AddPipeLiaison(add)).await;
    view.follow(&mut client, |view| shown(view, &first).is_some_and(|pipe| pipe.lead_id == "liaison")).await;
    // A pipe's liaison uses no other pipe: Scout, limited to each, is
    // on both.
    let members = shown(&view, &first).expect("first").member_ids;
    let both = v1::UpdatePipe {
        pipe_id: first.clone(),
        members: Some(v1::PipeMembers { agent_ids: members.into_iter().chain(["scout".to_owned()]).collect() }),
        quarantined: Some(v1::PipeMembers { agent_ids: vec!["liaison".into(), "scout".into()] }),
        ..Default::default()
    };
    apply_request(&mut client, &mut view, Request::UpdatePipe(both)).await;
    let second = pipes::open_for_test(&factory, "site", "Docs", &["scout"], "scout", false);
    let limited = v1::UpdatePipe { pipe_id: second.clone(), quarantined: Some(v1::PipeMembers { agent_ids: vec!["scout".into()] }), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdatePipe(limited)).await;
    view.follow(&mut client, |view| shown(view, &second).is_some_and(|pipe| pipe.quarantined_ids == ["scout"])).await;
    (factory, client, view, first, second)
}

/// `tool` through extension_call: a tool that came after the agent's context began.
async fn later(client: &mut Client, view: &mut View, agent: &str, tool: &str, args: &str) -> String {
    call(client, view, agent, "extension_call", &format!(r#"{{"tool": "{tool}", "args": {args}}}"#)).await
}

fn forward(pipe: &str, to: &str, types: &[&str]) -> Request {
    Request::SetPipeForward(v1::SetPipeForward {
        pipe_id: pipe.into(),
        recipient_id: to.into(),
        make_reader: true,
        type_ids: types.iter().map(|id| (*id).to_owned()).collect(),
        ..Default::default()
    })
}

async fn refused(client: &mut Client, view: &mut View, request: Request) -> String {
    match apply_request(client, view, request).await {
        server_message::Body::Error(error) => error.message,
        other => panic!("not refused: {other:?}"),
    }
}

async fn list(client: &mut Client, view: &mut View, pipe: &str) -> v1::PipeReportPage {
    let request = Request::ListPipeReports(v1::ListPipeReports { pipe_id: pipe.into(), ..Default::default() });
    match apply_request(client, view, request).await {
        server_message::Body::PipeReportPage(page) => page,
        other => panic!("{other:?}"),
    }
}

async fn history(client: &mut Client, view: &mut View) -> Vec<v1::NeedsYouRecord> {
    match apply_request(client, view, Request::ListNeedsYouHistory(v1::ListNeedsYouHistory::default())).await {
        server_message::Body::NeedsYouHistory(history) => history.records,
        other => panic!("{other:?}"),
    }
}

fn events(view: &View) -> Vec<String> {
    view.entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Some(Kind::Event(event)) => Some(event.text.clone()),
            _ => None,
        })
        .collect()
}

fn submit(pipe: &str, kind: &str, fields: serde_json::Value) -> String {
    serde_json::json!({"pipe": pipe, "type": kind, "fields": fields, "token": format!("{kind}-{fields}")}).to_string()
}

fn field(label: &str, kind: v1::ReportFieldType) -> v1::ReportField {
    v1::ReportField { label: label.into(), r#type: kind as i32, ..Default::default() }
}

fn recipe(id: &str, extra: Vec<v1::ReportField>) -> v1::ReportSchema {
    let mut fields = vec![
        v1::ReportField { name: if id.is_empty() { String::new() } else { "dish".into() }, required: true, ..field("Dish", v1::ReportFieldType::Text) },
        v1::ReportField {
            name: if id.is_empty() { String::new() } else { "serves".into() },
            min: Some(1),
            max: Some(12),
            ..field("Serves", v1::ReportFieldType::Number)
        },
        v1::ReportField {
            name: if id.is_empty() { String::new() } else { "course".into() },
            choices: vec!["starter".into(), "main".into()],
            ..field("Course", v1::ReportFieldType::Enum)
        },
    ];
    fields.extend(extra);
    v1::ReportSchema {
        id: id.into(),
        name: "Recipe".into(),
        fields,
        outcomes: vec![v1::PipeReportState::Accepted as i32, v1::PipeReportState::Done as i32],
        ..Default::default()
    }
}

fn save(schema: v1::ReportSchema, base: u32) -> Request {
    Request::SaveReportType(v1::SaveReportType { r#type: Some(schema), base_version: base })
}

#[tokio::test]
async fn each_pipe_forwards_its_own_types_and_a_reader_answers_only_what_a_type_allows() {
    let (factory, mut client, mut view, first, second) = setup().await;
    // Built in: Bug report first, then the starters; and the rules in code.
    let ids: Vec<&str> = view.workspace.report_schemas.iter().map(|schema| schema.id.as_str()).collect();
    assert_eq!(ids, ["bug_report", "task_request", "question", "file_share", "status_update", "feature_idea"]);
    assert!(view.workspace.report_schemas.iter().all(|schema| schema.builtin && schema.version == 1));
    let rules = view.workspace.report_type_rules.clone().expect("rules");
    assert_eq!((rules.text_bytes, rules.fields, rules.facts, rules.choices, rules.per_pipe, rules.types), (4096, 20, 12, 32, 10, 50));
    assert!(rules.reserved_ids.contains(&"recipient".to_owned()));

    apply_request(&mut client, &mut view, forward(&first, "ingrid", &["bug_report", "task_request"])).await;
    view.follow(&mut client, |view| view.agent("ingrid").reads_reports).await;
    apply_request(&mut client, &mut view, forward(&second, "ingrid", &["question"])).await;
    let set = shown(&view, &first).and_then(|pipe| pipe.forward).expect("forward");
    assert_eq!((set.type_ids.clone(), set.schema_id.as_str(), set.revision), (vec!["bug_report".to_owned(), "task_request".to_owned()], "bug_report", 1));
    let said = refused(&mut client, &mut view, forward(&first, "ingrid", &["nothing_like_it"])).await;
    assert!(said.contains("There's no report type"), "{said}");

    // Scout forwards each pipe's types: not just its first pipe's.
    let task = serde_json::json!({"title": "Translate the page", "needed": "In Italian", "priority": "high", "due": "2026-12-01"});
    let said = later(&mut client, &mut view, "scout", "handoff_submit", &submit(&first, "task_request", task.clone())).await;
    assert_eq!(said, "accepted #1");
    let tools = pipes::handoff::submit_parameters_for_test(&factory, "scout").expect("handoff_submit");
    assert!(tools.contains("task_request") && tools.contains("question") && tools.contains("anyOf"), "{tools}");
    let question = serde_json::json!({"question": "Is there a style guide?", "urgency": "today"});
    assert_eq!(later(&mut client, &mut view, "scout", "handoff_submit", &submit(&first, "question", question.clone())).await, "refused: type");
    assert_eq!(later(&mut client, &mut view, "scout", "handoff_submit", &submit(&second, "question", question)).await, "accepted #1");
    let bad_day = serde_json::json!({"title": "x", "needed": "y", "priority": "low", "due": "2026-02-30"});
    assert_eq!(later(&mut client, &mut view, "scout", "handoff_submit", &submit(&first, "task_request", bad_day)).await, "refused: date due");
    // A refusal for the type keeps no type: nothing Scout sent.
    let page = list(&mut client, &mut view, &first).await;
    let refusal = page.reports.iter().find(|report| report.refused_code == "type").expect("refused");
    assert_eq!(refusal.schema_id, "");

    // Turning a type off holds nothing and keeps the revision; new ones of it are refused.
    apply_request(&mut client, &mut view, forward(&first, "ingrid", &["bug_report"])).await;
    let set = shown(&view, &first).and_then(|pipe| pipe.forward).expect("forward");
    assert_eq!((set.type_ids, set.revision), (vec!["bug_report".to_owned()], 1));
    let page = list(&mut client, &mut view, &first).await;
    let kept = page.reports.iter().find(|report| report.number == 1).expect("#1");
    assert_eq!((kept.state(), kept.schema_id.as_str(), kept.type_version), (v1::PipeReportState::New, "task_request", 1));
    assert_eq!(later(&mut client, &mut view, "scout", "handoff_submit", &submit(&first, "task_request", task)).await, "refused: type");
    view.follow(&mut client, |view| events(view).iter().any(|text| text.contains("now forwards \"Bug report\""))).await;
    // With handoff_submit as it is now, for an agent whose tool list is older.
    let told = events(&view).into_iter().rev().find(|text| text.contains("now forwards \"Bug report\"")).expect("told");
    let both = told.contains(r#""enum":["bug_report","question"]"#) || told.contains(r#""enum":["question","bug_report"]"#);
    assert!(told.contains("through extension_call") && both && !told.contains("task_request"), "{told}");

    // The reader reads its type and what it may answer, and only that.
    apply_request(&mut client, &mut view, select("ingrid")).await;
    view.follow(&mut client, |view| idle(view, "ingrid")).await;
    let facts = call(&mut client, &mut view, "ingrid", "handoff_read", r#"{"text": false, "since": 0}"#).await;
    assert!(facts.contains("\"Task request\" (type task_request, version 1)") && facts.contains("due: 2026-12-01") && facts.contains("priority: high"), "{facts}");
    assert!(facts.contains("Its answers: accepted, done, declined, known, needs_info.") && !facts.contains("Translate"), "{facts}");
    assert_eq!(pipes::taint_of_for_test(&factory, "ingrid"), "{}");
    let id = kept.id;
    let wrong = call(&mut client, &mut view, "ingrid", "handoff_mark", &format!(r#"{{"report": {id}, "status": "fixed", "fixed_in": "v0.1.16"}}"#)).await;
    assert!(wrong.contains("can be marked accepted, done, declined, known, needs_info"), "{wrong}");
    let done = call(&mut client, &mut view, "ingrid", "handoff_mark", &format!(r#"{{"report": {id}, "status": "done"}}"#)).await;
    assert!(done.ends_with("done."), "{done}");
    let status = later(&mut client, &mut view, "scout", "handoff_status", &format!(r#"{{"pipe": "{first}"}}"#)).await;
    assert_eq!(status, "#1: done");
    let page = list(&mut client, &mut view, &first).await;
    assert_eq!(page.reports.iter().find(|report| report.number == 1).map(|report| report.state()), Some(v1::PipeReportState::Done));

    // History: turning types on and off on a pipe, with what they let cross.
    let records = history(&mut client, &mut view).await;
    let forwards: Vec<_> = records.iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::PipeForward && record.pipe_id == first).collect();
    let titles: Vec<&str> = forwards.iter().map(|record| record.title.as_str()).collect();
    assert!(titles.contains(&"You stopped this pipe forwarding \u{201C}Task request\u{201D}"), "{titles:?}");
    let on = forwards.iter().find(|record| record.title == "You let this pipe forward \u{201C}Task request\u{201D}").expect("on");
    assert_eq!(on.report_types.iter().map(|schema| (schema.id.as_str(), schema.fields.len())).collect::<Vec<_>>(), [("task_request", 6)]);
}

#[tokio::test]
async fn the_person_defines_a_type_and_its_old_reports_keep_their_version() {
    let (_factory, mut client, mut view, first, _second) = setup().await;
    // Every rule in code, whoever asks.
    let mut bad = recipe("", vec![v1::ReportField { name: "to".into(), ..field("Send to", v1::ReportFieldType::Text) }]);
    assert!(refused(&mut client, &mut view, save(bad.clone(), 0)).await.contains("where the report goes"));
    bad.fields.pop();
    bad.fields[2].choices = (0..33).map(|index| format!("c{index}")).collect();
    assert!(refused(&mut client, &mut view, save(bad.clone(), 0)).await.contains("1 to 32 choices"));
    bad.fields[2].choices = vec!["starter".into()];
    bad.fields[0].max_bytes = 4097;
    assert!(refused(&mut client, &mut view, save(bad, 0)).await.contains("at most 4,096 bytes"));
    let said = refused(&mut client, &mut view, Request::DeleteReportType(v1::DeleteReportType { id: "bug_report".into() })).await;
    assert!(said.contains("built in"), "{said}");

    apply_request(&mut client, &mut view, save(recipe("", Vec::new()), 0)).await;
    let made = view.workspace.report_schemas.iter().find(|schema| schema.id == "recipe").cloned().expect("recipe");
    assert_eq!((made.version, made.builtin, made.by_id.as_str()), (1, false, ""));
    let fields: Vec<(&str, bool)> = made.fields.iter().map(|field| (field.name.as_str(), field.fact)).collect();
    assert_eq!(fields, [("dish", false), ("serves", true), ("course", true)]);
    assert_eq!((made.fields[1].min, made.fields[1].max, made.fields[1].rule.as_str()), (Some(1), Some(12), "A whole number from 1 to 12"));

    apply_request(&mut client, &mut view, forward(&first, "ingrid", &["recipe"])).await;
    let dish = serde_json::json!({"dish": "Risotto", "serves": 4, "course": "main"});
    assert_eq!(later(&mut client, &mut view, "liaison", "handoff_submit", &submit(&first, "recipe", dish)).await, "accepted #1");
    let many = serde_json::json!({"dish": "Risotto", "serves": 13, "course": "main"});
    assert_eq!(later(&mut client, &mut view, "liaison", "handoff_submit", &submit(&first, "recipe", many)).await, "refused: out_of_range serves");

    // An edit that lets more cross: a new version, and History says so.
    let mut wider = recipe("recipe", vec![field("Notes", v1::ReportFieldType::Text)]);
    wider.fields[2].choices.push("dessert".into());
    apply_request(&mut client, &mut view, save(wider.clone(), 1)).await;
    let edited = view.workspace.report_schemas.iter().find(|schema| schema.id == "recipe").cloned().expect("recipe");
    assert_eq!((edited.version, edited.widened_fields.clone()), (2, vec!["course".to_owned(), "notes".to_owned()]));
    let said = refused(&mut client, &mut view, save(wider, 1)).await;
    assert!(said.contains("changed after you opened it. Open it again to see version 2."), "{said}");
    view.follow(&mut client, |view| events(view).iter().any(|text| text.contains("now forwards \"Recipe\""))).await;

    // The report keeps version 1, shown against it.
    let page = list(&mut client, &mut view, &first).await;
    let report = page.reports.iter().find(|report| report.number == 1).expect("#1");
    assert_eq!((report.schema_id.as_str(), report.type_version), ("recipe", 1));
    assert!(report.values.iter().any(|value| value.field == "serves" && value.number == 4));
    assert_eq!(page.schemas.iter().map(|schema| (schema.id.as_str(), schema.version)).collect::<Vec<_>>(), [("recipe", 1)]);

    // Deleted only once nothing forwards it; its versions stay for its reports.
    let said = refused(&mut client, &mut view, Request::DeleteReportType(v1::DeleteReportType { id: "recipe".into() })).await;
    assert!(said.contains("“Recipe” is still forwarded by \"Website\". Turn it off there first."), "{said}");
    apply_request(&mut client, &mut view, forward(&first, "ingrid", &["bug_report"])).await;
    apply_request(&mut client, &mut view, Request::DeleteReportType(v1::DeleteReportType { id: "recipe".into() })).await;
    assert!(!view.workspace.report_schemas.iter().any(|schema| schema.id == "recipe"));
    let page = list(&mut client, &mut view, &first).await;
    assert_eq!(page.schemas.iter().map(|schema| (schema.id.as_str(), schema.version)).collect::<Vec<_>>(), [("recipe", 1)]);
    assert!(refused(&mut client, &mut view, forward(&first, "ingrid", &["recipe"])).await.contains("There's no report type"));

    let records = history(&mut client, &mut view).await;
    let kinds: Vec<_> = records.iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::ReportType).collect();
    let titles: Vec<&str> = kinds.iter().map(|record| record.title.as_str()).collect();
    assert_eq!(
        titles,
        ["You deleted the report type \u{201C}Recipe\u{201D}", "You changed the report type \u{201C}Recipe\u{201D}", "You made the report type \u{201C}Recipe\u{201D}"]
    );
    let changed = kinds[1];
    assert!(changed.widens && changed.outcome() == v1::NeedsYouOutcome::Changed);
    assert_eq!((changed.report_type.as_ref().map(|schema| schema.version), changed.report_type_before.as_ref().map(|schema| schema.version)), (Some(2), Some(1)));
    assert!(!kinds[2].widens && kinds[2].report_type_before.is_none());
    assert_eq!(kinds[0].report_type.as_ref().map(|schema| schema.version), Some(2));
}

#[tokio::test]
async fn rivet_defines_types_only_while_it_has_read_nothing_from_outside() {
    let (factory, mut client, mut view, first, _second) = setup().await;
    let define = r#"{"action": "define", "name": "Recipe", "fields": [{"label": "Dish", "kind": "text", "required": true}, {"label": "Serves", "kind": "number", "min": 1, "max": 12}], "outcomes": ["accepted", "done"]}"#;
    let dev = call(&mut client, &mut view, "dev", "pipes_report_type", define).await;
    assert!(dev.starts_with("Only an admin agent"), "{dev}");

    let routing = r#"{"action": "define", "name": "Sneaky", "fields": [{"id": "recipient", "label": "Who", "kind": "text"}], "outcomes": ["accepted"]}"#;
    let said = call(&mut client, &mut view, "rivet", "pipes_report_type", routing).await;
    assert!(said.contains("where the report goes"), "{said}");
    let said = call(&mut client, &mut view, "rivet", "pipes_report_type", define).await;
    assert!(said.starts_with("Kept as version 1.") && said.contains("- serves: \"Serves\", number 1..12, a fact"), "{said}");
    let made = view.workspace.report_schemas.iter().find(|schema| schema.id == "recipe").cloned().expect("recipe");
    assert_eq!(made.by_id, "rivet");
    let said = call(&mut client, &mut view, "rivet", "pipes_forward", &format!(r#"{{"pipe": "{first}", "to": "Ingrid", "make_reader": true, "types": ["recipe", "question"]}}"#)).await;
    assert!(said.contains("of the types \"Recipe\" and \"Question\""), "{said}");

    // Tainted, it can still look, but change nothing.
    pipes::taint_for_test(&factory, "rivet", "browser:https://example.com");
    let edit = r#"{"action": "edit", "id": "recipe", "name": "Recipe", "fields": [{"id": "dish", "label": "Dish", "kind": "text"}, {"label": "Send to", "kind": "text"}], "outcomes": ["accepted"]}"#;
    for args in [define, edit, r#"{"action": "delete", "id": "recipe"}"#] {
        let said = call(&mut client, &mut view, "rivet", "pipes_report_type", args).await;
        assert!(said.starts_with("Not while you've read a page on https://example.com"), "{said}");
    }
    let shown_types = call(&mut client, &mut view, "rivet", "pipes_report_type", r#"{"action": "show"}"#).await;
    assert!(shown_types.contains("\"Bug report\" (bug_report, version 1, built in)") && shown_types.contains("\"Recipe\" (recipe, version 1)"), "{shown_types}");
    assert_eq!(view.workspace.report_schemas.iter().find(|schema| schema.id == "recipe").map(|schema| schema.version), Some(1));

    let records = history(&mut client, &mut view).await;
    let made = records.iter().find(|record| record.kind() == v1::NeedsYouRecordKind::ReportType).expect("made");
    assert_eq!((made.agent_id.as_str(), made.title.as_str()), ("rivet", "Rivet made the report type \u{201C}Recipe\u{201D}"));
    let on = records.iter().find(|record| record.kind() == v1::NeedsYouRecordKind::PipeForward && record.title.contains("forward")).expect("on");
    // Bug report was on as the app showed it, until Rivet chose others.
    assert_eq!(on.title, "Rivet let this pipe forward \u{201C}Recipe\u{201D} and \u{201C}Question\u{201D} instead of \u{201C}Bug report\u{201D}");
}

/// A pipe's first save says only what the person changed from what the app
/// showed (no one, Bug report), and an edit that only adds a choice widens
/// what crosses but lets no more of their words through.
#[tokio::test]
async fn a_first_save_and_a_new_choice_say_only_what_changed() {
    let (_factory, mut client, mut view, first, _second) = setup().await;
    apply_request(&mut client, &mut view, save(recipe("", Vec::new()), 0)).await;
    assert!(shown(&view, &first).is_some_and(|pipe| pipe.forward.is_none()));
    let tick = v1::SetPipeForward { pipe_id: first.clone(), type_ids: vec!["bug_report".into(), "recipe".into()], ..Default::default() };
    apply_request(&mut client, &mut view, Request::SetPipeForward(tick)).await;
    let mut more = recipe("recipe", Vec::new());
    more.fields[2].choices.push("dessert".into());
    apply_request(&mut client, &mut view, save(more, 1)).await;
    let edited = view.workspace.report_schemas.iter().find(|schema| schema.id == "recipe").cloned().expect("recipe");
    assert_eq!((edited.version, edited.widened_fields.clone()), (2, vec!["course".to_owned()]));

    let records = history(&mut client, &mut view).await;
    let forwards: Vec<&str> =
        records.iter().filter(|record| record.kind() == v1::NeedsYouRecordKind::PipeForward).map(|record| record.title.as_str()).collect();
    assert_eq!(forwards, ["You let this pipe forward \u{201C}Recipe\u{201D}"]);
    let changed = records.iter().find(|record| record.title == "You changed the report type \u{201C}Recipe\u{201D}").expect("the edit");
    assert!(!changed.widens, "a choice carries no words of theirs");
}
