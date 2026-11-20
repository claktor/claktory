//! The Usage pane's numbers: what agents' model calls used, by provider,
//! model and agent, for a time range the app asks for (LoadUsage).

use factory_protocol::v1::{self, client_message::Body as Request, server_message::Body};

use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, create_project, send_message, start_with, temp_dir};

fn idle(view: &View, agent: &str) -> bool {
    let agent = view.agent(agent);
    agent.status() == v1::AgentStatus::Idle && agent.queued_messages == 0
}

async fn load(client: &mut Client, view: &mut View, request: v1::LoadUsage) -> v1::UsageReport {
    match apply_request(client, view, Request::LoadUsage(request)).await {
        Body::UsageReport(report) => report,
        other => panic!("expected a usage report, got {other:?}"),
    }
}

#[tokio::test]
async fn usage_is_broken_down_by_agent_and_model_and_adds_up() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_in()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let empty = load(&mut client, &mut view, v1::LoadUsage::default()).await;
    assert!(empty.rows.is_empty() && empty.agents_since_ms == 0, "{empty:?}");

    apply_request(&mut client, &mut view, create_project("Site", &dir.join("site"))).await;
    for name in ["Writer", "Builder"] {
        apply_request(&mut client, &mut view, create_agent("site", name, "")).await;
    }
    for (agent, times) in [("writer", 2), ("builder", 1)] {
        for _ in 0..times {
            apply_request(&mut client, &mut view, Request::SelectAgent(v1::SelectAgent { agent_id: agent.into() })).await;
            let before = view.entries.len();
            apply_request(&mut client, &mut view, send_message(agent, "hello")).await;
            view.follow(&mut client, |view| view.entries.len() > before + 1 && idle(view, agent)).await;
        }
    }

    let now = factory_agent::clock::now_ms() as i64;
    let day = 24 * 3_600_000;
    let report =
        load(&mut client, &mut view, v1::LoadUsage { since_ms: now - day, until_ms: 0, bucket_ms: 3_600_000, utc_offset_minutes: 0 }).await;
    let (provider, model) = MODEL.split_once('/').expect("provider/model");
    let mut rows: Vec<(&str, &str, &str, &str, u64)> = report
        .rows
        .iter()
        .map(|row| (row.agent_id.as_str(), row.agent_name.as_str(), row.provider_id.as_str(), row.model.as_str(), row.requests))
        .collect();
    rows.sort_unstable();
    assert_eq!(rows, [("builder", "Builder", provider, model, 1), ("writer", "Writer", provider, model, 2)]);
    let sum = |tokens: Vec<v1::UsageTokens>| {
        tokens.iter().map(|tokens| tokens.input + tokens.output + tokens.cache_read + tokens.cache_write).sum::<u64>()
    };
    let by_rows = sum(report.rows.iter().map(|row| row.tokens.unwrap_or_default()).collect());
    let by_hours = sum(report.series.iter().map(|bucket| bucket.tokens.unwrap_or_default()).collect());
    assert!(by_rows > 0 && by_rows == by_hours, "the rows and the series add up: {by_rows} {by_hours}");
    assert_eq!(report.series.len(), 25, "every hour of the day, the one under way too");
    assert!(report.agents_since_ms > now - 2 * 3_600_000);

    // Renamed, then deleted: its use stays, under the name it last had.
    let rename = v1::UpdateAgent { agent_id: "builder".into(), name: Some("Mason".into()), ..Default::default() };
    apply_request(&mut client, &mut view, Request::UpdateAgent(rename)).await;
    let before = view.entries.len();
    apply_request(&mut client, &mut view, send_message("builder", "again")).await;
    view.follow(&mut client, |view| view.entries.len() > before + 1 && idle(view, "builder")).await;
    apply_request(&mut client, &mut view, Request::DeleteAgent(v1::DeleteAgent { agent_id: "builder".into() })).await;
    let report = load(&mut client, &mut view, v1::LoadUsage::default()).await;
    let builder = report.rows.iter().find(|row| row.agent_id == "builder").expect("its use stays");
    assert_eq!((builder.agent_name.as_str(), builder.requests), ("Mason", 2));

    match apply_request(&mut client, &mut view, Request::LoadUsage(v1::LoadUsage { bucket_ms: 1_000, ..Default::default() })).await {
        Body::Error(error) => assert_eq!(error.message, "bucket_ms is a whole number of hours, or 0 for no series"),
        other => panic!("expected an error, got {other:?}"),
    }
}
