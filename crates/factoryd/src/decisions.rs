//! The person's comment on a decision (#289): every card where they approve
//! or dismiss something an agent hears the outcome of takes one optional
//! comment, with either answer. It reaches that agent with the decision,
//! as the person's own words ("Matt's note: …"), never mixed into what an
//! agent or the other side of a pipe wrote.

use crate::factory::RequestError;

/// The longest comment, in characters.
pub(crate) const MAX_CHARS: usize = 2_000;

/// The comment, trimmed; refused when it's longer than `MAX_CHARS`.
pub(crate) fn comment(text: &str) -> Result<&str, RequestError> {
    let text = text.trim();
    if text.chars().count() > MAX_CHARS {
        return Err(RequestError::TooLong { what: "The comment", most: MAX_CHARS });
    }
    Ok(text)
}

/// The comment as its writer's, set apart from what comes before it
/// ("\n\nMatt's note: …"); nothing when it's empty.
pub(crate) fn note(writer: &str, comment: &str) -> String {
    let comment = comment.trim();
    if comment.is_empty() { String::new() } else { format!("\n\n{writer}'s note: {comment}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_comment_is_trimmed_and_capped() {
        assert_eq!(comment("  ok \n").expect("fine"), "ok");
        assert!(comment(&"é".repeat(MAX_CHARS)).is_ok());
        let error = comment(&"é".repeat(MAX_CHARS + 1)).expect_err("too long");
        assert_eq!(error.to_string(), "The comment is too long: 2000 characters at most.");
        assert_eq!(note("Matt", " "), "");
        assert_eq!(note("Matt", "not now"), "\n\nMatt's note: not now");
    }
}
