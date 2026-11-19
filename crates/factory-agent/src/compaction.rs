//! Automatic context compaction: when a response shows the context has reached the agent's threshold,
//! the model summarizes the conversation and the summary replaces it.

use crate::types::{AssistantMessage, AssistantPart, Message, StopReason, Usage, UserMessage};

/// Opens the message that replaces a compacted context.
pub const SUMMARY_PREAMBLE: &str = "This session is being continued from a previous conversation that ran out of context. The summary below covers the earlier portion of the conversation.";

const CONTINUE_INSTRUCTION: &str = "Continue the task from where it left off without asking the user any further questions. Resume directly: don't acknowledge the summary or recap what was happening. Pick up the last task as if the break never happened.";

/// Sent as the last user message of the compaction request, after the whole context.
pub const COMPACTION_PROMPT: &str = r#"Your task is to create a detailed summary of the conversation so far. The summary replaces the conversation: once it is written, the earlier messages are gone, and the summary is all you will have to continue the work. Capture the user's explicit requests and your previous actions, along with every technical detail, code pattern and decision that continuing the work depends on.

CRITICAL: Respond with text only. Do not call any tools.

Before writing the summary, go through the conversation in order and note for each part: what the user asked for and meant, how you approached it, the decisions you made, and the specific details (file names, full code snippets, function signatures, file edits), the errors you ran into and how you fixed them, and any feedback the user gave, especially where they told you to do something differently.

Then write the summary inside <summary></summary> tags, with these sections:

1. Primary Request and Intent: All of the user's explicit requests and intents, in detail.
2. Key Technical Concepts: The important technologies, frameworks, concepts and constraints discussed.
3. Files and Code Sections: The files and code sections that were examined, modified or created, and why each matters. Pay special attention to the most recent messages, and include full code snippets where they matter.
4. Errors and Fixes: Every error you ran into and how you fixed it, including what the user said about it.
5. Problem Solving: Problems solved so far and any troubleshooting still under way.
6. All User Messages: Every user message that isn't a tool result, verbatim where the wording matters. They are what shows the user's feedback and how their intent changed.
7. Pending Tasks: Tasks you were explicitly asked to do that aren't done yet.
8. Current Work: Precisely what was being worked on immediately before this summary, with file names and code snippets.
9. Next Step: The next step, in line with the user's most recent explicit request and the task you were working on. If that task was finished, give a next step only if the user asked for one. Quote the most recent messages verbatim where they show exactly where you left off, so the work doesn't drift."#;

/// Tokens the context held for a response: everything sent plus everything generated.
pub fn context_tokens(usage: &Usage) -> u64 {
    usage.input + usage.cache_read + usage.cache_write + usage.output
}

/// The context size the latest completed response reported. Failed and aborted responses don't count.
pub fn latest_context_tokens(context: &[Message]) -> Option<u64> {
    for message in context.iter().rev() {
        if let Message::Assistant(assistant) = message
            && !matches!(assistant.stop_reason, StopReason::Error | StopReason::Aborted)
        {
            return Some(context_tokens(&assistant.usage));
        }
    }
    None
}

/// The summary the model wrote: the text inside `<summary>` tags if it used them, otherwise all of its text.
pub fn extract_summary(response: &AssistantMessage) -> Option<String> {
    let mut text = String::new();
    for part in &response.content {
        if let AssistantPart::Text { text: part, .. } = part {
            text.push_str(part);
        }
    }
    let inner = match (text.find("<summary>"), text.rfind("</summary>")) {
        (Some(start), Some(end)) if start + "<summary>".len() <= end => &text[start + "<summary>".len()..end],
        _ => text.as_str(),
    };
    let summary = inner.trim();
    (!summary.is_empty()).then(|| summary.to_owned())
}

/// The user message that replaces the context. `continuing` is set when no new prompt follows, so the model
/// must carry on with the task by itself.
pub fn summary_message(summary: &str, continuing: bool, timestamp: u64) -> Message {
    let mut text = format!("{SUMMARY_PREAMBLE}\n\n{summary}");
    if continuing {
        text.push_str("\n\n");
        text.push_str(CONTINUE_INSTRUCTION);
    }
    Message::User(UserMessage::text(text, timestamp))
}
