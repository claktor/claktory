use factory_agent::providers::openai_responses::make_strict_json_schema;
use factory_agent::schema::{check, convert, parameters, strict_parameters, validate_tool_arguments};
use factory_agent::text::{utf16_head, utf16_len, utf16_tail};
use serde_json::{Value, json};

fn validate(tool: &str, arguments: Value) -> Result<Value, String> {
    validate_tool_arguments(&parameters(tool), tool, &arguments)
}

#[test]
fn the_schemas_are_the_typescript_agents() {
    assert_eq!(
        parameters("read_file"),
        json!({"type":"object","required":["path"],"properties":{"path":{"type":"string"},"offset":{"anyOf":[{"type":"integer","minimum":1},{"type":"null"}]},"limit":{"anyOf":[{"type":"integer","minimum":1,"maximum":2000},{"type":"null"}]}},"additionalProperties":false})
    );
    assert_eq!(
        parameters("write_file"),
        json!({"type":"object","required":["path","content"],"properties":{"path":{"type":"string"},"content":{"type":"string"}},"additionalProperties":false})
    );
    assert_eq!(
        parameters("edit_file"),
        json!({"type":"object","required":["path","edits"],"properties":{"path":{"type":"string"},"edits":{"type":"array","items":{"type":"object","required":["oldText","newText"],"properties":{"oldText":{"type":"string","minLength":1},"newText":{"type":"string"},"replaceAll":{"anyOf":[{"type":"boolean"},{"type":"null"}]}},"additionalProperties":false},"minItems":1}},"additionalProperties":false})
    );
    assert_eq!(
        parameters("bash"),
        json!({"type":"object","required":["command"],"properties":{"command":{"type":"string"},"timeout":{"anyOf":[{"type":"number","exclusiveMinimum":0,"maximum":7200},{"type":"null"}]},"run_in_background":{"anyOf":[{"type":"boolean"},{"type":"null"}]}},"additionalProperties":false})
    );
    let strict = strict_parameters(&parameters("read_file"));
    assert_eq!(strict["required"], json!(["path", "offset", "limit"]));
    assert_eq!(strict_parameters(&parameters("bash"))["required"], json!(["command", "timeout", "run_in_background"]));
    let strict = strict_parameters(&parameters("edit_file"));
    assert_eq!(strict["required"], json!(["path", "edits"]));
    assert_eq!(strict["properties"]["edits"]["items"]["required"], json!(["oldText", "newText", "replaceAll"]));
    assert_eq!(make_strict_json_schema(&parameters("edit_file")).unwrap(), strict);
}

#[test]
fn arguments_are_coerced_the_way_typebox_converts() {
    assert_eq!(validate("read_file", json!({"path": 5})), Ok(json!({"path": "5"})));
    assert_eq!(validate("read_file", json!({"path": "a", "offset": 0})), Ok(json!({"path": "a", "offset": null})));
    assert_eq!(validate("read_file", json!({"path": "a", "offset": "2"})), Ok(json!({"path": "a", "offset": 2})));
    assert_eq!(validate("read_file", json!({"path": "a", "offset": 1.5})), Ok(json!({"path": "a", "offset": 1})));
    assert_eq!(validate("read_file", json!({"path": "a", "offset": true})), Ok(json!({"path": "a", "offset": 1})));
    assert_eq!(validate("read_file", json!({"path": "a", "offset": ""})), Ok(json!({"path": "a", "offset": null})));
    assert_eq!(validate("read_file", json!({"path": null})), Ok(json!({"path": "null"})));
    assert_eq!(
        validate("read_file", json!({"path": "a", "offset": null, "limit": "20"})),
        Ok(json!({"path": "a", "offset": null, "limit": 20}))
    );
    assert_eq!(validate("bash", json!({"command": "ls", "timeout": 0})), Ok(json!({"command": "ls", "timeout": null})));
    assert_eq!(validate("bash", json!({"command": "ls", "timeout": "1.5"})), Ok(json!({"command": "ls", "timeout": 1.5})));
    assert_eq!(validate("bash", json!({"command": "ls", "timeout": "0"})), Ok(json!({"command": "ls", "timeout": null})));
    assert_eq!(validate("bash", json!({"command": "ls", "timeout": false})), Ok(json!({"command": "ls", "timeout": null})));
    assert_eq!(validate("write_file", json!({"path": "a", "content": 5})), Ok(json!({"path": "a", "content": "5"})));
    assert_eq!(validate("write_file", json!({"path": "a", "content": null})), Ok(json!({"path": "a", "content": "null"})));
    assert_eq!(validate("write_file", json!({"path": "a", "content": true})), Ok(json!({"path": "a", "content": "true"})));
    assert_eq!(
        validate("edit_file", json!({"path": "a", "edits": [{"oldText": 5, "newText": 6}]})),
        Ok(json!({"path": "a", "edits": [{"oldText": "5", "newText": "6"}]}))
    );
    assert_eq!(
        validate("edit_file", json!({"path": "a", "edits": [{"oldText": "x", "newText": "y", "replaceAll": "true"}]})),
        Ok(json!({"path": "a", "edits": [{"oldText": "x", "newText": "y", "replaceAll": true}]}))
    );
    assert_eq!(
        validate("edit_file", json!({"path": "a", "edits": [{"oldText": "x", "newText": "y", "replaceAll": null}]})),
        Ok(json!({"path": "a", "edits": [{"oldText": "x", "newText": "y", "replaceAll": null}]}))
    );
    assert_eq!(convert(&json!({"type": "array", "items": {"type": "string"}}), json!(7)), json!(["7"]));
    let converted = validate("read_file", json!({"path": "a", "limit": "3", "offset": "2"})).unwrap();
    let keys: Vec<String> = converted.as_object().unwrap().keys().cloned().collect();
    assert_eq!(keys, ["path", "limit", "offset"]);
}

#[test]
fn validation_failures_list_every_error_in_typebox_order() {
    let cases = [
        (
            "edit_file",
            json!({"path": "code.ts", "offset": 1, "limit": 10}),
            "Validation failed for tool \"edit_file\":\n  - edits: must have required properties edits\n  - root: must not have additional properties\n\nReceived arguments:\n{\n  \"path\": \"code.ts\",\n  \"offset\": 1,\n  \"limit\": 10\n}",
        ),
        (
            "edit_file",
            json!({"command": "touch bad"}),
            "Validation failed for tool \"edit_file\":\n  - path: must have required properties path, edits\n  - root: must not have additional properties\n\nReceived arguments:\n{\n  \"command\": \"touch bad\"\n}",
        ),
        (
            "edit_file",
            json!({"path": "code.ts", "edits": [{"oldText": "x", "newText": "y", "unexpected": true}]}),
            "Validation failed for tool \"edit_file\":\n  - edits.0: must not have additional properties\n\nReceived arguments:\n{\n  \"path\": \"code.ts\",\n  \"edits\": [\n    {\n      \"oldText\": \"x\",\n      \"newText\": \"y\",\n      \"unexpected\": true\n    }\n  ]\n}",
        ),
        (
            "edit_file",
            json!({"path": "code.ts", "edits": []}),
            "Validation failed for tool \"edit_file\":\n  - edits: must not have fewer than 1 items\n\nReceived arguments:\n{\n  \"path\": \"code.ts\",\n  \"edits\": []\n}",
        ),
        (
            "edit_file",
            json!({"path": "code.ts", "edits": [{"oldText": "", "newText": "y"}]}),
            "Validation failed for tool \"edit_file\":\n  - edits.0.oldText: must not have fewer than 1 characters\n\nReceived arguments:\n{\n  \"path\": \"code.ts\",\n  \"edits\": [\n    {\n      \"oldText\": \"\",\n      \"newText\": \"y\"\n    }\n  ]\n}",
        ),
        (
            "edit_file",
            json!({"edits": 5, "extra": 1}),
            "Validation failed for tool \"edit_file\":\n  - path: must have required properties path\n  - root: must not have additional properties\n  - edits.0: must be object\n\nReceived arguments:\n{\n  \"edits\": 5,\n  \"extra\": 1\n}",
        ),
        (
            "read_file",
            json!({"path": "a", "limit": 3000}),
            "Validation failed for tool \"read_file\":\n  - limit: must be <= 2000\n  - limit: must be null\n  - limit: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"limit\": 3000\n}",
        ),
        (
            "read_file",
            json!({"path": "a", "offset": -3}),
            "Validation failed for tool \"read_file\":\n  - offset: must be >= 1\n  - offset: must be null\n  - offset: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"offset\": -3\n}",
        ),
        (
            "read_file",
            json!({"path": "a", "offset": "abc"}),
            "Validation failed for tool \"read_file\":\n  - offset: must be integer\n  - offset: must be null\n  - offset: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"offset\": \"abc\"\n}",
        ),
        (
            "read_file",
            json!("not an object"),
            "Validation failed for tool \"read_file\":\n  - root: must be object\n\nReceived arguments:\n\"not an object\"",
        ),
        ("read_file", json!([]), "Validation failed for tool \"read_file\":\n  - root: must be object\n\nReceived arguments:\n[]"),
        (
            "bash",
            json!({"command": "ls", "timeout": 8000}),
            "Validation failed for tool \"bash\":\n  - timeout: must be <= 7200\n  - timeout: must be null\n  - timeout: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"command\": \"ls\",\n  \"timeout\": 8000\n}",
        ),
        (
            "bash",
            json!({"command": "ls", "timeout": "abc"}),
            "Validation failed for tool \"bash\":\n  - timeout: must be number\n  - timeout: must be null\n  - timeout: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"command\": \"ls\",\n  \"timeout\": \"abc\"\n}",
        ),
        (
            "bash",
            json!({"command": "ls", "timeout": -1}),
            "Validation failed for tool \"bash\":\n  - timeout: must be > 0\n  - timeout: must be null\n  - timeout: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"command\": \"ls\",\n  \"timeout\": -1\n}",
        ),
        (
            "bash",
            json!({}),
            "Validation failed for tool \"bash\":\n  - command: must have required properties command\n\nReceived arguments:\n{}",
        ),
        (
            "bash",
            json!({"command": ["ls"]}),
            "Validation failed for tool \"bash\":\n  - command: must be string\n\nReceived arguments:\n{\n  \"command\": [\n    \"ls\"\n  ]\n}",
        ),
    ];
    for (tool, arguments, message) in cases {
        let result = validate(tool, arguments.clone());
        assert_eq!(result.as_ref().err().map(String::as_str), Some(message), "{tool} {arguments}");
    }
    assert_eq!(check(&json!({"type": "boolean"}), &json!("x"), ""), vec![("root".to_owned(), "must be boolean".to_owned())]);
    assert_eq!(check(&json!({"type": "null"}), &json!(1), "a"), vec![("a".to_owned(), "must be null".to_owned())]);
}

#[test]
fn typebox_string_rules_for_null_number_and_bounds() {
    let accepted = [
        ("read_file", json!({"path": "a", "offset": "null"}), json!({"path": "a", "offset": null})),
        ("read_file", json!({"path": "a", "offset": "NULL"}), json!({"path": "a", "offset": null})),
        ("read_file", json!({"path": "a", "offset": "undefined"}), json!({"path": "a", "offset": null})),
        ("read_file", json!({"path": "a", "offset": "0x10"}), json!({"path": "a", "offset": 16})),
        ("read_file", json!({"path": "a", "offset": "0o7"}), json!({"path": "a", "offset": 7})),
        ("read_file", json!({"path": "a", "offset": "0b11"}), json!({"path": "a", "offset": 3})),
        ("read_file", json!({"path": "a", "offset": "  12  "}), json!({"path": "a", "offset": 12})),
        ("read_file", json!({"path": "a", "offset": 2.0}), json!({"path": "a", "offset": 2})),
        ("read_file", json!({"path": "a", "offset": "1e2"}), json!({"path": "a", "offset": 100})),
        ("read_file", json!({"path": "a", "offset": "1.5"}), json!({"path": "a", "offset": 1})),
        ("read_file", json!({"path": "a", "offset": "+3"}), json!({"path": "a", "offset": 3})),
        ("read_file", json!({"path": "a", "offset": "5."}), json!({"path": "a", "offset": 5})),
        ("read_file", json!({"path": "a", "limit": 2000.5}), json!({"path": "a", "limit": 2000})),
        ("read_file", json!({"path": "a", "limit": "2000.9"}), json!({"path": "a", "limit": 2000})),
        ("bash", json!({"command": "ls", "timeout": "null"}), json!({"command": "ls", "timeout": null})),
        ("bash", json!({"command": "ls", "timeout": "true"}), json!({"command": "ls", "timeout": 1})),
        ("bash", json!({"command": "ls", "timeout": "TRUE"}), json!({"command": "ls", "timeout": 1})),
        ("bash", json!({"command": "ls", "timeout": "0x1"}), json!({"command": "ls", "timeout": 1})),
        ("bash", json!({"command": "ls", "timeout": "1e-1"}), json!({"command": "ls", "timeout": 0.1})),
        ("write_file", json!({"path": 1e21, "content": 1e-7}), json!({"path": "1e+21", "content": "1e-7"})),
        ("write_file", json!({"path": 9007199254740992_u64, "content": -0.0}), json!({"path": "9007199254740992", "content": "0"})),
        ("write_file", json!({"path": "a", "content": "false"}), json!({"path": "a", "content": "false"})),
        (
            "edit_file",
            json!({"path": "a", "edits": {"oldText": "x", "newText": "y"}}),
            json!({"path": "a", "edits": [{"oldText": "x", "newText": "y"}]}),
        ),
    ];
    for (tool, arguments, converted) in accepted {
        assert_eq!(validate(tool, arguments.clone()), Ok(converted), "{tool} {arguments}");
    }
    let integer_union = "  - offset: must be integer\n  - offset: must be null\n  - offset: must match a schema in anyOf";
    let number_union = "  - timeout: must be number\n  - timeout: must be null\n  - timeout: must match a schema in anyOf";
    let rejected = [
        ("read_file", json!({"path": "a", "offset": "false"}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \"false\"\n}}")),
        ("read_file", json!({"path": "a", "offset": " "}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \" \"\n}}")),
        ("read_file", json!({"path": "a", "offset": 0.5}), "Validation failed for tool \"read_file\":\n  - offset: must be integer\n  - offset: must be >= 1\n  - offset: must be null\n  - offset: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"offset\": 0.5\n}".to_owned()),
        ("read_file", json!({"path": "a", "offset": -0.5}), "Validation failed for tool \"read_file\":\n  - offset: must be integer\n  - offset: must be >= 1\n  - offset: must be null\n  - offset: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"offset\": -0.5\n}".to_owned()),
        ("read_file", json!({"path": "a", "offset": "Infinity"}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \"Infinity\"\n}}")),
        ("read_file", json!({"path": "a", "offset": "-0"}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \"-0\"\n}}")),
        ("read_file", json!({"path": "a", "offset": ".5"}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \".5\"\n}}")),
        ("read_file", json!({"path": "a", "offset": "1_000"}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": \"1_000\"\n}}")),
        ("read_file", json!({"path": "a", "offset": []}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": []\n}}")),
        ("read_file", json!({"path": "a", "offset": {}}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": {{}}\n}}")),
        ("read_file", json!({"path": "a", "offset": [2]}), format!("Validation failed for tool \"read_file\":\n{integer_union}\n\nReceived arguments:\n{{\n  \"path\": \"a\",\n  \"offset\": [\n    2\n  ]\n}}")),
        ("bash", json!({"command": "ls", "timeout": "  "}), format!("Validation failed for tool \"bash\":\n{number_union}\n\nReceived arguments:\n{{\n  \"command\": \"ls\",\n  \"timeout\": \"  \"\n}}")),
        ("bash", json!({"command": "ls", "timeout": 1e21}), "Validation failed for tool \"bash\":\n  - timeout: must be <= 7200\n  - timeout: must be null\n  - timeout: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"command\": \"ls\",\n  \"timeout\": 1e+21\n}".to_owned()),
        ("bash", json!({"command": "ls", "timeout": "Infinity"}), format!("Validation failed for tool \"bash\":\n{number_union}\n\nReceived arguments:\n{{\n  \"command\": \"ls\",\n  \"timeout\": \"Infinity\"\n}}")),
        ("bash", json!({"command": "ls", "timeout": "NaN"}), format!("Validation failed for tool \"bash\":\n{number_union}\n\nReceived arguments:\n{{\n  \"command\": \"ls\",\n  \"timeout\": \"NaN\"\n}}")),
        ("write_file", json!({"path": 2.5, "content": []}), "Validation failed for tool \"write_file\":\n  - content: must be string\n\nReceived arguments:\n{\n  \"path\": 2.5,\n  \"content\": []\n}".to_owned()),
        ("write_file", json!({"path": "a", "content": {}}), "Validation failed for tool \"write_file\":\n  - content: must be string\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"content\": {}\n}".to_owned()),
        ("edit_file", json!({"path": "a", "edits": [["x"]]}), "Validation failed for tool \"edit_file\":\n  - edits.0: must be object\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"edits\": [\n    [\n      \"x\"\n    ]\n  ]\n}".to_owned()),
        ("edit_file", json!({"path": "a", "edits": [{"oldText": "x", "newText": "y", "replaceAll": "all"}]}), "Validation failed for tool \"edit_file\":\n  - edits.0.replaceAll: must be boolean\n  - edits.0.replaceAll: must be null\n  - edits.0.replaceAll: must match a schema in anyOf\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"edits\": [\n    {\n      \"oldText\": \"x\",\n      \"newText\": \"y\",\n      \"replaceAll\": \"all\"\n    }\n  ]\n}".to_owned()),
        ("edit_file", json!({"path": "a", "edits": [null]}), "Validation failed for tool \"edit_file\":\n  - edits.0: must be object\n\nReceived arguments:\n{\n  \"path\": \"a\",\n  \"edits\": [\n    null\n  ]\n}".to_owned()),
    ];
    for (tool, arguments, message) in rejected {
        let result = validate(tool, arguments.clone());
        assert_eq!(result.as_ref().err(), Some(&message), "{tool} {arguments}");
    }
}

#[test]
fn utf16_helpers_count_code_units() {
    assert_eq!(utf16_len("😀a"), 3);
    assert_eq!(utf16_head("😀😀", 3), "😀");
    assert_eq!(utf16_head("abc", 5), "abc");
    assert_eq!(utf16_tail("a😀😀", 3), "😀");
    assert_eq!(utf16_tail("abc", 2), "bc");
}
