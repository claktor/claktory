//! The DM crate keeps its own set of hidden characters, since it's built
//! into the app too: it must miss none of the ones `text::hidden` names.

use claktory_dm_crypto::certificate::{hidden, hidden_in};
use factory_agent::text;

#[test]
fn the_dm_crates_hidden_characters_hold_every_one_text_hidden_names() {
    let selector = |c: char| ('\u{FE00}'..='\u{FE0F}').contains(&c);
    let missed: Vec<String> = (0..=u32::from(char::MAX))
        .filter_map(char::from_u32)
        .filter(|&c| text::hidden(c) && !hidden(c) && !selector(c))
        .map(|c| format!("U+{:04X}", u32::from(c)))
        .collect();
    assert!(missed.is_empty(), "missed: {missed:?}");
    // The variation selectors it judges by where they stand: hidden unless
    // right after a visible character.
    for c in ('\u{FE00}'..='\u{FE0F}').filter(|&c| text::hidden(c)) {
        assert_eq!(hidden_in(&format!(" {c}")).collect::<Vec<_>>(), [c]);
        assert_eq!(hidden_in(&format!("a{c}")).count(), 0);
    }
}
