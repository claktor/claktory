//! Leaf certificates: the identity key's signature over
//! one leaf of a person's, a device or a Claktory: who it belongs to
//! (`account_id`), which leaf it is, the leaf's MLS signature key, its name
//! and when it was made. It rides in the leaf's MLS credential, a
//! BasicCredential whose identity bytes are the certificate, TLS-encoded,
//! so every member can check a leaf against the person's pinned identity
//! key (`validation`) and the service can't make one.

use openmls::prelude::{BasicCredential, Credential, CredentialType};
use tls_codec::{Deserialize as _, Serialize as _, TlsDeserialize, TlsSerialize, TlsSize, VLBytes};

use crate::Error;
use crate::identity::{IdentityKey, IdentityPublic, Purpose};

/// The certificate's format.
const VERSION: u8 = 1;
/// The longest account id, leaf id and name, in bytes.
pub const MAX_ACCOUNT_ID: usize = 64;
pub const MAX_LEAF_ID: usize = 128;
pub const MAX_NAME: usize = 64;

/// What kind of leaf: one of the person's devices, or one of their
/// Claktories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum LeafKind {
    Device,
    Claktory,
}

impl LeafKind {
    fn code(self) -> u8 {
        match self {
            LeafKind::Device => 1,
            LeafKind::Claktory => 2,
        }
    }

    fn from_code(code: u8) -> Result<LeafKind, Error> {
        match code {
            1 => Ok(LeafKind::Device),
            2 => Ok(LeafKind::Claktory),
            _ => Err(Error::Format(format!("unknown leaf kind {code}"))),
        }
    }
}

/// A leaf, as its certificate names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub kind: LeafKind,
    pub account_id: String,
    /// A device's account key, or a Claktory's `claktory_leaf_id`.
    pub leaf_id: String,
    /// The leaf's MLS signature key (Ed25519).
    pub signature_key: [u8; 32],
    /// What people see it called ("iPad", "mac-mini"): one line.
    pub name: String,
    pub created_ms: u64,
}

/// A Claktory's leaf id: its id and its registration epoch, so a Claktory
/// signed in again (a new epoch, a move) needs a new certificate.
pub fn claktory_leaf_id(claktory_id: &str, registration_epoch: u64) -> String {
    format!("{claktory_id}#{registration_epoch}")
}

/// The Claktory id in a Claktory's leaf id.
pub fn claktory_of(leaf_id: &str) -> Option<&str> {
    let (claktory, epoch) = leaf_id.rsplit_once('#')?;
    (!claktory.is_empty() && !epoch.is_empty() && epoch.bytes().all(|b| b.is_ascii_digit())).then_some(claktory)
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct Body {
    version: u8,
    kind: u8,
    account_id: VLBytes,
    leaf_id: VLBytes,
    signature_key: [u8; 32],
    name: VLBytes,
    created_ms: u64,
    identity: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, TlsSerialize, TlsDeserialize, TlsSize)]
struct Wire {
    body: Body,
    signature: [u8; 64],
}

/// A leaf certificate: the leaf, signed by `identity`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafCertificate {
    pub leaf: Leaf,
    /// The identity key that signed it (checked against the pin, never
    /// trusted for itself).
    pub identity: IdentityPublic,
    signature: [u8; 64],
}

fn ascii_id(what: &str, id: &str, most: usize) -> Result<(), Error> {
    if id.is_empty() || id.len() > most || !id.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(Error::Format(format!("{what} is 1 to {most} printable ASCII characters")));
    }
    Ok(())
}

/// A character that shows nothing, or shows as something else: the set the
/// app's review card marks (`hiddenCharacter` in app/lib/people/review.dart),
/// plus every control character, the braille blank and U+FFF0–FFF8. Friends
/// see these names. It holds every character `factory_agent::text::hidden`
/// does but the variation selectors, which `hidden_in` judges by where they
/// stand (factoryd's tests/all/hidden_characters.rs checks it); this crate
/// is built into the app too, so it keeps its own copy.
pub fn hidden(c: char) -> bool {
    let rune = u32::from(c);
    c.is_control()
        || rune == 0x00ad
        || rune == 0x034f
        || rune == 0x061c
        || rune == 0x115f
        || rune == 0x1160
        || rune == 0x17b4
        || rune == 0x17b5
        || (0x180b..=0x180f).contains(&rune)
        || (0x200b..=0x200f).contains(&rune)
        || (0x2028..=0x202e).contains(&rune)
        || (0x2060..=0x206f).contains(&rune)
        || rune == 0x2800
        || rune == 0x3164
        || rune == 0xfeff
        || rune == 0xffa0
        || (0xfff0..=0xfffb).contains(&rune)
        || (0x1d173..=0x1d17a).contains(&rune)
        || (0xe0000..=0xe0fff).contains(&rune)
}

/// One line of at most MAX_NAME bytes with nothing hidden in it. A
/// variation selector (U+FE00–FE0F) is a visible character's only when it's
/// the one right after it (an emoji's); anywhere else it's refused.
fn check_name(name: &str) -> Result<(), Error> {
    if one_line(name, MAX_NAME) {
        Ok(())
    } else {
        Err(Error::Format(format!("a leaf's name is one line of 1 to {MAX_NAME} bytes, with nothing hidden in it")))
    }
}

/// Whether `text` is one line of 1 to `most` bytes with nothing hidden in it:
/// a variation selector only right after a visible character. Also what a
/// device checks of names it shows that came from elsewhere (E8's history).
pub fn one_line(text: &str, most: usize) -> bool {
    !text.trim().is_empty() && text.len() <= most && hidden_in(text).next().is_none()
}

/// The characters of `text` that show nothing or show as something else:
/// those `hidden` names, and a variation selector (U+FE00–FE0F) anywhere but
/// right after a visible character, where it's an emoji's own, and a zero
/// width joiner anywhere but between two pictographs (`joins_pictographs`).
pub fn hidden_in(text: &str) -> impl Iterator<Item = char> + '_ {
    let marks = hidden_marks(text);
    text.chars().zip(marks).filter_map(|(c, hidden)| hidden.then_some(c))
}

/// For each character of `text`, in order, whether `hidden_in` names it.
pub fn hidden_marks(text: &str) -> Vec<bool> {
    let chars: Vec<char> = text.chars().collect();
    let mut after_visible = false;
    let mut marks = Vec::with_capacity(chars.len());
    for (at, &c) in chars.iter().enumerate() {
        let hidden = if ('\u{fe00}'..='\u{fe0f}').contains(&c) {
            let stray = !after_visible;
            after_visible = false;
            stray
        } else if c == '\u{200d}' {
            after_visible = false;
            !joins_pictographs(&chars, at)
        } else if hidden(c) {
            after_visible = false;
            true
        } else {
            after_visible = !c.is_whitespace();
            false
        };
        marks.push(hidden);
    }
    marks
}

/// Whether the zero width joiner at `at` joins two emoji: the
/// character after it is a pictograph, and so is the one before it, with at
/// most one U+FE0F and one skin tone (U+1F3FB–1F3FF) between (❤️‍🔥, 🧑🏽‍💻).
/// That only joins their glyphs; anywhere else it could hide something.
fn joins_pictographs(chars: &[char], at: usize) -> bool {
    if !chars.get(at + 1).is_some_and(|&next| pictographic(next)) {
        return false;
    }
    let (mut selector, mut tone) = (false, false);
    for &before in chars[..at].iter().rev() {
        match before {
            '\u{fe0f}' if !selector => selector = true,
            '\u{1f3fb}'..='\u{1f3ff}' if !tone => tone = true,
            other => return pictographic(other),
        }
    }
    false
}

/// Unicode's Extended_Pictographic (emoji-data.txt, Unicode 15.1), which
/// emoji sequences are made of: not letters, digits, flags' regional
/// indicators or skin tones.
pub fn pictographic(c: char) -> bool {
    const RANGES: &[(u32, u32)] = &[
        (0x00a9, 0x00a9), (0x00ae, 0x00ae), (0x203c, 0x203c), (0x2049, 0x2049), (0x2122, 0x2122), (0x2139, 0x2139),
        (0x2194, 0x2199), (0x21a9, 0x21aa), (0x231a, 0x231b), (0x2328, 0x2328), (0x2388, 0x2388), (0x23cf, 0x23cf),
        (0x23e9, 0x23f3), (0x23f8, 0x23fa), (0x24c2, 0x24c2), (0x25aa, 0x25ab), (0x25b6, 0x25b6), (0x25c0, 0x25c0),
        (0x25fb, 0x25fe), (0x2600, 0x2605), (0x2607, 0x2612), (0x2614, 0x2685), (0x2690, 0x2705), (0x2708, 0x2712),
        (0x2714, 0x2714), (0x2716, 0x2716), (0x271d, 0x271d), (0x2721, 0x2721), (0x2728, 0x2728), (0x2733, 0x2734),
        (0x2744, 0x2744), (0x2747, 0x2747), (0x274c, 0x274c), (0x274e, 0x274e), (0x2753, 0x2755), (0x2757, 0x2757),
        (0x2763, 0x2767), (0x2795, 0x2797), (0x27a1, 0x27a1), (0x27b0, 0x27b0), (0x27bf, 0x27bf), (0x2934, 0x2935),
        (0x2b05, 0x2b07), (0x2b1b, 0x2b1c), (0x2b50, 0x2b50), (0x2b55, 0x2b55), (0x3030, 0x3030), (0x303d, 0x303d),
        (0x3297, 0x3297), (0x3299, 0x3299), (0x1f000, 0x1f0ff), (0x1f10d, 0x1f10f), (0x1f12f, 0x1f12f), (0x1f16c, 0x1f171),
        (0x1f17e, 0x1f17f), (0x1f18e, 0x1f18e), (0x1f191, 0x1f19a), (0x1f1ad, 0x1f1e5), (0x1f201, 0x1f20f), (0x1f21a, 0x1f21a),
        (0x1f22f, 0x1f22f), (0x1f232, 0x1f23a), (0x1f23c, 0x1f23f), (0x1f249, 0x1f3fa), (0x1f400, 0x1f53d), (0x1f546, 0x1f64f),
        (0x1f680, 0x1f6ff), (0x1f774, 0x1f77f), (0x1f7d5, 0x1f7ff), (0x1f80c, 0x1f80f), (0x1f848, 0x1f84f), (0x1f85a, 0x1f85f),
        (0x1f888, 0x1f88f), (0x1f8ae, 0x1f8ff), (0x1f90c, 0x1f93a), (0x1f93c, 0x1f945), (0x1f947, 0x1faff), (0x1fc00, 0x1fffd),
    ];
    let rune = u32::from(c);
    RANGES
        .binary_search_by(|&(low, high)| {
            if high < rune {
                std::cmp::Ordering::Less
            } else if low > rune {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

impl Leaf {
    fn check(&self) -> Result<(), Error> {
        ascii_id("an account id", &self.account_id, MAX_ACCOUNT_ID)?;
        ascii_id("a leaf id", &self.leaf_id, MAX_LEAF_ID)?;
        if self.kind == LeafKind::Claktory && claktory_of(&self.leaf_id).is_none() {
            return Err(Error::Format("a Claktory's leaf id is its id and registration epoch".to_owned()));
        }
        check_name(&self.name)
    }

    fn body(&self, identity: IdentityPublic) -> Body {
        Body {
            version: VERSION,
            kind: self.kind.code(),
            account_id: self.account_id.as_bytes().to_vec().into(),
            leaf_id: self.leaf_id.as_bytes().to_vec().into(),
            signature_key: self.signature_key,
            name: self.name.as_bytes().to_vec().into(),
            created_ms: self.created_ms,
            identity: identity.0,
        }
    }
}

fn text(bytes: &VLBytes) -> Result<String, Error> {
    String::from_utf8(bytes.as_slice().to_vec()).map_err(|_| Error::Format("not UTF-8".to_owned()))
}

impl LeafCertificate {
    /// `identity` certifies `leaf`.
    pub fn issue(identity: &IdentityKey, leaf: Leaf) -> Result<LeafCertificate, Error> {
        leaf.check()?;
        let public = identity.public();
        let body = leaf.body(public).tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        let signature = identity.sign(Purpose::LeafCertificate, &body);
        Ok(LeafCertificate { leaf, identity: public, signature })
    }

    /// Checks the signature against the identity named inside. Whether that
    /// identity is the one pinned for the account is `validation`'s.
    pub fn verify(&self) -> Result<(), Error> {
        let body = self.leaf.body(self.identity).tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))?;
        self.identity.verify(Purpose::LeafCertificate, &body, &self.signature)
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, Error> {
        let wire = Wire { body: self.leaf.body(self.identity), signature: self.signature };
        wire.tls_serialize_detached().map_err(|error| Error::Format(error.to_string()))
    }

    /// Reads a certificate, all of `bytes` and nothing after, checking its
    /// form and its signature.
    pub fn from_bytes(bytes: &[u8]) -> Result<LeafCertificate, Error> {
        let wire = Wire::tls_deserialize_exact(bytes).map_err(|_| Error::Format("not a leaf certificate".to_owned()))?;
        let body = wire.body;
        if body.version != VERSION {
            return Err(Error::Format(format!("leaf certificate version {}", body.version)));
        }
        let leaf = Leaf {
            kind: LeafKind::from_code(body.kind)?,
            account_id: text(&body.account_id)?,
            leaf_id: text(&body.leaf_id)?,
            signature_key: body.signature_key,
            name: text(&body.name)?,
            created_ms: body.created_ms,
        };
        leaf.check()?;
        let certificate = LeafCertificate { leaf, identity: IdentityPublic::from_slice(&body.identity)?, signature: wire.signature };
        certificate.verify()?;
        Ok(certificate)
    }

    /// The MLS credential that carries it.
    pub fn to_credential(&self) -> Result<Credential, Error> {
        Ok(BasicCredential::new(self.to_bytes()?).into())
    }

    /// The certificate in an MLS credential, checked; refused for any other
    /// kind of credential.
    pub fn from_credential(credential: &Credential) -> Result<LeafCertificate, Error> {
        if credential.credential_type() != CredentialType::Basic {
            return Err(Error::Format("not a basic credential".to_owned()));
        }
        let basic = BasicCredential::try_from(credential.clone()).map_err(|_| Error::Format("not a basic credential".to_owned()))?;
        LeafCertificate::from_bytes(basic.identity())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn leaf(kind: LeafKind, account: &str, id: &str, key: [u8; 32]) -> Leaf {
        Leaf {
            kind,
            account_id: account.to_owned(),
            leaf_id: id.to_owned(),
            signature_key: key,
            name: "iPad".to_owned(),
            created_ms: 1_763_700_000_000,
        }
    }

    #[test]
    fn a_certificate_travels_in_a_credential_and_any_change_breaks_it() {
        let identity = IdentityKey::generate().unwrap();
        let certificate = LeafCertificate::issue(&identity, leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32])).unwrap();
        let credential = certificate.to_credential().unwrap();
        let read = LeafCertificate::from_credential(&credential).unwrap();
        assert_eq!(read, certificate);
        assert_eq!(read.identity, identity.public());

        // A byte changed anywhere, or one added, is refused.
        let bytes = certificate.to_bytes().unwrap();
        for at in [1, 5, bytes.len() / 2, bytes.len() - 1] {
            let mut changed = bytes.clone();
            changed[at] ^= 1;
            assert!(LeafCertificate::from_bytes(&changed).is_err(), "byte {at}");
        }
        let mut longer = bytes.clone();
        longer.push(0);
        assert!(LeafCertificate::from_bytes(&longer).is_err());

        // Another identity claiming it is refused: the signature is over it.
        let mut stolen = certificate.clone();
        stolen.identity = IdentityKey::generate().unwrap().public();
        assert_eq!(stolen.verify().map_err(|e| e.to_string()), Err("refused: a signature doesn't verify".to_owned()));
    }

    #[test]
    fn what_a_certificate_may_say_is_checked() {
        let identity = IdentityKey::generate().unwrap();
        let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        bad.name = "two\nlines".to_owned();
        assert!(LeafCertificate::issue(&identity, bad).is_err());
        let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        bad.name = "a\u{202e}gnp.exe".to_owned();
        assert!(LeafCertificate::issue(&identity, bad).is_err());
        for hidden in [
            "a\u{2028}b",
            "a\u{2063}b",
            "soft\u{ad}hyphen",
            "\u{3164}",
            "a\u{fe0f}\u{fe0f}",
            " \u{fe0f}",
            "a\u{e0041}",
            "a\u{e0065}b",
            "a\u{061c}b",
            "Ana\u{3164}",
            "a\u{fff0}b",
        ] {
            let mut bad = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
            bad.name = hidden.to_owned();
            assert!(LeafCertificate::issue(&identity, bad).is_err(), "{hidden:?}");
        }
        let mut heart = leaf(LeafKind::Device, "acct-1", "dev-1", [7; 32]);
        heart.name = "Ana\u{2764}\u{fe0f} iPad".to_owned();
        assert!(LeafCertificate::issue(&identity, heart).is_ok(), "an emoji's own selector");
        let found: Vec<char> = hidden_in("\u{fe0f}A\u{2764}\u{fe0f}\u{fe0f}\u{e0065}\u{61c} \u{fe0e}\u{3164}").collect();
        assert_eq!(found, ['\u{fe0f}', '\u{fe0f}', '\u{e0065}', '\u{61c}', '\u{fe0e}', '\u{3164}']);
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Device, "", "dev-1", [7; 32])).is_err());
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Claktory, "acct-1", "no-epoch", [7; 32])).is_err());
        let id = claktory_leaf_id("mac-mini-1a2b", 3);
        assert_eq!(claktory_of(&id), Some("mac-mini-1a2b"));
        assert!(LeafCertificate::issue(&identity, leaf(LeafKind::Claktory, "acct-1", &id, [7; 32])).is_ok());
    }

    /// A zero width joiner is an emoji sequence's only between two
    /// pictographs, the earlier with at most its U+FE0F and a skin tone;
    /// tag characters stay hidden.
    #[test]
    fn a_joiner_only_between_pictographs() {
        for joined in ["👩\u{200d}💻", "🏳\u{fe0f}\u{200d}🌈", "❤\u{fe0f}\u{200d}🔥", "🧑🏽\u{200d}💻", "👨\u{200d}👩\u{200d}👧"] {
            assert_eq!(hidden_in(joined).count(), 0, "{joined:?}");
            assert!(one_line(joined, MAX_NAME));
        }
        for stray in [
            "A\u{200d}na",
            "\u{200d}👍",
            "👍\u{200d}",
            "👍\u{200d}\u{200d}💻",
            "a\u{200d}💻",
            "💻\u{200d}a",
            "👍\u{fe0f}\u{fe0f}\u{200d}💻",
            "👍🏽🏽\u{200d}💻",
            "🏴\u{e0067}\u{e007f}",
        ] {
            assert!(hidden_in(stray).count() > 0, "{stray:?}");
        }
        assert_eq!(hidden_marks("👍\u{200d}\u{200d}💻"), [false, true, true, false]);
        assert!(pictographic('©') && pictographic('\u{1fffd}') && !pictographic('A') && !pictographic('\u{1f3fb}'));
        assert!(!pictographic('\u{1f1e6}'), "a regional indicator");
    }
}
