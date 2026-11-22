//! Views (WIT 1.1): the host opens, acts on and closes an app's surfaces
//! when factoryd asks, lets the app push only to surfaces open now, hands
//! each push to factoryd (whose answer the app gets), and still runs apps
//! built for 1.0, which have no views.

use crate::support::{Factoryd, guest_1_0, quick, temp_dir};
use factory_protocol::apps::frame::Body;
use factory_protocol::apps::{ViewAction, ViewClosed, ViewOpen};

fn open(app: &str, view: &str, params: &str, surface: &str) -> Body {
    Body::ViewOpen(ViewOpen { app_id: app.into(), view: view.into(), params: params.into(), surface: surface.into() })
}

fn action(app: &str, surface: &str, message: &str) -> Body {
    Body::ViewAction(ViewAction { app_id: app.into(), surface: surface.into(), message: message.into() })
}

fn closed(app: &str, surface: &str) -> Body {
    Body::ViewClosed(ViewClosed { app_id: app.into(), surface: surface.into() })
}

#[tokio::test]
async fn a_view_opens_pushes_hears_actions_and_closes() {
    let dir = temp_dir("views");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("charts", &dir, |_| {}).await;
    assert_eq!(factoryd.view(open("charts", "main", "{}", "charts-1")).await, "charts isn't running.");
    factoryd.start_app("charts").await;

    assert_eq!(factoryd.view(open("charts", "main", r#"{"id":"a"}"#, "charts-1")).await, "");
    let pushed = factoryd.pushed.pop().expect("its createSurface reached factoryd");
    assert_eq!((pushed.app_id.as_str(), pushed.surface.as_str()), ("charts", "charts-1"));
    assert!(pushed.message.contains(r#""createSurface""#) && pushed.message.contains(r#"{\"id\":\"a\"}"#), "{}", pushed.message);

    assert_eq!(factoryd.view(action("charts", "charts-1", "clicked")).await, "");
    assert_eq!(factoryd.text("charts", "get", "last-action").await, "clicked");
    assert!(factoryd.pushed.pop().expect("its update").message.contains("updateDataModel"));
    assert_eq!(factoryd.view(action("charts", "charts-9", "clicked")).await, "The surface \"charts-9\" isn't open.");

    // Pushing from a tool works while it's open, and only to what's open.
    let message = r#"{"version":"1.0","deleteSurface":{"surfaceId":"charts-1"}}"#;
    assert_eq!(factoryd.text("charts", "push", &format!("charts-1 {message}")).await, "pushed");
    let Body::CallResult(other) = factoryd.call("charts", "push", "charts-2 {}").await else { panic!("a result") };
    assert_eq!((other.text.as_str(), other.is_error), ("The surface \"charts-2\" isn't open.", true));
    let too_big = format!("charts-1 {}", "x".repeat((1 << 20) + 1));
    let Body::CallResult(big) = factoryd.call("charts", "push", &too_big).await else { panic!("a result") };
    assert!(big.is_error && big.text.contains("at most 1048576"), "{}", big.text);

    // factoryd's refusal is the app's answer.
    factoryd.push_error = "VALIDATION_FAILED at /: no".into();
    let Body::CallResult(refused) = factoryd.call("charts", "push", "charts-1 {}").await else { panic!("a result") };
    assert_eq!((refused.text.as_str(), refused.is_error), ("VALIDATION_FAILED at /: no", true));
    assert_eq!(factoryd.view(open("charts", "invalid", "{}", "charts-3")).await, "VALIDATION_FAILED at /: no");
    factoryd.push_error.clear();
    let Body::CallResult(gone) = factoryd.call("charts", "push", "charts-3 {}").await else { panic!("a result") };
    assert!(gone.is_error, "a surface the app refused to open isn't open");

    assert_eq!(factoryd.view(open("charts", "nope", "{}", "charts-4")).await, "no view nope");
    assert_eq!(factoryd.view(closed("charts", "charts-1")).await, "");
    assert_eq!(factoryd.text("charts", "get", "closed").await, "charts-1");
    assert_eq!(factoryd.text("charts", "get", "push-after-close").await, "The surface \"charts-1\" isn't open.");
    assert_eq!(factoryd.view(closed("charts", "charts-1")).await, "", "closing one that isn't open is a no-op");
}

#[tokio::test]
async fn surfaces_dont_outlive_the_instance_and_are_capped() {
    let dir = temp_dir("views-cap");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("charts", &dir, |_| {}).await;
    factoryd.start_app("charts").await;
    for n in 0..16 {
        assert_eq!(factoryd.view(open("charts", "main", "{}", &format!("s{n}"))).await, "");
    }
    assert_eq!(factoryd.view(open("charts", "main", "{}", "s16")).await, "charts has 16 views open already.");
    assert_eq!(factoryd.view(open("charts", "main", "{}", "s3")).await, "", "opening an open one again is fine");

    factoryd.stop_app("charts").await;
    factoryd.start_app("charts").await;
    let Body::CallResult(gone) = factoryd.call("charts", "push", "s0 {}").await else { panic!("a result") };
    assert_eq!(gone.text, "The surface \"s0\" isn't open.", "a new instance has none open");
}

#[tokio::test]
async fn an_app_built_for_1_0_runs_and_has_no_views() {
    let dir = temp_dir("views-1-0");
    let mut factoryd = Factoryd::start(quick()).await;
    let loaded = factoryd
        .load("old", &dir, |load| {
            load.component = guest_1_0().display().to_string();
            load.world = "claktory:app@1".into();
        })
        .await;
    assert_eq!(loaded, "");
    assert_eq!(factoryd.start_app("old").await, "");
    assert_eq!(factoryd.text("old", "echo", "hi").await, "hi from builder");
    assert_eq!(factoryd.text("old", "set", "k v").await, "kept");
    assert_eq!(factoryd.view(open("old", "main", "{}", "old-1")).await, "old has no views.");
    assert_eq!(factoryd.view(closed("old", "old-1")).await, "old has no views.");
}
