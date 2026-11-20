//! The person this Claktory belongs to, as every device shows them
//! (Workspace.person): their name and avatar, a circle with their initials
//! in white on one of 12 gradients. It's computed here, so every device and
//! app version draws the same. The gradient comes from a seed: one the
//! person made by shuffling (`user.avatar.seed`), or until then this
//! Claktory's key, so it never changes by itself.

use factory_protocol::v1;
use sha2::{Digest, Sha256};
use unicode_segmentation::UnicodeSegmentation;

use crate::config::Config;

/// The avatars' gradients, from (top-left) to (bottom-right). White text is
/// at least 4.5:1 on every colour. None is the agents' or Claktories'
/// palette, so a person never looks like either.
pub(crate) const PAIRS: [(&str, &str); 12] = [
    ("#4F46E5", "#7C3AED"), // indigo to violet
    ("#7C3AED", "#C026D3"), // violet to fuchsia
    ("#BE185D", "#7C3AED"), // pink to violet
    ("#E11D48", "#9F1239"), // rose
    ("#C2410C", "#BE123C"), // orange to rose
    ("#B45309", "#C2410C"), // amber to orange
    ("#4D7C0F", "#047857"), // lime to emerald
    ("#047857", "#0F766E"), // emerald to teal
    ("#0F766E", "#0369A1"), // teal to sky
    ("#0369A1", "#4338CA"), // sky to indigo
    ("#1D4ED8", "#0E7490"), // blue to cyan
    ("#475569", "#334155"), // slate
];

/// How many letters a one-word name gives ("Matt": "M").
const ONE_WORD_LETTERS: usize = 1;

/// The initials of `name`: the first letters of its first and last words
/// ("Matt Smith": "MS"), or of its one word; "" for no name. Words are split
/// on spaces, "-", "_" and ".", letters are graphemes.
pub(crate) fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|char: char| char.is_whitespace() || "-_.".contains(char)).filter(|word| !word.is_empty()).collect();
    let first = |word: &str, count: usize| word.graphemes(true).take(count).collect::<String>();
    let letters = match words.as_slice() {
        [] => String::new(),
        [word] => first(word, ONE_WORD_LETTERS),
        [first_word, .., last_word] => format!("{}{}", first(first_word, 1), first(last_word, 1)),
    };
    letters.to_uppercase()
}

/// Which of `PAIRS` a seed gives.
pub(crate) fn pair(seed: &str) -> usize {
    let digest = Sha256::digest(format!("claktory avatar\0{seed}").as_bytes());
    let number = u64::from_be_bytes(digest[..8].try_into().expect("8 bytes"));
    usize::try_from(number % PAIRS.len() as u64).expect("under 12")
}

/// A new seed, giving a different pair than `seed` does.
pub(crate) fn shuffled(seed: &str) -> String {
    let before = pair(seed);
    loop {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the system's random source");
        let next = crate::encoding::hex(&bytes);
        if pair(&next) != before {
            return next;
        }
    }
}

/// The seed the avatar's colours come from now: the person's, or `key_seed`.
pub(crate) fn seed<'a>(config: &'a Config, key_seed: &'a str) -> &'a str {
    if config.user.avatar.seed.is_empty() { key_seed } else { &config.user.avatar.seed }
}

/// The person as every device shows them. `signed_in` is the account this
/// Claktory is signed in with, if it is: its id, email and service.
pub(crate) fn person(config: &Config, key_seed: &str, signed_in: Option<(String, String, String)>) -> v1::Person {
    let name = config.user.name.trim().to_owned();
    let (from, to) = PAIRS[pair(seed(config, key_seed))];
    let (identity_id, email) = match signed_in {
        Some((account, email, service)) => {
            let host = reqwest::Url::parse(&service).ok().and_then(|url| url.host_str().map(str::to_owned)).unwrap_or(service);
            (format!("acct:{account}@{host}"), email)
        }
        None => (String::new(), String::new()),
    };
    let mut letters = initials(&name);
    if letters.is_empty() {
        letters = initials(email.split('@').next().unwrap_or_default()).graphemes(true).take(1).collect();
    }
    v1::Person { name, initials: letters, avatar_from: from.to_owned(), avatar_to: to.to_owned(), identity_id, email }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_are_the_first_and_last_words_first_letters() {
        for (name, expected) in [
            ("Matt Smith", "MS"),
            ("matt", "M"),
            ("  Mary-Jane  van der Berg ", "MB"),
            ("jean_luc.picard", "JP"),
            ("山田太郎", "山"),
            ("Émile Zola", "ÉZ"),
            ("", ""),
            (" - ", ""),
        ] {
            assert_eq!(initials(name), expected, "{name}");
        }
    }

    #[test]
    fn a_seed_keeps_its_colours_and_a_shuffle_changes_them() {
        assert_eq!(pair("abc"), pair("abc"));
        let seeds: Vec<String> = (0..200).map(|index| index.to_string()).collect();
        let used: std::collections::HashSet<usize> = seeds.iter().map(|seed| pair(seed)).collect();
        assert_eq!(used.len(), PAIRS.len(), "every pair is reachable");
        for seed in ["abc", "key", ""] {
            assert_ne!(pair(&shuffled(seed)), pair(seed));
        }
    }

    #[test]
    fn the_profile_keeps_what_a_later_backend_wrote() {
        let mut config = Config::default();
        config.user.name = "Matt".into();
        let mut text: serde_json::Value = serde_json::from_str(&config.to_json().unwrap()).unwrap();
        assert!(text["user"].get("avatar").is_none(), "not written until shuffled");
        text["user"]["avatar"] = serde_json::json!({"seed": "abc", "image": "avatar.png"});
        text["user"]["pronouns"] = serde_json::json!("he");
        let read = Config::parse(&text.to_string()).expect("a later backend's profile reads");
        assert_eq!(read.user.avatar.seed, "abc");
        let written: serde_json::Value = serde_json::from_str(&read.to_json().unwrap()).unwrap();
        assert_eq!(written["user"], text["user"], "and is written back as it was");
    }

    #[test]
    fn the_person_is_their_name_avatar_and_identity_when_signed_in() {
        let mut config = Config::default();
        config.user.name = "Matt Smith".into();
        let shown = person(&config, "key", None);
        assert_eq!((shown.name.as_str(), shown.initials.as_str()), ("Matt Smith", "MS"));
        assert_eq!((shown.avatar_from.as_str(), shown.avatar_to.as_str()), PAIRS[pair("key")]);
        assert!(shown.identity_id.is_empty() && shown.email.is_empty());
        config.user.avatar.seed = shuffled("key");
        assert_eq!(person(&config, "key", None).avatar_from, PAIRS[pair(&config.user.avatar.seed)].0);
        config.user.name.clear();
        let signed_in = Some(("abc123".to_owned(), "matt@example.com".to_owned(), "https://claktory.com".to_owned()));
        let shown = person(&config, "key", signed_in);
        assert_eq!(
            (shown.initials.as_str(), shown.identity_id.as_str(), shown.email.as_str()),
            ("M", "acct:abc123@claktory.com", "matt@example.com")
        );
    }
}
