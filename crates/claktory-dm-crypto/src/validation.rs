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
    /// Whether the person said `key`'s safety numbers didn't match
    /// (`Pins::undo_change`): a leaf of any kind under it holds sends until
    /// it's out (Fugo 41542).
    fn refused(&self, _account: &str, _key: &IdentityPublic) -> bool {
        false
    }
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
    member_tolerating(kind, trust, None, credential, signature_key)
}

/// `member`, with `tolerated` (a window's leaf, row 561) counting as listed
/// though the account's latest list doesn't name it: past that one check
/// only, every other one as before.
pub(crate) fn member_tolerating(
    kind: &GroupKind,
    trust: &impl Trust,
    tolerated: Option<&LeafCertificate>,
    credential: &Credential,
    signature_key: &[u8],
) -> Result<Member, Error> {
    let certificate = LeafCertificate::from_credential(credential)?;
    if certificate.leaf.signature_key.as_slice() != signature_key {
        return Err(refused("the leaf's key isn't the one its certificate names"));
    }
    let account = certificate.leaf.account_id.as_str();
    if trust.retired(account, &certificate.identity) {
        return Err(Error::Replaced { account: account.to_owned() });
    }
    let Some((identity, list)) = trust.current(account) else {
        return Err(refused(format!("no security key is known for {account} here yet")));
    };
    if identity != certificate.identity {
        return Err(Error::UnknownIdentity { account: account.to_owned() });
    }
    if !list.is_some_and(|list| list.names(&certificate)) && tolerated != Some(&certificate) {
        return Err(Error::Unlisted { account: account.to_owned() });
    }
    belongs(kind, &certificate)?;
    Ok(Member { certificate })
}

/// An application message from `sender` (its leaf's credential and key):
/// who said it, or why it's dropped. A Claktory never speaks in a
/// conversation (H1): its words reach the friend only as a message the
/// person's device sent.
pub fn application(kind: &GroupKind, trust: &impl Trust, credential: &Credential, signature_key: &[u8]) -> Result<Member, Error> {
    application_tolerating(kind, trust, None, credential, signature_key)
}

/// `application`, with a window's leaf tolerated as `member_tolerating`.
pub(crate) fn application_tolerating(
    kind: &GroupKind,
    trust: &impl Trust,
    tolerated: Option<&LeafCertificate>,
    credential: &Credential,
    signature_key: &[u8],
) -> Result<Member, Error> {
    let member = member_tolerating(kind, trust, tolerated, credential, signature_key)?;
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
    commit_tolerating(kind, trust, None, sender, credential, staged, leaf)
}

/// `commit`, with a window's leaf tolerated as `member_tolerating` wherever
/// a leaf is checked (row 561).
pub(crate) fn commit_tolerating(
    kind: &GroupKind,
    trust: &impl Trust,
    tolerated: Option<&LeafCertificate>,
    sender: &Sender,
    credential: &Credential,
    staged: &StagedCommit,
    leaf: &dyn Fn(LeafNodeIndex) -> Option<(Credential, Vec<u8>)>,
) -> Result<(), Error> {
    let member =
        |kind: &GroupKind, trust: &_, credential: &Credential, key: &[u8]| member_tolerating(kind, trust, tolerated, credential, key);
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
                Err(error) => {
                    return rotated(kind, trust, credential, staged, (&old_credential, &old_key)).map_err(|why| why.unwrap_or(error));
                }
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
                    // In every kind of group (Melone's F5): a Claktory
                    // never takes the person's devices out of its channel.
                    Some(_) if proposer.kind() == LeafKind::Claktory && !own => {
                        return Err(refused("a Claktory removes only itself"));
                    }
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
    let path =
        staged.update_path_leaf_node().ok_or_else(|| refuse("a leaf under a replaced key commits only its own move to the new one"))?;
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
/// about to be sent), each as `present` checks it.
pub fn joined<'a>(
    kind: &GroupKind,
    trust: &impl Trust,
    members: impl IntoIterator<Item = (&'a Credential, &'a [u8])>,
) -> Result<Vec<Member>, Error> {
    members.into_iter().map(|(credential, key)| present(kind, trust, credential, key)).collect()
}

/// A leaf a group being joined may hold (row 611, Fugo's a′): one `member`
/// accepts, or one of the person's devices or Claktories they kept when
/// they made a new security key, still under the old one until it next
/// comes online and moves (a Claktory: until a device enrolls it again).
/// That one only if all of these hold:
/// - its certificate's identity is one the pins record as its account's
///   replaced key (never a key they don't know);
/// - the account's current list, signed by its current identity, names it
///   (its kind, leaf id and signature key): a kept leaf is carried onto
///   the new list, a leaf a thief made under the stolen key isn't;
/// - it belongs in the group (a channel's Claktory is that channel's).
///
/// It stays inert all the same: `member` still refuses it, so what it says
/// and proposes is dropped and it commits only its own move (`rotated`).
/// Anything else under a replaced key still fails the join, and the joiner
/// waits until it's swept.
pub fn present(kind: &GroupKind, trust: &impl Trust, credential: &Credential, signature_key: &[u8]) -> Result<Member, Error> {
    let error = match member(kind, trust, credential, signature_key) {
        Ok(member) => return Ok(member),
        Err(error) => error,
    };
    let Ok(certificate) = LeafCertificate::from_credential(credential) else { return Err(error) };
    let leaf = &certificate.leaf;
    let account = leaf.account_id.as_str();
    let kept = matches!(leaf.kind, LeafKind::Device | LeafKind::Claktory)
        && leaf.signature_key.as_slice() == signature_key
        && trust.retired(account, &certificate.identity)
        && listed_now(trust, &certificate);
    if !kept {
        return Err(error);
    }
    belongs(kind, &certificate)?;
    Ok(Member { certificate })
}

/// Whether the account's current list, signed by its current identity,
/// names this leaf: its kind, leaf id and signature key.
fn listed_now(trust: &impl Trust, certificate: &LeafCertificate) -> bool {
    let leaf = &certificate.leaf;
    trust.current(&leaf.account_id).is_some_and(|(identity, list)| {
        list.is_some_and(|list| {
            list.identity == identity
                && list.list.account_id == leaf.account_id
                && list
                    .list
                    .leaves
                    .iter()
                    .any(|listed| listed.kind == leaf.kind && listed.leaf_id == leaf.leaf_id && listed.signature_key == leaf.signature_key)
        })
    })
}

/// A dead leaf, as the endpoint `me` sees it (row 630, Fugo's rulings): a
/// leaf under a security key its account replaced that the account's
/// current list doesn't carry over, left behind when the person started
/// over on a device that had lost its keys (a lost device's, or one a thief
/// made under the stolen key). That is, its certificate is for its own
/// signature key, under an identity that isn't the one pinned for its
/// account now, and the current list doesn't name it; and either
/// - it's `me`'s own account's, under an identity other than `me`'s: a
///   device that started over never pinned the key it lost, so it can't
///   know it as retired; or
/// - the pins hold its identity as that account's replaced one.
///
/// A leaf the person kept (named on the new list) is never dead, nor is a
/// leaf under a key never pinned for another account. The joiner takes
/// its own account's out at once (`Group::remove_dead`); until then no
/// one sends to a group with a dead device in it (`Group::send`).
pub fn dead(trust: &impl Trust, me: &LeafCertificate, credential: &Credential, signature_key: &[u8]) -> Option<LeafCertificate> {
    let certificate = LeafCertificate::from_credential(credential).ok()?;
    let leaf = &certificate.leaf;
    if leaf.signature_key.as_slice() != signature_key {
        return None;
    }
    let account = leaf.account_id.as_str();
    if trust.current(account).is_some_and(|(identity, _)| identity == certificate.identity) {
        return None;
    }
    let own = *account == me.leaf.account_id && certificate.identity != me.identity;
    if !own && !trust.retired(account, &certificate.identity) {
        return None;
    }
    (!listed_now(trust, &certificate)).then_some(certificate)
}

/// A former install of a device (row 202): a leaf under its account's
/// current identity whose leaf id that identity's current list names under
/// another signature key, and which the list doesn't name itself. A
/// reinstalled device keeps its leaf id (its account key) but comes back
/// with a new leaf key; only the identity holder could list that id under
/// it, so the former install is no longer authorised, and its keys may be
/// in someone else's hands (a phone reinstalled to clean it). Treated as
/// `dead`: the account's own devices take it out at once by its tree index
/// (`Group::remove_dead`), never by its id, which the new install shares;
/// nobody sends to the group until it's out (`Group::send`).
pub fn stale(trust: &impl Trust, credential: &Credential, signature_key: &[u8]) -> Option<LeafCertificate> {
    let certificate = LeafCertificate::from_credential(credential).ok()?;
    let leaf = &certificate.leaf;
    if leaf.signature_key.as_slice() != signature_key {
        return None;
    }
    let (identity, list) = trust.current(&leaf.account_id)?;
    let list = list?;
    if certificate.identity != identity || list.identity != identity || list.list.account_id != leaf.account_id {
        return None;
    }
    let same_id = |listed: &&crate::device_list::Listed| listed.kind == leaf.kind && listed.leaf_id == leaf.leaf_id;
    let mut named = list.list.leaves.iter().filter(same_id);
    let relisted = named.clone().any(|listed| listed.signature_key != leaf.signature_key);
    let itself = named.any(|listed| listed.signature_key == leaf.signature_key);
    (relisted && !itself).then_some(certificate)
}

/// Why a group holds sends (`Error::Waiting`, `Group::held`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Held {
    /// A dead leaf: under a security key its account replaced (`dead`).
    Replaced,
    /// A stale leaf: a reinstalled device's former install (`stale`).
    Reinstalled,
}

impl std::fmt::Display for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Held::Replaced => "new security key",
            Held::Reinstalled => "reinstalled device",
        })
    }
}

/// Trust as a device that started over sees it while it rejoins (row 630):
/// its own account's keys other than its own count as replaced, since it
/// never pinned the one it lost. So a device the person kept, which its own
/// new list names, is `present` past the old key, and the rest are `dead`.
/// Only for checking who's in a group being rejoined.
pub struct Rejoining<'a, T: Trust> {
    pub trust: &'a T,
    pub me: &'a LeafCertificate,
}

impl<T: Trust> Trust for Rejoining<'_, T> {
    fn current(&self, account: &str) -> Option<(IdentityPublic, Option<&SignedDeviceList>)> {
        self.trust.current(account)
    }

    fn retired(&self, account: &str, key: &IdentityPublic) -> bool {
        self.trust.retired(account, key) || (account == self.me.leaf.account_id && *key != self.me.identity)
    }

    fn refused(&self, account: &str, key: &IdentityPublic) -> bool {
        self.trust.refused(account, key)
    }
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
    /// Keys this account had before that a commit proved replaced
    /// (`Pins::replaced`), or that a device retired as such: never accepted
    /// again (L5).
    #[serde(default)]
    pub retired: Vec<IdentityPublic>,
    /// Keys this account had before that the person's devices moved off on
    /// the service's word alone (`key_changed`): replaced as far as leaves
    /// go, but one can come back, as an ordinary key change, so a single
    /// lying answer can't end a conversation for good (Fugo's E11-3). Each
    /// keeps its list and highest version, so L1 still holds if it does.
    #[serde(default)]
    pub previous: Vec<Previous>,
    /// The key changed and the person hasn't seen the notice yet: sending
    /// waits (Fugo 2b).
    #[serde(default)]
    pub changed_unseen: bool,
    /// The person compared safety numbers for this key.
    #[serde(default)]
    pub verified: bool,
    /// The key this one replaced at its last change (`key_changed`), while
    /// that one is still previous: what "the numbers don't match" goes back
    /// to (`undo_change`). None after an undo, or once that key is retired.
    #[serde(default)]
    pub changed_from: Option<IdentityPublic>,
    /// This key is one the person said didn't match before (`undo_change`)
    /// and it came back: the notice says so. Cleared once they verify it.
    #[serde(default)]
    pub mismatched: bool,
    /// Every key the person said didn't match (`undo_change`), previous or
    /// retired since: a leaf of any kind under one holds sends until it's
    /// out (`Trust::refused`, Fugo 41542 and 41686). Unioned on import and
    /// never dropped, unless the person verifies that very key as current.
    #[serde(default)]
    pub refused: Vec<IdentityPublic>,
}

/// A key an account had before, as it was when the person's devices moved
/// off it (`Pin::previous`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Previous {
    pub identity: IdentityPublic,
    #[serde(default)]
    pub device_list: Option<Vec<u8>>,
    #[serde(default)]
    pub highest_version: u64,
    /// Whether the person had compared safety numbers for it: given back
    /// only by `undo_change`.
    #[serde(default)]
    pub verified: bool,
    /// The person said its safety numbers didn't match (`undo_change`).
    #[serde(default)]
    pub mismatched: bool,
}

impl Pin {
    /// Whether `key` is one this account had before, retired for good or
    /// previous: a leaf under it is a replaced key's.
    pub fn replaced(&self, key: &IdentityPublic) -> bool {
        self.retired.contains(key) || self.previous.iter().any(|previous| previous.identity == *key)
    }

    /// `changed_from` only while there's still that key to go back to.
    fn settle(&mut self) {
        if self.changed_from.is_some_and(|key| !self.previous.iter().any(|previous| previous.identity == key)) {
            self.changed_from = None;
        }
    }
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
                    Pin {
                        identity,
                        device_list: None,
                        highest_version: 0,
                        retired: Vec::new(),
                        previous: Vec::new(),
                        changed_unseen: false,
                        verified: false,
                        changed_from: None,
                        mismatched: false,
                        refused: Vec::new(),
                    },
                );
                Ok(())
            }
        }
    }

    /// Retires `account`'s key the service lists as replaced (GET keys'
    /// `replaced_identity_keys`) that a commit or proposal this endpoint
    /// holds proves: `held`, the certificate of the leaf behind it (see
    /// `Group::held_signer`), verified under that key, so the service can't
    /// retire a key it doesn't hold a certificate for, such as a friend's
    /// next one (Fugo's row 654 conditions). Refused whole, nothing changed,
    /// when the list names the key pinned now or `served` (the response's
    /// current key). Touches nothing else: not the current key, not
    /// verified or the notice. Whether it retired a key.
    ///
    /// And only a key older than the one pinned now, by the account's own
    /// signatures: the held leaf's certificate made before the latest list
    /// the pinned key signed. A service still serving an old key can't list
    /// the account's real next one as replaced and have it retired from a
    /// genuine commit under it (Fugo's stale-answer wedge); with no list
    /// taken yet, nothing is retired until one is.
    pub fn replaced(&mut self, account: &str, served: [u8; 32], listed: &[[u8; 32]], held: &LeafCertificate) -> Result<bool, Error> {
        let Some(pin) = self.pins.get_mut(account) else { return Ok(false) };
        if listed.iter().any(|key| *key == pin.identity.0 || *key == served) {
            return Err(refused("the service lists a current security key as replaced"));
        }
        let key = held.identity;
        if held.leaf.account_id != account || !listed.contains(&key.0) || key.0 == served || pin.retired.contains(&key) {
            return Ok(false);
        }
        let older =
            self.lists.get(account).is_some_and(|list| list.identity == pin.identity && held.leaf.created_ms < list.list.created_ms);
        if !older {
            return Ok(false);
        }
        // Proven: retired for good, previous or not.
        pin.previous.retain(|previous| previous.identity != key);
        pin.retired.push(key);
        pin.settle();
        Ok(true)
    }

    /// `account`'s security key changed to `identity`: the old one becomes
    /// previous (kept with its list and highest version), and sending to
    /// them waits until the person has seen the notice (`seen`). A previous
    /// key may come back the same way, as an ordinary change with its own
    /// list and version again (L1), so one lying answer from the service
    /// doesn't end a conversation (Fugo's E11-3); a retired one never does
    /// (L5).
    pub fn key_changed(&mut self, account: &str, identity: IdentityPublic) -> Result<(), Error> {
        let pin = self.pins.get_mut(account).ok_or_else(|| refused("nothing pinned for this account to change"))?;
        if pin.identity == identity {
            return Ok(());
        }
        if pin.retired.contains(&identity) {
            return Err(refused("that security key was replaced; it's never trusted again"));
        }
        let back = pin.previous.iter().position(|previous| previous.identity == identity).map(|at| pin.previous.remove(at));
        let left = Previous {
            identity: pin.identity,
            device_list: pin.device_list.take(),
            highest_version: pin.highest_version,
            verified: pin.verified,
            mismatched: pin.mismatched,
        };
        pin.previous.retain(|previous| previous.identity != left.identity);
        pin.changed_from = Some(left.identity);
        pin.previous.push(left);
        pin.identity = identity;
        pin.device_list = back.as_ref().and_then(|back| back.device_list.clone());
        pin.highest_version = back.as_ref().map_or(0, |back| back.highest_version);
        pin.mismatched = back.as_ref().is_some_and(|back| back.mismatched);
        pin.changed_unseen = true;
        pin.verified = false;
        self.lists.remove(account);
        self.reparse()
    }

    /// The person compared safety numbers after `account`'s key changed and
    /// they didn't match: `back_to`, the key the notice said it changed
    /// from, is pinned again as it was (its list, version and whether it was
    /// verified), the change counts as seen, and the key refused becomes
    /// previous, marked as one the person said didn't match, so it can only
    /// come back as another visible change, which says so (`mismatched`).
    /// Refused when `back_to` isn't one of the account's previous keys
    /// (never whichever is last: import orders them its own way);
    /// `Pin::changed_from` names the one the notice was about.
    pub fn undo_change(&mut self, account: &str, back_to: &IdentityPublic) -> Result<(), Error> {
        let pin = self.pins.get_mut(account).ok_or_else(|| refused("nothing pinned for this account"))?;
        let at = pin
            .previous
            .iter()
            .position(|previous| previous.identity == *back_to)
            .ok_or_else(|| refused("that isn't an earlier security key to go back to"))?;
        let back = pin.previous.remove(at);
        let refused_key = Previous {
            identity: pin.identity,
            device_list: pin.device_list.take(),
            highest_version: pin.highest_version,
            verified: false,
            mismatched: true,
        };
        pin.identity = back.identity;
        pin.device_list = back.device_list;
        pin.highest_version = back.highest_version;
        pin.verified = back.verified;
        pin.changed_unseen = false;
        pin.changed_from = None;
        pin.mismatched = false;
        if !pin.refused.contains(&refused_key.identity) {
            pin.refused.push(refused_key.identity);
        }
        pin.previous.retain(|previous| previous.identity != refused_key.identity);
        pin.previous.push(refused_key);
        self.lists.remove(account);
        self.reparse()
    }

    /// The person's own devices replaced `account`'s keys themselves (a
    /// start-over, a rotation they made or took from their self group): its
    /// previous keys are retired for good (L5), never to come back as a
    /// change the service could steer.
    pub fn retire_previous(&mut self, account: &str) {
        if let Some(pin) = self.pins.get_mut(account) {
            for previous in std::mem::take(&mut pin.previous) {
                if !pin.retired.contains(&previous.identity) {
                    pin.retired.push(previous.identity);
                }
            }
            pin.settle();
        }
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
        pin.mismatched = false;
        pin.refused.retain(|key| key != identity);
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
        if pin.replaced(&list.identity) {
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
    ///
    /// A key that changes here this way shows as any key change does (Fugo
    /// 40159, Melone's F1): not yet seen, so sending to them waits for the
    /// person's notice, and not verified. How the other device saw or
    /// compared it never comes with it, so a forged pins content can't swap
    /// a friend's key silently.
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
                        pin.changed_from = ours.changed_from.or(their.changed_from);
                        pin.mismatched = (ours.mismatched || their.mismatched) && !pin.verified;
                        pin
                    } else if their.replaced(&ours.identity) && !ours.replaced(&their.identity) {
                        // That device saw this key replaced: its newer one
                        // wins, as a change the person hasn't seen here.
                        let mut pin = their.clone();
                        pin.changed_unseen = true;
                        pin.verified = false;
                        pin.changed_from = Some(ours.identity);
                        pin
                    } else {
                        ours.clone()
                    };
                    // A key either device retired never comes back (L5).
                    if retired.contains(&pin.identity) {
                        return Err(refused(format!("your devices disagree about {account}'s security key")));
                    }
                    // Keys either device moved off, each once (the higher
                    // version's), never the one pinned now or a retired one.
                    let mut previous: Vec<Previous> = Vec::new();
                    for key in ours.previous.iter().chain(&their.previous) {
                        if key.identity == pin.identity || retired.contains(&key.identity) {
                            continue;
                        }
                        match previous.iter_mut().find(|kept| kept.identity == key.identity) {
                            Some(kept) => {
                                let mismatched = kept.mismatched || key.mismatched;
                                if kept.highest_version < key.highest_version {
                                    *kept = key.clone();
                                }
                                kept.mismatched = mismatched;
                            }
                            None => previous.push(key.clone()),
                        }
                    }
                    // Keys the person said didn't match, on either device.
                    let mut refused = ours.refused.clone();
                    refused.extend(their.refused.iter().filter(|key| !ours.refused.contains(key)));
                    pin.retired = retired;
                    pin.previous = previous;
                    pin.refused = refused;
                    pin.settle();
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
        self.pins.get(account).is_some_and(|pin| pin.replaced(key))
    }

    fn refused(&self, account: &str, key: &IdentityPublic) -> bool {
        self.pins.get(account).is_some_and(|pin| pin.refused.contains(key))
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

    /// Fugo's E11-3: one lying keys answer moves a device off a friend's
    /// real key; when the real key comes back, it's an ordinary key change
    /// (the notice, not verified, sending held), with the list and version
    /// it had, so L1 still holds. Never a key a held commit proved replaced.
    #[test]
    fn a_key_moved_off_on_the_services_word_can_come_back_as_a_change() {
        let (real, lie) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let mut pins = Pins::default();
        pins.first_contact("bo", real.public()).unwrap();
        pins.accept_list("bo", &list(&real, "bo", 5, &[(LeafKind::Device, "phone", [2; 32])])).unwrap();
        pins.verified("bo", &real.public()).unwrap();
        pins.seen("bo");

        // The lie, taken as a key change; then the real key again.
        pins.key_changed("bo", lie.public()).unwrap();
        assert!(pins.retired("bo", &real.public()), "a leaf under the real key is a replaced key's meanwhile");
        pins.seen("bo");
        pins.key_changed("bo", real.public()).unwrap();
        let back = pins.get("bo").unwrap();
        assert_eq!((back.identity, back.changed_unseen, back.verified), (real.public(), true, false), "an ordinary change");
        assert!(!pins.can_send("bo"), "sending waits for the notice");
        assert_eq!(back.highest_version, 5);
        assert!(back.device_list.is_some() && pins.lists.contains_key("bo"), "its list is back");
        // L1 as before the lie: older refused, the same taken, newer taken.
        assert!(pins.accept_list("bo", &list(&real, "bo", 4, &[])).is_err());
        pins.accept_list("bo", &list(&real, "bo", 5, &[(LeafKind::Device, "phone", [2; 32])])).unwrap();
        pins.accept_list("bo", &list(&real, "bo", 6, &[])).unwrap();
        assert!(pins.retired("bo", &lie.public()), "the lie is previous now");
        assert_eq!(pins.get("bo").unwrap().previous.len(), 1);

        // "The numbers don't match" after a change: the key before is back
        // as it was, and the refused one is previous.
        let mut undone = Pins::default();
        undone.first_contact("bo", real.public()).unwrap();
        undone.accept_list("bo", &list(&real, "bo", 5, &[(LeafKind::Device, "phone", [2; 32])])).unwrap();
        undone.verified("bo", &real.public()).unwrap();
        assert!(undone.undo_change("bo", &real.public()).is_err(), "nothing to go back to");
        undone.key_changed("bo", lie.public()).unwrap();
        // Another of the person's devices synced its own earlier key in
        // meanwhile: the undo goes back to the key the notice named, not
        // whichever earlier key is last (Fugo 40783).
        let third = IdentityKey::generate().unwrap();
        let mut synced = Pins::default();
        synced.first_contact("bo", third.public()).unwrap();
        synced.key_changed("bo", lie.public()).unwrap();
        undone.import(&synced).unwrap();
        assert_eq!(undone.get("bo").unwrap().previous.last().map(|previous| previous.identity), Some(third.public()));
        assert!(undone.undo_change("bo", &IdentityKey::generate().unwrap().public()).is_err(), "never a key it didn't have");
        let changed_from = undone.get("bo").unwrap().changed_from;
        assert_eq!(changed_from, Some(real.public()), "the key the notice is about, through the sync");
        undone.undo_change("bo", &changed_from.unwrap()).unwrap();
        let pin = undone.get("bo").unwrap();
        assert_eq!((pin.identity, pin.verified, pin.changed_unseen, pin.highest_version), (real.public(), true, false, 5));
        assert!(undone.can_send("bo") && undone.lists.contains_key("bo"));
        assert!(pin.retired.is_empty() && pin.replaced(&lie.public()));
        assert_eq!((pin.changed_from, pin.mismatched), (None, false), "nothing to undo now");
        // The refused key served again: a visible change that says the
        // person found it didn't match, until they verify it after all.
        undone.key_changed("bo", lie.public()).unwrap();
        let pin = undone.get("bo").unwrap();
        assert_eq!((pin.mismatched, pin.changed_from, pin.changed_unseen), (true, Some(real.public()), true));
        let mut synced_again = Pins::default();
        synced_again.import(&undone).unwrap();
        assert!(synced_again.get("bo").unwrap().mismatched, "the person's word reaches their other devices");
        undone.verified("bo", &lie.public()).unwrap();
        assert!(!undone.get("bo").unwrap().mismatched);
        undone.undo_change("bo", &real.public()).unwrap();

        // The lie spread to another of the person's devices through the
        // self group: there too the real key stays previous, never retired,
        // and comes back the same way (Fugo 40412).
        let mut lied = Pins::default();
        lied.first_contact("bo", real.public()).unwrap();
        lied.key_changed("bo", lie.public()).unwrap();
        let mut other = Pins::default();
        other.first_contact("bo", real.public()).unwrap();
        other.import(&lied).unwrap();
        let taken = other.get("bo").unwrap();
        assert_eq!((taken.identity, taken.changed_unseen), (lie.public(), true));
        assert!(taken.retired.is_empty() && taken.replaced(&real.public()));
        other.key_changed("bo", real.public()).unwrap();
        // Two devices each past the other's key keep their own (each takes
        // the real one back from the service's next honest answer), and
        // neither retires anything.
        lied.import(&pins).unwrap();
        assert_eq!(lied.get("bo").unwrap().identity, lie.public());
        assert!(lied.get("bo").unwrap().retired.is_empty());
        lied.key_changed("bo", real.public()).unwrap();
    }

    /// The other side of E11-3: a key a commit this endpoint holds proves
    /// replaced (`Pins::replaced`) is retired for good, previous or not
    /// (L5), and another device's retired keys stay retired here.
    #[test]
    fn a_key_proven_replaced_never_comes_back() {
        let (old, new) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let mut pins = Pins::default();
        pins.first_contact("bo", old.public()).unwrap();
        pins.key_changed("bo", new.public()).unwrap();
        let later = DeviceList { account_id: "bo".to_owned(), version: 1, created_ms: 1_763_800_000_000, leaves: Vec::new() };
        pins.accept_list("bo", &later.sign(&new).unwrap().to_bytes().unwrap()).unwrap();
        let held = LeafCertificate::issue(&old, leaf(LeafKind::Device, "bo", "phone", [4; 32])).unwrap();
        assert!(pins.replaced("bo", [9; 32], &[old.public().0], &held).unwrap());
        let pin = pins.get("bo").unwrap();
        assert!(pin.retired.contains(&old.public()) && pin.previous.is_empty());
        assert!(pins.key_changed("bo", old.public()).is_err(), "never again (L5)");

        // Another device that took the proof: here too, for good.
        let mut other = Pins::default();
        other.first_contact("bo", old.public()).unwrap();
        other.import(&pins).unwrap();
        assert_eq!(other.get("bo").unwrap().identity, new.public());
        assert!(other.key_changed("bo", old.public()).is_err());
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
        // The old key is previous: no list from it while it isn't pinned, and
        // a leaf under it counts as a replaced key's.
        assert!(pins.accept_list("ana", &list(&old, "ana", 6, &[])).is_err());
        assert!(pins.retired("ana", &old.public()));

        // Pins from another of the person's devices: a newer key wins only
        // if that device retired this one, and retired keys stay retired.
        let mut stale = Pins::default();
        stale.first_contact("ana", old.public()).unwrap();
        stale.import(&pins).unwrap();
        assert_eq!(stale.get("ana").unwrap().identity, new.public());
        assert!(stale.retired("ana", &old.public()));
        // A change shown as one here, whatever the other device says it saw
        // (Fugo 40159): a forged pins content can't swap a key silently.
        let mut forged = pins.clone();
        forged.verified("ana", &new.public()).unwrap();
        let mut here = Pins::default();
        here.first_contact("ana", old.public()).unwrap();
        here.verified("ana", &old.public()).unwrap();
        here.import(&forged).unwrap();
        let taken = here.get("ana").unwrap();
        assert_eq!((taken.identity, taken.changed_unseen, taken.verified), (new.public(), true, false));
        assert!(!here.can_send("ana"), "sending waits for the person's notice");
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
