//! The checks every endpoint shares (dms-e2ee.md §2 and §8; Fugo's E0
//! review): the person's devices, a Claktory's leaf and the worker's
//! PublicGroup all run these on every member, message and commit, so no one
//! of them can be talked into a leaf the others refuse.
//!
//! - A leaf is a member only if its certificate verifies, chains to the
//!   identity key pinned for its account, that identity isn't retired, the
//!   latest device list that identity signed names it, and its account
//!   belongs in the group (L4: one of the two people in a conversation; the
//!   owner in a self group or a channel).
//! - A Claktory leaf can't speak in a conversation (H1): its application
//!   messages are dropped; it commits only its own Update or self-Remove;
//!   it never joins by external commit; only a device of the same person
//!   adds one.
//! - Pins come from first contact or from the person's own devices, never
//!   from the service (M3): nothing here pins on its own. A key that was
//!   replaced stays refused for good (L5), and device lists are compared
//!   only under one identity key (L1), never going down.

use std::collections::BTreeMap;

use openmls::prelude::{Credential, LeafNodeIndex, Proposal, Sender, StagedCommit};

use crate::Error;
use crate::certificate::{LeafCertificate, LeafKind, claktory_of};
use crate::device_list::SignedDeviceList;
use crate::identity::IdentityPublic;

/// Which kind of group, as the endpoint knows it from its own records (the
/// worker from its `dm_groups`), never from the group's own say.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GroupKind {
    /// A conversation between two people.
    Conversation { people: [String; 2] },
    /// A person's own devices.
    SelfGroup { account: String },
    /// A person's devices and one of their Claktories.
    Channel { account: String, claktory_id: String },
}

/// What an endpoint trusts about each account: the identity key it pinned
/// (or, at the worker, the one the account first uploaded), the latest
/// device list that key signed, and the keys that account had before.
pub trait Trust {
    /// The account's current identity and its latest device list.
    fn current(&self, account: &str) -> Option<(IdentityPublic, Option<&SignedDeviceList>)>;
    /// Whether `key` was the account's once and was replaced (L5).
    fn retired(&self, account: &str, key: &IdentityPublic) -> bool;
}

/// A leaf that passed: who it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub certificate: LeafCertificate,
}

impl Member {
    pub fn kind(&self) -> LeafKind {
        self.certificate.leaf.kind
    }

    pub fn account(&self) -> &str {
        &self.certificate.leaf.account_id
    }

    pub fn leaf_id(&self) -> &str {
        &self.certificate.leaf.leaf_id
    }
}

fn refused(why: impl Into<String>) -> Error {
    Error::Refused(why.into())
}

/// Whether a leaf of this kind and account belongs in the group.
fn belongs(kind: &GroupKind, certificate: &LeafCertificate) -> Result<(), Error> {
    let leaf = &certificate.leaf;
    match kind {
        GroupKind::Conversation { people } => {
            if !people.contains(&leaf.account_id) {
                return Err(refused("a leaf of someone outside the conversation"));
            }
        }
        GroupKind::SelfGroup { account } => {
            if leaf.account_id != *account || leaf.kind != LeafKind::Device {
                return Err(refused("only the person's own devices are in their self group"));
            }
        }
        GroupKind::Channel { account, claktory_id } => {
            let ours = leaf.account_id == *account;
            let fits = match leaf.kind {
                LeafKind::Device => true,
                LeafKind::Claktory => claktory_of(&leaf.leaf_id) == Some(claktory_id.as_str()),
            };
            if !ours || !fits {
                return Err(refused("only the person's devices and that Claktory are in its channel"));
            }
        }
    }
    Ok(())
}

/// The leaf `credential` with MLS signature key `signature_key`, if it's a
/// member `trust` accepts for a group of `kind`.
pub fn member(kind: &GroupKind, trust: &impl Trust, credential: &Credential, signature_key: &[u8]) -> Result<Member, Error> {
    let certificate = LeafCertificate::from_credential(credential)?;
    if certificate.leaf.signature_key.as_slice() != signature_key {
        return Err(refused("the leaf's key isn't the one its certificate names"));
    }
    let account = certificate.leaf.account_id.as_str();
    if trust.retired(account, &certificate.identity) {
        return Err(refused(format!("{account}'s certificate is from a security key that was replaced")));
    }
    let Some((identity, list)) = trust.current(account) else {
        return Err(refused(format!("no security key is known for {account} here yet")));
    };
    if identity != certificate.identity {
        return Err(Error::UnknownIdentity { account: account.to_owned() });
    }
    if !list.is_some_and(|list| list.names(&certificate)) {
        return Err(refused(format!("{account}'s device list doesn't name this leaf")));
    }
    belongs(kind, &certificate)?;
    Ok(Member { certificate })
}

/// An application message from `sender` (its leaf's credential and key):
/// who said it, or why it's dropped. A Claktory never speaks in a
/// conversation (H1): its words reach the friend only as a message the
/// person's device sent.
pub fn application(kind: &GroupKind, trust: &impl Trust, credential: &Credential, signature_key: &[u8]) -> Result<Member, Error> {
    let member = member(kind, trust, credential, signature_key)?;
    if matches!(kind, GroupKind::Conversation { .. }) && member.kind() == LeafKind::Claktory {
        return Err(refused("a Claktory doesn't send messages in a conversation"));
    }
    Ok(member)
}

/// A commit, before it's merged (endpoints) or accepted (the worker):
/// `sender` and `credential` are the committer's, as the processed message
/// says; `leaf` finds the credential and key of a current member by index.
pub fn commit(
    kind: &GroupKind,
    trust: &impl Trust,
    sender: &Sender,
    credential: &Credential,
    staged: &StagedCommit,
    leaf: &dyn Fn(LeafNodeIndex) -> Option<(Credential, Vec<u8>)>,
) -> Result<(), Error> {
    let conversation = matches!(kind, GroupKind::Conversation { .. });
    // The committer as it is after the commit: its update path's leaf, or
    // the leaf it had.
    let (committer, committer_index) = match sender {
        Sender::Member(index) => {
            let (old_credential, old_key) = leaf(*index).ok_or_else(|| refused("a commit from no member"))?;
            let before = match member(kind, trust, &old_credential, &old_key) {
                Ok(before) => before,
                // Under a key its account replaced, a leaf does one thing:
                // move itself to the new key (dms-e2ee §2.1).
                Err(error) => return rotated(kind, trust, credential, staged, (&old_credential, &old_key)).map_err(|why| why.unwrap_or(error)),
            };
            if *credential != old_credential {
                return Err(refused("the commit's credential isn't its sender's"));
            }
            if let Some(path) = staged.update_path_leaf_node() {
                let after = member(kind, trust, path.credential(), path.signature_key().as_slice())?;
                if after.account() != before.account() || after.leaf_id() != before.leaf_id() || after.kind() != before.kind() {
                    return Err(refused("a leaf can't become another leaf"));
                }
            }
            (before, Some(*index))
        }
        Sender::NewMemberCommit => {
            let path = staged.update_path_leaf_node().ok_or_else(|| refused("an external commit without its leaf"))?;
            let joiner = member(kind, trust, path.credential(), path.signature_key().as_slice())?;
            if conversation && joiner.kind() == LeafKind::Claktory {
                return Err(refused("a Claktory doesn't join a conversation by itself"));
            }
            (joiner, None)
        }
        Sender::External(_) | Sender::NewMemberProposal => return Err(refused("no one outside the group commits")),
    };

    for queued in staged.queued_proposals() {
        // Who proposed it: the committer, or a member by reference.
        let proposer = match queued.sender() {
            Sender::Member(index) if Some(*index) == committer_index => committer.clone(),
            Sender::Member(index) => {
                let (credential, key) = leaf(*index).ok_or_else(|| refused("a proposal from no member"))?;
                member(kind, trust, &credential, &key)?
            }
            Sender::NewMemberCommit if committer_index.is_none() => committer.clone(),
            _ => return Err(refused("no one outside the group proposes")),
        };
        let quiet = conversation && proposer.kind() == LeafKind::Claktory;
        // A Claktory's commit in a conversation carries only its own
        // proposals: its Update, or removing itself.
        if conversation && committer.kind() == LeafKind::Claktory && proposer.leaf_id() != committer.leaf_id() {
            return Err(refused("a Claktory commits only its own changes in a conversation"));
        }
        match queued.proposal() {
            Proposal::Add(add) => {
                let added = add.key_package().leaf_node();
                let new = member(kind, trust, added.credential(), added.signature_key().as_slice())?;
                // Only the person's devices add leaves, in every kind of group.
                if proposer.kind() != LeafKind::Device || committer_index.is_none() {
                    return Err(refused("only a member's device adds a leaf"));
                }
                if new.kind() == LeafKind::Claktory && (proposer.kind() != LeafKind::Device || proposer.account() != new.account()) {
                    return Err(refused("only a device of the same person adds a Claktory"));
                }
            }
            Proposal::Update(update) => {
                let node = update.leaf_node();
                let after = member(kind, trust, node.credential(), node.signature_key().as_slice())?;
                if after.account() != proposer.account() || after.leaf_id() != proposer.leaf_id() || after.kind() != proposer.kind() {
                    return Err(refused("a leaf can't become another leaf"));
                }
            }
            Proposal::Remove(remove) => {
                let removed = remove.removed();
                let own = matches!(queued.sender(), Sender::Member(index) if *index == removed);
                match committer_index {
                    // An external commit removes only its own earlier leaf.
                    None => {
                        let (credential, _) = leaf(removed).ok_or_else(|| refused("a removal of no member"))?;
                        let gone = LeafCertificate::from_credential(&credential)?;
                        if gone.leaf.account_id != committer.account() || gone.leaf.leaf_id != committer.leaf_id() {
                            return Err(refused("a rejoining leaf removes only its own earlier one"));
                        }
                    }
                    Some(_) if quiet && !own => return Err(refused("a Claktory removes only itself")),
                    // A device removes only its own person's leaves: Ana's
                    // devices never drop Matt's.
                    Some(_) => {
                        let (credential, _) = leaf(removed).ok_or_else(|| refused("a removal of no member"))?;
                        let gone = LeafCertificate::from_credential(&credential)?;
                        if gone.leaf.account_id != proposer.account() {
                            return Err(refused("a device removes only its own person's leaves"));
                        }
                    }
                }
            }
            Proposal::SelfRemove => {}
            Proposal::ExternalInit(_) if committer_index.is_none() => {}
            _ => return Err(refused("a kind of proposal Claktory doesn't use")),
        }
    }
    Ok(())
}

/// A commit from a member whose certificate is under an identity its
/// account replaced (dms-e2ee §2.1, R1, with Fugo's three tightenings): it
/// counts only as that leaf's own new certificate, the same leaf (account,
/// leaf id, kind and signature key) under the identity pinned for its
/// account now, on that identity's list; with nothing else in it, no
/// proposal of its own or anyone's. Such a member is otherwise inert: its
/// messages, proposals and other commits fail `member` as before.
///
/// `Err(None)`: the sender's key wasn't replaced, so the ordinary refusal
/// stands.
fn rotated(
    kind: &GroupKind,
    trust: &impl Trust,
    credential: &Credential,
    staged: &StagedCommit,
    (old_credential, old_key): (&Credential, &[u8]),
) -> Result<(), Option<Error>> {
    let old = LeafCertificate::from_credential(old_credential).map_err(|_| None)?;
    if old.leaf.signature_key.as_slice() != old_key || !trust.retired(&old.leaf.account_id, &old.identity) {
        return Err(None);
    }
    let refuse = |why: &str| Some(refused(why));
    if credential != old_credential {
        return Err(refuse("the commit's credential isn't its sender's"));
    }
    if staged.queued_proposals().next().is_some() {
        return Err(refuse("a leaf under a replaced key commits only its own move to the new one"));
    }
    let path = staged.update_path_leaf_node().ok_or_else(|| refuse("a leaf under a replaced key commits only its own move to the new one"))?;
    // The new certificate: the account's identity as pinned here now (never
    // one the commit brings, nor another replaced one), and listed.
    let new = member(kind, trust, path.credential(), path.signature_key().as_slice()).map_err(Some)?;
    if new.account() != old.leaf.account_id || new.leaf_id() != old.leaf.leaf_id || new.kind() != old.leaf.kind {
        return Err(refuse("a leaf can't become another leaf"));
    }
    if path.signature_key().as_slice() != old_key {
        return Err(refuse("a leaf moving to the new key keeps its own signature key"));
    }
    Ok(())
}

/// Every member of a group being joined (a Welcome, or an external commit
/// about to be sent), each as `member` checks it.
pub fn joined<'a>(
    kind: &GroupKind,
    trust: &impl Trust,
    members: impl IntoIterator<Item = (&'a Credential, &'a [u8])>,
) -> Result<Vec<Member>, Error> {
    members.into_iter().map(|(credential, key)| member(kind, trust, credential, key)).collect()
}

/// One account's pin, as an endpoint keeps it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pin {
    pub identity: IdentityPublic,
    /// The latest device list this identity signed, as bytes.
    #[serde(default)]
    pub device_list: Option<Vec<u8>>,
    /// The highest list version seen under this identity.
    #[serde(default)]
    pub highest_version: u64,
    /// Keys this account had before; never accepted again (L5).
    #[serde(default)]
    pub retired: Vec<IdentityPublic>,
    /// The key changed and the person hasn't seen the notice yet: sending
    /// waits (Fugo 2b).
    #[serde(default)]
    pub changed_unseen: bool,
    /// The person compared safety numbers for this key.
    #[serde(default)]
    pub verified: bool,
}

/// An endpoint's pins: the identity keys of the person's own account and
/// their friends'. Changed only by `first_contact`, `key_changed`, `import`
/// from the person's own devices and `accept_list`; never from what the
/// service says on its own (M3).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Pins {
    pins: BTreeMap<String, Pin>,
    /// Parsed device lists, by account (not kept: made again from bytes).
    #[serde(skip)]
    lists: BTreeMap<String, SignedDeviceList>,
}

impl Pins {
    pub fn get(&self, account: &str) -> Option<&Pin> {
        self.pins.get(account)
    }

    /// Pins `identity` for `account` at first contact (trust on first use):
    /// only when nothing is pinned for it yet. The same key again is fine; a
    /// different one is a key change, which `key_changed` makes visible.
    pub fn first_contact(&mut self, account: &str, identity: IdentityPublic) -> Result<(), Error> {
        match self.pins.get(account) {
            Some(pin) if pin.identity == identity => Ok(()),
            Some(pin) if pin.retired.contains(&identity) => Err(refused("that security key was replaced; it's never trusted again")),
            Some(_) => Err(refused("a different security key is pinned for this account")),
            None => {
                self.pins.insert(
                    account.to_owned(),
                    Pin { identity, device_list: None, highest_version: 0, retired: Vec::new(), changed_unseen: false, verified: false },
                );
                Ok(())
            }
        }
    }

    /// `account`'s security key changed to `identity`: the old one is
    /// retired for good, its device list forgotten, and sending to them
    /// waits until the person has seen the notice (`seen`).
    pub fn key_changed(&mut self, account: &str, identity: IdentityPublic) -> Result<(), Error> {
        let pin = self.pins.get_mut(account).ok_or_else(|| refused("nothing pinned for this account to change"))?;
        if pin.identity == identity {
            return Ok(());
        }
        if pin.retired.contains(&identity) {
            return Err(refused("that security key was replaced; it's never trusted again"));
        }
        let old = std::mem::replace(&mut pin.identity, identity);
        pin.retired.push(old);
        pin.device_list = None;
        pin.highest_version = 0;
        pin.changed_unseen = true;
        pin.verified = false;
        self.lists.remove(account);
        Ok(())
    }

    /// The person saw that `account`'s key changed.
    pub fn seen(&mut self, account: &str) {
        if let Some(pin) = self.pins.get_mut(account) {
            pin.changed_unseen = false;
        }
    }

    /// The person compared safety numbers with `account` for its key now.
    pub fn verified(&mut self, account: &str, identity: &IdentityPublic) -> Result<(), Error> {
        let pin = self.pins.get_mut(account).ok_or_else(|| refused("nothing pinned for this account"))?;
        if pin.identity != *identity {
            return Err(refused("that isn't the key pinned now"));
        }
        pin.verified = true;
        Ok(())
    }

    /// Whether the person may send to `account` now (Fugo 2b).
    pub fn can_send(&self, account: &str) -> bool {
        self.pins.get(account).is_some_and(|pin| !pin.changed_unseen)
    }

    /// Takes a device list for `account`: signed by its pinned key, for that
    /// account, and no older than any seen under that key (L1). An equal
    /// version must be the same list.
    pub fn accept_list(&mut self, account: &str, bytes: &[u8]) -> Result<(), Error> {
        let list = SignedDeviceList::from_bytes(bytes)?;
        let pin = self.pins.get_mut(account).ok_or_else(|| refused("nothing pinned for this account"))?;
        if pin.retired.contains(&list.identity) {
            return Err(refused("a device list from a security key that was replaced"));
        }
        if list.identity != pin.identity || list.list.account_id != account {
            return Err(refused("a device list that isn't from this account's security key"));
        }
        let version = list.list.version;
        if version < pin.highest_version {
            return Err(refused("an older device list than one already seen"));
        }
        if version == pin.highest_version && pin.device_list.as_deref().is_some_and(|kept| kept != bytes) {
            return Err(refused("a different device list with the same version"));
        }
        pin.highest_version = version;
        pin.device_list = Some(bytes.to_vec());
        self.lists.insert(account.to_owned(), list);
        Ok(())
    }

    /// Takes pins from another of the person's own devices (the hand-off, or
    /// the self group): accounts unknown here are pinned as they are there;
    /// a key known here is kept unless that device moved past it, and keys
    /// either side retired stay retired. Never from the service (M3).
    pub fn import(&mut self, theirs: &Pins) -> Result<(), Error> {
        for (account, their) in &theirs.pins {
            let pin = match self.pins.get(account) {
                None => their.clone(),
                Some(ours) => {
                    let mut retired = ours.retired.clone();
                    retired.extend(their.retired.iter().filter(|key| !ours.retired.contains(key)));
                    let mut pin = if ours.identity == their.identity {
                        let mut pin = if their.highest_version > ours.highest_version { their.clone() } else { ours.clone() };
                        pin.verified = ours.verified || their.verified;
                        pin.changed_unseen = ours.changed_unseen && their.changed_unseen;
                        pin
                    } else if their.retired.contains(&ours.identity) && !ours.retired.contains(&their.identity) {
                        // That device saw this key replaced: its newer one wins.
                        their.clone()
                    } else {
                        ours.clone()
                    };
                    // A key either device retired never comes back (L5).
                    if retired.contains(&pin.identity) {
                        return Err(refused(format!("your devices disagree about {account}'s security key")));
                    }
                    pin.retired = retired;
                    pin
                }
            };
            self.pins.insert(account.clone(), pin);
        }
        self.reparse()
    }

    /// Parses the kept lists again (after reading the pins back).
    pub fn reparse(&mut self) -> Result<(), Error> {
        self.lists.clear();
        for (account, pin) in &self.pins {
            if let Some(bytes) = &pin.device_list {
                let list = SignedDeviceList::from_bytes(bytes)?;
                if list.identity == pin.identity {
                    self.lists.insert(account.clone(), list);
                }
            }
        }
        Ok(())
    }
}

impl Trust for Pins {
    fn current(&self, account: &str) -> Option<(IdentityPublic, Option<&SignedDeviceList>)> {
        let pin = self.pins.get(account)?;
        Some((pin.identity, self.lists.get(account)))
    }

    fn retired(&self, account: &str, key: &IdentityPublic) -> bool {
        self.pins.get(account).is_some_and(|pin| pin.retired.contains(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certificate::tests::leaf;
    use crate::device_list::{DeviceList, Listed};
    use crate::identity::IdentityKey;

    fn list(identity: &IdentityKey, account: &str, version: u64, leaves: &[(LeafKind, &str, [u8; 32])]) -> Vec<u8> {
        let leaves = leaves.iter().map(|(kind, id, key)| Listed { kind: *kind, leaf_id: (*id).to_owned(), signature_key: *key }).collect();
        DeviceList { account_id: account.to_owned(), version, created_ms: 1, leaves }.sign(identity).unwrap().to_bytes().unwrap()
    }

    #[test]
    fn a_member_needs_the_pinned_key_its_device_list_and_a_place_in_the_group() {
        let (matt, ana) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let mut pins = Pins::default();
        pins.first_contact("matt", matt.public()).unwrap();
        pins.first_contact("ana", ana.public()).unwrap();
        let clak = crate::certificate::claktory_leaf_id("mini", 1);
        pins.accept_list("matt", &list(&matt, "matt", 1, &[(LeafKind::Device, "phone", [1; 32]), (LeafKind::Claktory, &clak, [3; 32])]))
            .unwrap();
        pins.accept_list("ana", &list(&ana, "ana", 1, &[(LeafKind::Device, "ipad", [2; 32])])).unwrap();
        let chat = GroupKind::Conversation { people: ["matt".to_owned(), "ana".to_owned()] };
        let credential = |identity: &IdentityKey, kind, account, id: &str, key| {
            crate::certificate::LeafCertificate::issue(identity, leaf(kind, account, id, key)).unwrap().to_credential().unwrap()
        };

        let phone = credential(&matt, LeafKind::Device, "matt", "phone", [1; 32]);
        assert!(member(&chat, &pins, &phone, &[1; 32]).is_ok());
        assert!(member(&chat, &pins, &phone, &[9; 32]).is_err(), "another key than its certificate's");
        // Not on the list.
        let tablet = credential(&matt, LeafKind::Device, "matt", "tablet", [4; 32]);
        assert!(member(&chat, &pins, &tablet, &[4; 32]).is_err());
        // A key the service made for Ana.
        let ghost = credential(&IdentityKey::generate().unwrap(), LeafKind::Device, "ana", "ipad", [2; 32]);
        assert!(member(&chat, &pins, &ghost, &[2; 32]).is_err());
        // Someone outside the conversation, even with a good pin (L4).
        let bo = IdentityKey::generate().unwrap();
        pins.first_contact("bo", bo.public()).unwrap();
        pins.accept_list("bo", &list(&bo, "bo", 1, &[(LeafKind::Device, "bo-phone", [5; 32])])).unwrap();
        let bo_phone = credential(&bo, LeafKind::Device, "bo", "bo-phone", [5; 32]);
        assert!(member(&chat, &pins, &bo_phone, &[5; 32]).is_err());

        // A Claktory is a member of a conversation but never speaks there (H1),
        // and isn't in the self group; in its own channel it is.
        let mini = credential(&matt, LeafKind::Claktory, "matt", &clak, [3; 32]);
        assert!(member(&chat, &pins, &mini, &[3; 32]).is_ok());
        assert!(application(&chat, &pins, &mini, &[3; 32]).is_err());
        assert!(application(&chat, &pins, &phone, &[1; 32]).is_ok());
        assert!(member(&GroupKind::SelfGroup { account: "matt".to_owned() }, &pins, &mini, &[3; 32]).is_err());
        let channel = GroupKind::Channel { account: "matt".to_owned(), claktory_id: "mini".to_owned() };
        assert!(application(&channel, &pins, &mini, &[3; 32]).is_ok());
        let other = GroupKind::Channel { account: "matt".to_owned(), claktory_id: "studio".to_owned() };
        assert!(member(&other, &pins, &mini, &[3; 32]).is_err());
    }

    #[test]
    fn pins_change_only_visibly_and_a_replaced_key_never_comes_back() {
        let (old, new) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let mut pins = Pins::default();
        pins.first_contact("ana", old.public()).unwrap();
        assert!(pins.can_send("ana"));
        // First contact doesn't replace a pin.
        assert!(pins.first_contact("ana", new.public()).is_err());
        pins.accept_list("ana", &list(&old, "ana", 5, &[(LeafKind::Device, "ipad", [2; 32])])).unwrap();
        // An older list, or another list with the same version: refused.
        assert!(pins.accept_list("ana", &list(&old, "ana", 4, &[])).is_err());
        assert!(pins.accept_list("ana", &list(&old, "ana", 5, &[])).is_err());
        assert!(pins.accept_list("ana", &list(&new, "ana", 9, &[])).is_err(), "not the pinned key");

        // The key changes: sending waits until it's seen; versions start again.
        pins.key_changed("ana", new.public()).unwrap();
        assert!(!pins.can_send("ana"));
        pins.seen("ana");
        assert!(pins.can_send("ana"));
        pins.accept_list("ana", &list(&new, "ana", 1, &[])).unwrap();
        // The old key never again: no list, no pin, no change back (L5).
        assert!(pins.accept_list("ana", &list(&old, "ana", 6, &[])).is_err());
        assert!(pins.key_changed("ana", old.public()).is_err());
        assert!(pins.retired("ana", &old.public()));

        // Pins from another of the person's devices: a newer key wins only
        // if that device retired this one, and retired keys stay retired.
        let mut stale = Pins::default();
        stale.first_contact("ana", old.public()).unwrap();
        stale.import(&pins).unwrap();
        assert_eq!(stale.get("ana").unwrap().identity, new.public());
        assert!(stale.retired("ana", &old.public()));
        let mut fresh = pins.clone();
        let mut behind = Pins::default();
        behind.first_contact("ana", old.public()).unwrap();
        fresh.import(&behind).unwrap();
        assert_eq!(fresh.get("ana").unwrap().identity, new.public(), "an old key from another device doesn't come back");
        // Kept and read back.
        let mut read: Pins = serde_json::from_str(&serde_json::to_string(&pins).unwrap()).unwrap();
        read.reparse().unwrap();
        assert_eq!(read, pins);
    }
}
