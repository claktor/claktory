use factory_agent::providers::openai_responses::convert_input;
use factory_agent::stream::LlmContext;
use factory_agent::types::{Message, SystemMessage};

#[derive(serde::Deserialize)]
struct Note {
    role: String,
    content: String,
}

#[test]
fn system_notes_keep_the_message_content_contract() {
    let message = Message::System(SystemMessage { text: "USD budget: $0/$1; $1 remaining".into(), timestamp: 1 });
    let encoded = serde_json::to_string(&message).unwrap();
    let note: Note = serde_json::from_str(&encoded).unwrap();
    assert_eq!(note.role, "system");
    assert_eq!(note.content, "USD budget: $0/$1; $1 remaining");
    assert_eq!(serde_json::from_str::<Message>(&encoded).unwrap(), message);
    assert_eq!(
        serde_json::from_str::<Message>(r#"{"role":"system","text":"USD budget: $0/$1; $1 remaining","timestamp":1}"#).unwrap(),
        message
    );
}

#[test]
fn notes_use_a_supported_instruction_role_in_openai_transports() {
    let context = LlmContext {
        system_prompt: None,
        messages: vec![Message::System(SystemMessage { text: "USD budget: $0/$1; $1 remaining".into(), timestamp: 1 })],
        tools: vec![],
    };
    for reasoning in [false, true] {
        let mut model = crate::support::luna();
        model.reasoning = reasoning;
        let serialized = serde_json::to_string(&convert_input(&model, &context, false).unwrap()).unwrap();
        let notes: Vec<Note> = serde_json::from_str(&serialized).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].role, if reasoning { "developer" } else { "system" });
        assert_eq!(notes[0].content, "USD budget: $0/$1; $1 remaining");
    }
}
