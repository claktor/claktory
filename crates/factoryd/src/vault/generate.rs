//! Keys and passwords the vault makes itself: an agent asks for one
//! by type, the person approves, and it's made here, in factoryd's memory,
//! sealed straight into a new secret. It's never written, shown, logged or
//! sent anywhere else; the asker is told only a key's public half, or what
//! a password is made of. OpenSSH's formats are RustCrypto's `ssh-key`;
//! passwords are rand's uniform choice (Lemire's method, unbiased) over the
//! system's random numbers. Both draw on the system's randomness.

use factory_protocol::v1;
use rand::distr::{Distribution, Uniform};
use serde::{Deserialize, Serialize};
use ssh_key::getrandom::SysRng;
use ssh_key::rand_core::UnwrapErr;
use ssh_key::{Algorithm, LineEnding, PrivateKey};
use zeroize::Zeroizing;

use super::VaultError;

/// An ed25519 key for ssh: its private half an OpenSSH private key file.
pub(crate) const SSH_ED25519: &str = "ssh-ed25519";

/// A password (`Password` says what it's made of).
pub(crate) const PASSWORD: &str = "password";

/// What the vault makes.
pub(crate) const KINDS: &[&str] = &[SSH_ED25519, PASSWORD];

/// The variable a generated key's file goes in when the agent names none.
pub(crate) const VARIABLE: &str = "SSH_KEY";
/// The variable a generated password goes in when the agent names none.
pub(crate) const PASSWORD_VARIABLE: &str = "PASSWORD";

/// What a request to generate is called in sentences: "an ssh-ed25519
/// key", "a 32-character password (letters and digits, 190 bits)".
pub(crate) fn called(kind: &str, password: Option<&v1::PasswordSpec>) -> String {
    match (kind, password) {
        (PASSWORD, Some(spec)) => format!("a {}-character password ({}, {} bits)", spec.length, classes(spec), spec.bits.floor()),
        (PASSWORD, None) => "a password".to_owned(),
        _ => format!("an {kind} key"),
    }
}

/// A keypair just made. Deliberately neither Debug nor Display: the
/// private half is only ever moved into the vault.
pub(crate) struct Made {
    /// The private key's file, as OpenSSH writes it, with its last newline.
    pub private: Zeroizing<String>,
    /// The public key's line, "ssh-ed25519 AAAA… <comment>".
    pub public: String,
}

/// Makes a keypair of `kind`, commented `comment` (the secret's name). Its
/// errors never carry key material.
pub(crate) fn make(kind: &str, comment: &str) -> Result<Made, VaultError> {
    let algorithm = match kind {
        SSH_ED25519 => Algorithm::Ed25519,
        _ => return Err(VaultError::Invalid(format!("The vault can't make that kind of key; it makes {}.", KINDS.join(", ")))),
    };
    let failed = || VaultError::Invalid("The vault couldn't make the key.".to_owned());
    let mut key = PrivateKey::random(&mut UnwrapErr(SysRng), algorithm).map_err(|_| failed())?;
    key.set_comment(comment);
    let mut private = key.to_openssh(LineEnding::LF).map_err(|_| failed())?;
    // ssh refuses a key file without its last newline.
    if !private.ends_with('\n') {
        private.push('\n');
    }
    let public = key.public_key().to_openssh().map_err(|_| failed())?;
    Ok(Made { private, public })
}

// ---- Passwords ----

const LOWERCASE: &str = "abcdefghijklmnopqrstuvwxyz";
const UPPERCASE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &str = "0123456789";
/// Printable ASCII's symbols without the quotes, the backquote and the
/// backslash, which break in shells, YAML and JSON (an explicit set may
/// still have them), and without space.
const SYMBOLS: &str = "!#$%&()*+,-./:;<=>?@[]^_{|}~";
/// Characters that look alike, left out with avoid_similar.
const SIMILAR: &str = "0Oo1lI|";

/// The shortest a password is, unless the service's maximum is lower.
const SHORTEST: u32 = 16;
/// The shortest ever, whatever the service allows.
const FLOOR: u32 = 12;
/// The longest.
const LONGEST: u32 = 256;
/// The strength a password must have, in bits, unless the service's
/// maximum makes that impossible; and the least ever.
const TARGET_BITS: f64 = 80.0;
const FLOOR_BITS: f64 = 72.0;
/// The fewest characters to draw from.
const FEWEST_CHARACTERS: usize = 10;
/// Whole passwords drawn for require_each before giving up: within the
/// bounds, missing a class a thousand times over doesn't happen.
const TRIES: usize = 1000;

/// A password as an agent asks for it (secrets_request's `password`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Password {
    #[serde(default, skip_serializing_if = "is_zero")]
    pub length: u32,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub lowercase: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub uppercase: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub digits: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub symbols: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub allowed: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub avoid_similar: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub require_each: bool,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub max_length: u32,
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

fn refused(why: impl Into<String>) -> VaultError {
    VaultError::Invalid(why.into())
}

/// What a password is drawn from: its alphabet, in order, and the classes
/// that must each be in it (require_each).
struct Alphabet {
    characters: Vec<u8>,
    required: Vec<&'static str>,
}

fn class_of(character: u8) -> &'static str {
    match character {
        b'a'..=b'z' => LOWERCASE,
        b'A'..=b'Z' => UPPERCASE,
        b'0'..=b'9' => DIGITS,
        _ => SYMBOLS,
    }
}

fn alphabet(ask: &Password) -> Result<Alphabet, VaultError> {
    let mut characters: Vec<u8> = Vec::new();
    if ask.allowed.is_empty() {
        let none = !(ask.lowercase || ask.uppercase || ask.digits || ask.symbols);
        for (wanted, class) in [(ask.lowercase, LOWERCASE), (ask.uppercase, UPPERCASE), (ask.digits, DIGITS), (ask.symbols, SYMBOLS)] {
            if wanted || none {
                characters.extend(class.bytes());
            }
        }
    } else {
        if ask.lowercase || ask.uppercase || ask.digits || ask.symbols {
            return Err(refused("Give either the classes (lowercase, uppercase, digits, symbols) or allowed, not both."));
        }
        for character in ask.allowed.chars() {
            if !matches!(character, '!'..='~') {
                return Err(refused(
                    "allowed may hold only printable ASCII characters, without space: letters, digits and symbols such as ! # $ % - _ .",
                ));
            }
            let byte = character as u8;
            if !characters.contains(&byte) {
                characters.push(byte);
            }
        }
    }
    if ask.avoid_similar {
        characters.retain(|character| !SIMILAR.as_bytes().contains(character));
    }
    if characters.len() < FEWEST_CHARACTERS {
        return Err(refused(format!(
            "A password needs at least {FEWEST_CHARACTERS} different characters to draw from; this has {}.",
            characters.len()
        )));
    }
    let mut required = Vec::new();
    if ask.require_each {
        for class in [LOWERCASE, UPPERCASE, DIGITS, SYMBOLS] {
            if characters.iter().any(|character| class_of(*character) == class) {
                required.push(class);
            }
        }
    }
    Ok(Alphabet { characters, required })
}

/// A password's spec, checked against the bounds: what the card shows and
/// the vault makes. `length` overrides the length asked (the person's
/// choice; 0 keeps it). Errors say what to change; nothing weaker than the
/// bounds is ever let through.
pub(crate) fn password_spec(ask: &Password, length: u32) -> Result<v1::PasswordSpec, VaultError> {
    let alphabet = alphabet(ask)?;
    let size = alphabet.characters.len();
    let per_character = (size as f64).log2();
    if ask.max_length != 0 && ask.max_length < FLOOR {
        return Err(refused(format!(
            "The vault doesn't make passwords shorter than {FLOOR} characters, so it can't make one for a service that takes at most {}.",
            ask.max_length
        )));
    }
    let longest = if ask.max_length == 0 { LONGEST } else { ask.max_length.min(LONGEST) };
    let floor = if ask.max_length == 0 { SHORTEST } else { SHORTEST.min(ask.max_length) };
    // The target, unless the service's maximum can't reach it.
    let bits_needed = if f64::from(longest) * per_character >= TARGET_BITS { TARGET_BITS } else { FLOOR_BITS };
    let mut shortest = floor.max((bits_needed / per_character).ceil() as u32);
    while f64::from(shortest) * per_character < bits_needed {
        shortest += 1;
    }
    if shortest > longest {
        return Err(refused(format!(
            "A password of at most {longest} characters drawn from {size} would be weaker than the vault allows ({bits_needed} bits): allow more characters, or a longer password."
        )));
    }
    let length = match (length, ask.length) {
        (0, 0) => shortest,
        (0, asked) | (asked, _) => asked,
    };
    if length < shortest || length > longest {
        return Err(refused(format!(
            "{} {length}-character password is out of bounds here: from {size} characters, it can be {shortest} to {longest} characters long.",
            article(length)
        )));
    }
    let has = |class: &str| alphabet.characters.iter().any(|character| class_of(*character) == class);
    Ok(v1::PasswordSpec {
        length,
        lowercase: has(LOWERCASE),
        uppercase: has(UPPERCASE),
        digits: has(DIGITS),
        symbols: has(SYMBOLS),
        allowed: if ask.allowed.is_empty() { String::new() } else { String::from_utf8_lossy(&alphabet.characters).into_owned() },
        avoid_similar: ask.avoid_similar,
        require_each: ask.require_each,
        max_length: ask.max_length,
        alphabet_size: size as u32,
        bits: f64::from(length) * per_character,
        shortest,
        longest,
        bits_per_character: per_character,
    })
}

/// The ask a spec was made from, with its length (to check it again).
pub(crate) fn password_ask(spec: &v1::PasswordSpec) -> Password {
    let explicit = !spec.allowed.is_empty();
    Password {
        length: spec.length,
        lowercase: spec.lowercase && !explicit,
        uppercase: spec.uppercase && !explicit,
        digits: spec.digits && !explicit,
        symbols: spec.symbols && !explicit,
        allowed: spec.allowed.clone(),
        avoid_similar: spec.avoid_similar,
        require_each: spec.require_each,
        max_length: spec.max_length,
    }
}

/// "letters and digits", "lowercase letters, digits and symbols", "these
/// 20 characters: …".
pub(crate) fn classes(spec: &v1::PasswordSpec) -> String {
    let mut names = Vec::new();
    match (spec.lowercase, spec.uppercase) {
        (true, true) => names.push("letters"),
        (true, false) => names.push("lowercase letters"),
        (false, true) => names.push("uppercase letters"),
        (false, false) => {}
    }
    if spec.digits {
        names.push("digits");
    }
    if spec.symbols {
        names.push("symbols");
    }
    let names = match names.as_slice() {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    };
    if spec.allowed.is_empty() { names } else { format!("{names} from {} allowed characters", spec.alphabet_size) }
}

/// Makes a password to `spec`, checked against the bounds again. Its
/// errors never carry any of it.
pub(crate) fn make_password(spec: &v1::PasswordSpec) -> Result<Zeroizing<String>, VaultError> {
    let checked = password_spec(&password_ask(spec), spec.length)?;
    let alphabet = alphabet(&password_ask(&checked))?;
    let pick = Uniform::new(0, alphabet.characters.len()).map_err(|_| refused("The vault couldn't make the password."))?;
    let mut random = UnwrapErr(SysRng);
    for _ in 0..TRIES {
        let mut password = Zeroizing::new(String::with_capacity(checked.length as usize));
        for _ in 0..checked.length {
            password.push(char::from(alphabet.characters[pick.sample(&mut random)]));
        }
        if alphabet.required.iter().all(|class| password.bytes().any(|character| class_of(character) == *class)) {
            return Ok(password);
        }
    }
    Err(refused("The vault couldn't make a password with every kind of character asked for."))
}

/// "A" or "An" before a number below 1,000 said aloud: "An 8-character",
/// "An 11-", "An 18-", "An 80-" to "89-", "An 800-" to "899-"; "A" otherwise.
fn article(number: u32) -> &'static str {
    if matches!(number, 8 | 11 | 18 | 80..=89 | 800..=899) { "An" } else { "A" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_reads_with_its_article() {
        let read = |length: u32| format!("{} {length}-character", article(length));
        assert_eq!(read(8), "An 8-character");
        assert_eq!(read(11), "An 11-character");
        assert_eq!(read(18), "An 18-character");
        assert_eq!(read(84), "An 84-character");
        assert_eq!(read(12), "A 12-character");
        assert_eq!(read(16), "A 16-character");
        assert_eq!(read(108), "A 108-character");
        assert_eq!(read(256), "A 256-character");
    }

    #[test]
    fn a_made_key_reads_back_as_the_same_key() {
        let made = make(SSH_ED25519, "Deploy key").expect("made");
        // Split, so a secret scanner doesn't take the test for a key.
        let armor = |edge: &str| format!("-----{edge} OPENSSH {}-----\n", "PRIVATE KEY");
        assert!(made.private.starts_with(&armor("BEGIN")));
        assert!(made.private.ends_with(&armor("END")));
        assert!(made.public.starts_with("ssh-ed25519 AAAA") && made.public.ends_with(" Deploy key"), "{}", made.public);
        let read = PrivateKey::from_openssh(made.private.as_bytes()).expect("parses");
        assert_eq!(read.public_key().to_openssh().expect("public"), made.public);
        assert_eq!(read.comment().as_str_lossy(), "Deploy key");
        // Two keys are two keys.
        assert_ne!(make(SSH_ED25519, "Deploy key").expect("made").public, made.public);
        assert!(make("rsa", "x").is_err());
    }

    fn ask(length: u32) -> Password {
        Password { length, ..Password::default() }
    }

    fn refusal(ask: &Password) -> String {
        password_spec(ask, 0).expect_err("refused").to_string()
    }

    #[test]
    fn passwords_stay_within_the_bounds() {
        // All four classes by default: 90 characters, 16 by default.
        let all = password_spec(&ask(0), 0).expect("spec");
        assert_eq!((all.alphabet_size, all.length, all.shortest, all.longest), (90, 16, 16, 256));
        assert!(all.bits >= 80.0 && all.lowercase && all.uppercase && all.digits && all.symbols);
        // Letters and digits, 32, without look-alikes.
        let review = Password { length: 32, lowercase: true, uppercase: true, digits: true, avoid_similar: true, ..Password::default() };
        let review = password_spec(&review, 0).expect("spec");
        assert_eq!((review.alphabet_size, review.length, classes(&review).as_str()), (56, 32, "letters and digits"));
        // Too short, too long.
        assert!(refusal(&ask(11)).contains("out of bounds"), "{}", refusal(&ask(11)));
        assert!(refusal(&ask(15)).contains("16 to 256"));
        assert!(refusal(&ask(257)).contains("out of bounds"));
        // A service's maximum may bring it to 12, never below, and never under 72 bits.
        let short = Password { max_length: 12, ..ask(0) };
        let short = password_spec(&short, 0).expect("12 from 90 characters is 77.9 bits");
        assert_eq!((short.length, short.longest), (12, 12));
        assert!(short.bits >= 72.0 && short.bits < 80.0);
        assert!(refusal(&Password { max_length: 11, ..ask(0) }).contains("shorter than 12"));
        // 12 digits are 39.9 bits: refused, not made weak.
        let digits = Password { digits: true, max_length: 12, ..ask(0) };
        assert!(refusal(&digits).contains("weaker than the vault allows"), "{}", refusal(&digits));
        // 9 characters, non-ASCII, space, both a set and classes.
        assert!(refusal(&Password { allowed: "abcdefghi".into(), ..ask(0) }).contains("at least 10 different"));
        assert!(refusal(&Password { allowed: "abcdefghijé".into(), ..ask(0) }).contains("printable ASCII"));
        assert!(refusal(&Password { allowed: "abcdefghij k".into(), ..ask(0) }).contains("printable ASCII"));
        assert!(refusal(&Password { allowed: "abcdefghijk".into(), digits: true, ..ask(0) }).contains("not both"));
        // A set is deduplicated; 10 digits need 25 characters for 80 bits.
        let set = password_spec(&Password { allowed: "01234567899".into(), ..ask(0) }, 0).expect("spec");
        assert_eq!((set.alphabet_size, set.shortest, set.allowed.as_str()), (10, 25, "0123456789"));
        // The person's length is checked too.
        assert_eq!(password_spec(&ask(20), 40).expect("spec").length, 40);
        assert!(password_spec(&ask(20), 300).is_err());
        // Every spec Claktory shows reads back as itself.
        for spec in [all, review, short, set] {
            assert_eq!(password_spec(&password_ask(&spec), spec.length).expect("again"), spec);
        }
    }

    #[test]
    fn a_password_is_made_to_its_spec() {
        let spec = password_spec(&Password { length: 40, allowed: "abcXYZ123!".into(), require_each: true, ..Password::default() }, 0)
            .expect("spec");
        for _ in 0..200 {
            let made = make_password(&spec).expect("made");
            assert_eq!(made.len(), 40);
            assert!(made.bytes().all(|character| b"abcXYZ123!".contains(&character)));
            for class in ["abc", "XYZ", "123", "!"] {
                assert!(made.bytes().any(|character| class.as_bytes().contains(&character)), "{class} missing");
            }
        }
        // Two are two.
        let all = password_spec(&Password::default(), 0).expect("spec");
        assert_ne!(*make_password(&all).expect("made"), *make_password(&all).expect("made"));
    }

    /// Chi-square over about 100,000 characters: each character comes up
    /// as often as the others, for a small set and for the full one.
    #[test]
    fn every_character_is_as_likely_as_the_others() {
        for ask in [Password { allowed: "abcdefghij".into(), ..Password::default() }, Password::default()] {
            let spec = password_spec(&Password { length: 250, ..ask }, 0).expect("spec");
            let alphabet = alphabet(&password_ask(&spec)).expect("alphabet").characters;
            let mut counts = std::collections::HashMap::<u8, f64>::new();
            let mut total = 0.0;
            for _ in 0..400 {
                for character in make_password(&spec).expect("made").bytes() {
                    *counts.entry(character).or_default() += 1.0;
                    total += 1.0;
                }
            }
            let expected = total / alphabet.len() as f64;
            let chi: f64 =
                alphabet.iter().map(|character| (counts.get(character).copied().unwrap_or(0.0) - expected).powi(2) / expected).sum();
            // Degrees of freedom k-1: 9 and 89, at p = 10^-6 (41.3 and 167.6),
            // so a fair generator fails this about once in a million runs,
            // and a skewed one (one of the ten 10% too likely) every time.
            let limit = if alphabet.len() == 10 { 41.3 } else { 167.6 };
            assert!(chi < limit, "chi-square {chi:.1} over {} characters", alphabet.len());
        }
    }
}
