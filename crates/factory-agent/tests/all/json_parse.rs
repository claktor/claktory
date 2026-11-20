use factory_agent::json_parse::{parse_streaming_json, repair_json};
use factory_agent::types::same_json;
use serde_json::Value;

/// The fixture is JSON written by JavaScript, so `1500` there is the `1.5e3` the parser reads here.
#[test]
fn matches_pi_on_every_recorded_case() {
    let cases: Vec<Value> = serde_json::from_str(include_str!("../fixtures/json_parse.json")).expect("fixture");
    assert_eq!(cases.len(), 44);
    for case in cases {
        let input = case["input"].as_str().expect("input");
        assert_eq!(repair_json(input), case["repaired"].as_str().expect("repaired"), "repair {input:?}");
        let parsed = parse_streaming_json(input);
        assert!(same_json(&parsed, &case["parsed"]), "parse {input:?}: {parsed} != {}", case["parsed"]);
    }
}
