//! Data (WIT 1.1): an app's data-sources and data-query go to factoryd,
//! which decides; the app gets its answer, or why not.

use crate::support::{Factoryd, quick, temp_dir};
use factory_protocol::apps::frame::Body;
use factory_protocol::apps::{DataAnswer, DataError, DataSource};

#[tokio::test]
async fn an_apps_queries_go_to_factoryd_which_answers_them() {
    let dir = temp_dir("data");
    let mut factoryd = Factoryd::start(quick()).await;
    factoryd.load("charts", &dir, |_| {}).await;
    factoryd.start_app("charts").await;

    factoryd.sources = vec![
        DataSource { id: "factory-records".into(), label: "Claktory's records".into(), granted: true },
        DataSource { id: "agent:lea".into(), label: "Lea's database".into(), granted: false },
    ];
    assert_eq!(factoryd.text("charts", "sources", "").await, "factory-records Claktory's records true\nagent:lea Lea's database false");

    factoryd.answer = DataAnswer { json: r#"{"columns":["n"],"rows":[[1]],"more":false}"#.into(), ..Default::default() };
    assert_eq!(factoryd.text("charts", "query", "factory-records SELECT ?1 AS n").await, r#"{"columns":["n"],"rows":[[1]],"more":false}"#);
    let asked = factoryd.queries.pop().expect("factoryd was asked");
    assert_eq!(
        (asked.app_id.as_str(), asked.source.as_str(), asked.sql.as_str(), asked.params.as_str()),
        ("charts", "factory-records", "SELECT ?1 AS n", "[1]")
    );

    for (error, message, said) in [
        (DataError::NotGranted, "", "not-granted"),
        (DataError::UnknownSource, "", "unknown-source"),
        (DataError::Invalid, "One statement at a time.", "invalid: One statement at a time."),
        (DataError::Failed, "It ran longer than 10s", "failed: It ran longer than 10s"),
    ] {
        factoryd.answer = DataAnswer { json: String::new(), error: error.into(), message: message.into() };
        let Body::CallResult(result) = factoryd.call("charts", "query", "agent:lea SELECT 1").await else { panic!("a result") };
        assert_eq!((result.text.as_str(), result.is_error), (said, true));
    }
}
