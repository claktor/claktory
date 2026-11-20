use crate::common::{Client, MODEL, Scripted, View, apply_request, create_agent, start_with, temp_dir};
use factory_protocol::v1::{self, ErrorCode, LoginPromptKind, client_message::Body as Request, server_message::Body};

fn start_login(method: &str) -> Request {
    Request::StartLogin(v1::StartLogin { provider_id: "scripted".into(), method_id: method.into(), ..Default::default() })
}

fn answer(text: &str) -> Request {
    Request::AnswerLogin(v1::AnswerLogin { text: text.into() })
}

fn asks(view: &View, kind: LoginPromptKind) -> bool {
    view.workspace.login.as_ref().and_then(|login| login.prompt.as_ref()).is_some_and(|prompt| prompt.kind() == kind)
}

fn expect_ack(reply: Body) {
    assert!(matches!(reply, Body::Ack(_)), "expected ack, got {reply:?}");
}

#[tokio::test]
async fn browser_sign_in_finishes_with_a_pasted_redirect_and_gives_agents_their_model() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    assert_eq!(view.workspace.providers[0].signed_in_with, "");
    assert!(view.workspace.models.is_empty());

    // Configured for a model nobody can run yet: the real state says so.
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;
    assert_eq!(view.agent("builder").model, MODEL);
    assert!(!view.agent("builder").model_available);

    expect_ack(apply_request(&mut client, &mut view, start_login("oauth")).await);
    let login = view.workspace.login.as_ref().expect("sign-in shown before the ack");
    assert_eq!((login.provider_id.as_str(), login.method_id.as_str()), ("scripted", "oauth"));

    // The URL to open, with the paste fallback waiting next to it.
    view.follow(&mut client, |view| asks(view, LoginPromptKind::Code)).await;
    assert_eq!(view.workspace.login.as_ref().expect("login").url, "https://scripted.test/authorize");

    // The browser was on another machine, so the user pastes where it landed.
    expect_ack(apply_request(&mut client, &mut view, answer("http://localhost:53692/callback?code=good&state=s")).await);
    view.follow(&mut client, |view| view.workspace.login.is_none()).await;
    let provider = &view.workspace.providers[0];
    assert_eq!((provider.signed_in_with.as_str(), provider.can_sign_out), ("Scripted account", true));
    assert_eq!(view.workspace.models.len(), 2);
    assert!(view.agent("builder").model_available);
}

#[tokio::test]
async fn a_rejected_key_stays_on_screen_until_dismissed_and_signing_out_takes_the_model_away() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    apply_request(&mut client, &mut view, create_agent("", "Builder", "")).await;

    apply_request(&mut client, &mut view, start_login("api_key")).await;
    view.follow(&mut client, |view| asks(view, LoginPromptKind::Secret)).await;
    apply_request(&mut client, &mut view, answer("sk-wrong")).await;
    view.follow(&mut client, |view| view.workspace.login.as_ref().is_some_and(|login| !login.error.is_empty())).await;
    let login = view.workspace.login.as_ref().expect("failed sign-in stays");
    assert!(login.error.contains("invalid API key"), "{}", login.error);
    assert!(login.prompt.is_none());

    // Nothing is asking any more.
    match apply_request(&mut client, &mut view, answer("sk-good")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    expect_ack(apply_request(&mut client, &mut view, Request::CancelLogin(v1::CancelLogin {})).await);
    assert!(view.workspace.login.is_none());

    // Try again, right this time.
    apply_request(&mut client, &mut view, start_login("api_key")).await;
    view.follow(&mut client, |view| asks(view, LoginPromptKind::Secret)).await;
    apply_request(&mut client, &mut view, answer("sk-good")).await;
    view.follow(&mut client, |view| view.workspace.login.is_none()).await;
    assert_eq!(view.workspace.providers[0].signed_in_with, "Scripted key");
    assert!(view.agent("builder").model_available);

    expect_ack(
        apply_request(&mut client, &mut view, Request::SignOut(v1::SignOut { provider_id: "scripted".into(), ..Default::default() })).await,
    );
    assert_eq!(view.workspace.providers[0].signed_in_with, "");
    assert!(view.workspace.models.is_empty());
    assert!(!view.agent("builder").model_available);
}

#[tokio::test]
async fn unknown_providers_and_methods_are_refused() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::signed_out()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    let unknown = Request::StartLogin(v1::StartLogin { provider_id: "nope".into(), method_id: "oauth".into(), ..Default::default() });
    match apply_request(&mut client, &mut view, unknown).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::NotFound),
        other => panic!("expected an error, got {other:?}"),
    }
    match apply_request(&mut client, &mut view, start_login("carrier-pigeon")).await {
        Body::Error(error) => assert_eq!(error.code(), ErrorCode::InvalidRequest),
        other => panic!("expected an error, got {other:?}"),
    }
    assert!(view.workspace.login.is_none());
}

#[tokio::test]
async fn a_subscription_shows_its_usage_until_it_signs_out_and_an_api_key_has_none() {
    let dir = temp_dir();
    let (addr, _factory) = start_with(&dir, Scripted::subscribed()).await;
    let (mut client, workspace, _) = Client::connect(addr).await;
    let mut view = View { workspace, ..View::default() };
    view.follow(&mut client, |view| view.workspace.providers[0].usage.is_some()).await;
    let usage = view.workspace.providers[0].usage.clone().expect("usage");
    assert_eq!(usage.error, "");
    assert!(usage.updated > 0, "it says when it was read");
    let mut windows = Vec::new();
    for window in &usage.windows {
        windows.push((window.id.as_str(), window.label.as_str(), window.used_percent, window.resets_at));
    }
    assert_eq!(windows, [("weekly", "Weekly", 40.0, 1_795_554_000_000), ("five_hour", "5-hour", 2.5, 0)]);

    expect_ack(
        apply_request(&mut client, &mut view, Request::SignOut(v1::SignOut { provider_id: "scripted".into(), ..Default::default() })).await,
    );
    assert!(view.workspace.providers[0].usage.is_none(), "signed out, it has no usage");

    // An API key has no limits to show.
    apply_request(&mut client, &mut view, start_login("api_key")).await;
    view.follow(&mut client, |view| asks(view, LoginPromptKind::Secret)).await;
    apply_request(&mut client, &mut view, answer("sk-good")).await;
    view.follow(&mut client, |view| view.workspace.login.is_none()).await;
    assert_eq!(view.workspace.providers[0].signed_in_with, "Scripted key");
    assert!(view.workspace.providers[0].usage.is_none());
}
