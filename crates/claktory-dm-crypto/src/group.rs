//! An endpoint's groups (dms-e2ee.md §4): the person's devices and a
//! Claktory's leaf use MLS only through this, so every member, message and
//! commit passes `validation` before it counts. A commit that fails is never
//! merged; a message that fails is dropped, with why.
//!
//! What goes to the worker comes back as bytes (`Outgoing`); the caller
//! sends them and merges its own commit (`merge_pending`) only once the
//! worker took it, as the one commit for that epoch.

use ed25519_dalek::{Signer as _, SigningKey};
use openmls::prelude::{
    Capabilities, Credential, CredentialWithKey, ExtensionType, GroupId, KeyPackage, KeyPackageIn, LeafNodeIndex, LeafNodeParameters,
    Lifetime, MlsGroup, MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageBodyIn, MlsMessageIn, MlsMessageOut,
    PURE_PLAINTEXT_WIRE_FORMAT_POLICY, ProcessedMessageContent, ProtocolVersion, Sender, SignaturePublicKey, StagedWelcome,
};
use openmls_traits::OpenMlsProvider;
use openmls_traits::signatures::{Signer, SignerError};
use openmls_traits::types::SignatureScheme;
use tls_codec::{Deserialize as _, Serialize as _};
use zeroize::Zeroizing;

use crate::certificate::{Leaf, LeafCertificate, LeafKind};
use crate::validation::{self, GroupKind, Member, Trust};
use crate::{CIPHERSUITE, Error};

/// A leaf's MLS signature key (Ed25519), kept by its device or Claktory.
pub struct LeafKey {
    key: SigningKey,
}

impl LeafKey {
    pub fn generate() -> Result<LeafKey, Error> {
        let mut seed = Zeroizing::new([0u8; 32]);
        getrandom::fill(seed.as_mut()).map_err(|error| Error::Format(format!("no randomness: {error}")))?;
        Ok(LeafKey { key: SigningKey::from_bytes(&seed) })
    }

    pub fn from_secret(secret: &[u8; 32]) -> LeafKey {
        LeafKey { key: SigningKey::from_bytes(secret) }
    }

    pub fn secret(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.key.to_bytes())
    }

    pub fn public(&self) -> [u8; 32] {
        self.key.verifying_key().to_bytes()
    }
}

impl Signer for LeafKey {
    fn sign(&self, payload: &[u8]) -> Result<Vec<u8>, SignerError> {
        Ok(self.key.sign(payload).to_bytes().to_vec())
    }

    fn signature_scheme(&self) -> SignatureScheme {
        SignatureScheme::ED25519
    }
}

/// This endpoint's own leaf: its key and the certificate a device of the
/// person's signed for it.
pub struct Me {
    pub key: LeafKey,
    pub certificate: LeafCertificate,
}

impl Me {
    pub fn new(key: LeafKey, certificate: LeafCertificate) -> Result<Me, Error> {
        if certificate.leaf.signature_key != key.public() {
            return Err(Error::Refused("the certificate is for another leaf key".to_owned()));
        }
        Ok(Me { key, certificate })
    }

    /// The same leaf key under a new certificate: the person made a new
    /// identity key (dms-e2ee §2.1), and a device of theirs certified this
    /// leaf again. Account, leaf id and kind stay; only the identity moves.
    pub fn recertified(&self, certificate: LeafCertificate) -> Result<Me, Error> {
        let leaf = &self.certificate.leaf;
        let new = &certificate.leaf;
        if new.signature_key != leaf.signature_key
            || new.account_id != leaf.account_id
            || new.leaf_id != leaf.leaf_id
            || new.kind != leaf.kind
        {
            return Err(Error::Refused("a new certificate for another leaf".to_owned()));
        }
        Me::new(LeafKey::from_secret(&self.key.secret()), certificate)
    }

    fn credential(&self) -> Result<CredentialWithKey, Error> {
        Ok(CredentialWithKey {
            credential: self.certificate.to_credential()?,
            signature_key: SignaturePublicKey::from(self.key.public().to_vec()),
        })
    }

    /// A one-time key package for the worker's supply, as bytes. It lasts
    /// [`KEY_PACKAGE_LIFETIME`] (Melone's F4).
    pub fn key_package(&self, provider: &impl OpenMlsProvider) -> Result<Vec<u8>, Error> {
        let bundle = KeyPackage::builder()
            .key_package_lifetime(Lifetime::new(KEY_PACKAGE_LIFETIME))
            .build(CIPHERSUITE, provider, &self.key, self.credential()?)
            .map_err(mls)?;
        let message = MlsMessageOut::from(bundle.key_package().clone());
        message.tls_serialize_detached().map_err(mls)
    }

    /// The last-resort KeyPackage: the worker hands it out again once the
    /// one-time ones are gone, so its keys stay until a new one replaces it.
    /// It lasts [`KEY_PACKAGE_LIFETIME`] too; a leaf replaces it once it's
    /// within [`LAST_RESORT_RENEW`] of the end (Melone's F4).
    pub fn last_resort_key_package(&self, provider: &impl OpenMlsProvider) -> Result<Vec<u8>, Error> {
        // Its leaf says it understands the last-resort extension it carries.
        let capabilities = Capabilities::new(None, None, Some(&[ExtensionType::LastResort]), None, None);
        let bundle = KeyPackage::builder()
            .leaf_node_capabilities(capabilities)
            .mark_as_last_resort()
            .key_package_lifetime(Lifetime::new(KEY_PACKAGE_LIFETIME))
            .build(CIPHERSUITE, provider, &self.key, self.credential()?)
            .map_err(mls)?;
        let message = MlsMessageOut::from(bundle.key_package().clone());
        message.tls_serialize_detached().map_err(mls)
    }
}

/// How long a key package lasts, one-time or last resort (Melone's F4,
/// Fugo 40159): 30 days, not OpenMLS's 12 weeks, so a store taken later opens
/// no Welcome a sequencer held back past that.
pub const KEY_PACKAGE_LIFETIME: u64 = 30 * 24 * 60 * 60;
/// A last resort this close to its end is replaced at the next top-up.
pub const LAST_RESORT_RENEW: u64 = 7 * 24 * 60 * 60;

/// Refuses an MLS ciphersuite other than Claktory's one (Melone's F7):
/// every group and key package is suite 1, and nothing is taken in another.
fn our_suite(suite: openmls::prelude::Ciphersuite) -> Result<(), Error> {
    if suite != CIPHERSUITE {
        return Err(Error::Refused(format!("a group or key package in another ciphersuite ({suite:?})")));
    }
    Ok(())
}

fn mls(error: impl std::fmt::Debug) -> Error {
    Error::Mls(format!("{error:?}"))
}

/// The new epoch's GroupInfo, as an MLS message, for the worker to keep
/// for rejoins (the ratchet tree rides in it).
fn group_info(info: Option<openmls::messages::group_info::GroupInfo>) -> Result<Option<Vec<u8>>, Error> {
    info.map(|info| MlsMessageOut::from(info).tls_serialize_detached()).transpose().map_err(mls)
}

fn read(bytes: &[u8]) -> Result<MlsMessageBodyIn, Error> {
    MlsMessageIn::tls_deserialize_exact(bytes).map(MlsMessageIn::extract).map_err(|e| Error::NotMls(e.to_string()))
}

/// What a change to the group sends: the commit, and for an Add the Welcome;
/// with the GroupInfo for the new epoch, which the worker keeps for rejoins.
#[derive(Debug, Clone)]
pub struct Outgoing {
    pub commit: Vec<u8>,
    pub welcome: Option<Vec<u8>>,
    pub group_info: Option<Vec<u8>>,
}

/// What a message from the worker was.
#[derive(Debug)]
pub enum Incoming {
    /// Someone said something: who, the content's bytes (`envelope`'s to
    /// read), and the epoch it was said in (Melone's F6): a key-bearing
    /// content counts only from an epoch its caller's rules allow, such as
    /// one after the last Remove of the person's own leaves.
    Application { from: Box<Member>, content: Vec<u8>, epoch: u64 },
    /// A proposal, kept until a commit covers it.
    Proposal,
    /// A commit, checked and merged: the group is at its next epoch.
    Committed { epoch: u64 },
}

/// The most items a window takes before it's given up (row 561, Fugo).
pub const WINDOW_MOST: usize = 64;

/// An item a window took (`Group::process_window`): what it was, as
/// `process` would say, and whether it came from the window's leaf, a
/// device its person has since taken out.
#[derive(Debug)]
pub struct Windowed {
    pub incoming: Result<Incoming, Error>,
    pub departed: bool,
}

/// What `Group::process_window` came to.
#[derive(Debug)]
pub enum Window {
    /// Closed by the removal: `taken[i]` is what `items[i]` was, from the
    /// commit that opened it to the one that took the leaf out. Keep the
    /// transaction: the group is at the removal's epoch.
    Closed { taken: Vec<Windowed> },
    /// The items ran out before the removal: roll back, and take more once
    /// there are more (as long as there are more commits to come).
    Open,
    /// It can't close: roll back, and the commit stays refused.
    Aborted { why: String },
}

/// One group this endpoint is in.
pub struct Group {
    mls: MlsGroup,
    kind: GroupKind,
}

// Handshake messages (proposals, commits) go as PublicMessages, so the delivery
// service can check them before fanning them out (dms-e2ee.md §4); OpenMLS
// refuses one that comes as a PrivateMessage. Application messages are
// always encrypted, whatever the policy.
//
// The worker takes an application message up to five epochs older than the
// group's (a send that raced a commit), so every member keeps that many past
// epochs' secrets to read it.
const PAST_EPOCHS: usize = 5;

fn create_config() -> MlsGroupCreateConfig {
    MlsGroupCreateConfig::builder()
        .ciphersuite(CIPHERSUITE)
        .use_ratchet_tree_extension(true)
        .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
        .max_past_epochs(PAST_EPOCHS)
        .build()
}

fn join_config() -> MlsGroupJoinConfig {
    MlsGroupJoinConfig::builder()
        .use_ratchet_tree_extension(true)
        .wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY)
        .max_past_epochs(PAST_EPOCHS)
        .build()
}

impl Group {
    /// A new group with only this device in it.
    pub fn create(provider: &impl OpenMlsProvider, me: &Me, kind: GroupKind, group_id: &[u8]) -> Result<Group, Error> {
        // Every group starts on one of the person's devices: a Claktory is
        // only ever added.
        if me.certificate.leaf.kind != LeafKind::Device {
            return Err(Error::Refused("only a device starts a group".to_owned()));
        }
        let mls = MlsGroup::new_with_group_id(provider, &me.key, &create_config(), GroupId::from_slice(group_id), me.credential()?)
            .map_err(mls)?;
        Ok(Group { mls, kind })
    }

    /// The group as its storage keeps it.
    pub fn load(provider: &impl OpenMlsProvider, kind: GroupKind, group_id: &[u8]) -> Result<Option<Group>, Error> {
        let mls = MlsGroup::load(provider.storage(), &GroupId::from_slice(group_id)).map_err(mls)?;
        Ok(mls.map(|mls| Group { mls, kind }))
    }

    /// Joins from a Welcome to the group `group_id` the caller expects for
    /// `kind`: who added us must be a device of a member's (the owner's, in
    /// a self group or a channel), and every member is checked, before the
    /// group is kept. A Welcome from a leaf that couldn't have committed
    /// that Add is a fork the others never took.
    pub fn join(
        provider: &impl OpenMlsProvider,
        trust: &impl Trust,
        kind: GroupKind,
        group_id: &[u8],
        welcome: &[u8],
    ) -> Result<Group, Error> {
        let MlsMessageBodyIn::Welcome(welcome) = read(welcome)? else { return Err(Error::NotMls("not a welcome".to_owned())) };
        our_suite(welcome.ciphersuite())?;
        let staged = StagedWelcome::new_from_welcome(provider, &join_config(), welcome, None).map_err(mls)?;
        our_suite(staged.group_context().ciphersuite())?;
        if staged.group_context().group_id().as_slice() != group_id {
            return Err(Error::Refused("a welcome to another group than this one".to_owned()));
        }
        let sender = staged.welcome_sender().map_err(mls)?;
        let adder = validation::member(&kind, trust, sender.credential(), sender.signature_key().as_slice())?;
        if adder.kind() != LeafKind::Device {
            return Err(Error::Refused("only a member's device adds a leaf".to_owned()));
        }
        // A Claktory is added only by a device of its own person.
        if let Some(own) = staged.own_leaf_node()
            && let Ok(certificate) = LeafCertificate::from_credential(own.credential())
            && certificate.leaf.kind == LeafKind::Claktory
            && certificate.leaf.account_id != adder.account()
        {
            return Err(Error::Refused("only a device of the same person adds a Claktory".to_owned()));
        }
        let members: Vec<(Credential, Vec<u8>)> =
            staged.members().map(|member| (member.credential.clone(), member.signature_key.clone())).collect();
        validation::joined(&kind, trust, members.iter().map(|(credential, key)| (credential, key.as_slice())))?;
        let mls = staged.into_group(provider).map_err(mls)?;
        Ok(Group { mls, kind })
    }

    /// Rejoins with an external commit (dms-e2ee.md §4, Rejoin): a device
    /// whose state broke or that was away past the mailbox, from the
    /// GroupInfo the worker keeps (the ratchet tree rides in it). With the
    /// same certificate, its earlier leaf is removed in the same commit.
    /// After a start-over (row 630), the group may hold its own account's
    /// dead leaves (`validation::dead`): it joins past them, and takes them
    /// out with `remove_dead` as soon as the worker takes this commit
    /// (OpenMLS puts no other leaf's Remove in an external commit).
    /// Every member is checked first; a Claktory never rejoins a
    /// conversation by itself (H1). The GroupInfo must be `group_id`'s: a
    /// service could otherwise hand over another group with members the
    /// kind accepts (the self group's for a channel, an older generation's
    /// for a conversation), kept then under the wrong id (Fugo). The commit
    /// goes to the worker; once it takes it, `merge_pending`.
    ///
    /// `floor` is the highest epoch this endpoint ever merged in the group
    /// (`storage::epoch_floor`): a GroupInfo below it is refused, so a
    /// sequencer can't fork this endpoint back past a Remove it merged
    /// (Melone's F2, Fugo 40159).
    pub fn rejoin(
        provider: &impl OpenMlsProvider,
        me: &Me,
        trust: &impl Trust,
        kind: GroupKind,
        group_id: &[u8],
        info: &[u8],
        floor: u64,
    ) -> Result<(Group, Outgoing), Error> {
        if matches!(kind, GroupKind::Conversation { .. }) && me.certificate.leaf.kind == LeafKind::Claktory {
            return Err(Error::Refused("a Claktory doesn't join a conversation by itself".to_owned()));
        }
        let MlsMessageBodyIn::GroupInfo(info) = read(info)? else { return Err(Error::NotMls("not a group info".to_owned())) };
        // As the GroupInfo says before its signature is checked; checked
        // below, it's the group's own.
        if info.group_id().as_slice() != group_id {
            return Err(Error::Refused("a GroupInfo of another group than this one".to_owned()));
        }
        our_suite(info.group_context().ciphersuite())?;
        // Its epoch, likewise checked with the signature below.
        if info.epoch().as_u64() < floor {
            return Err(Error::Refused(format!(
                "a GroupInfo from epoch {}, before epoch {floor} this device already reached",
                info.epoch().as_u64()
            )));
        }
        let (joined, bundle) = MlsGroup::external_commit_builder()
            .with_config(join_config())
            .build_group(provider, info, me.credential()?)
            .map_err(mls)?
            .load_psks(provider.storage())
            .map_err(mls)?
            .build(provider.rand(), provider.crypto(), &me.key, |_| true)
            .map_err(mls)?
            .finalize(provider)
            .map_err(mls)?;
        let group = Group { mls: joined, kind };
        // The members it joins, as the tree it was given has them. A dead
        // or stale leaf of its own account's (a start-over's, row 630; its
        // own former install, row 202) it takes out at once (`remove_dead`),
        // before anything is sent; anything else must pass. A device it
        // kept, on its own new list, is present past its old key.
        let rejoining = validation::Rejoining { trust, me: &me.certificate };
        for member in group.mls.members() {
            if member.index == group.mls.own_leaf_index() {
                continue;
            }
            if !Self::ours_to_take_out(trust, me, &member) {
                validation::present(&group.kind, &rejoining, &member.credential, &member.signature_key)?;
            }
        }
        let (commit, _, info) = bundle.into_contents();
        Ok((group, Outgoing { commit: commit.tls_serialize_detached().map_err(mls)?, welcome: None, group_info: group_info(info)? }))
    }

    /// The group's GroupInfo at its current epoch, with the ratchet tree,
    /// signed by this member: what `POST groups` takes from the creator at
    /// epoch 0, so the worker can start the group's public state.
    pub fn group_info(&self, provider: &impl OpenMlsProvider, me: &Me) -> Result<Vec<u8>, Error> {
        self.mls.export_group_info(provider.crypto(), &me.key, true).map_err(mls)?.tls_serialize_detached().map_err(mls)
    }

    pub fn group_id(&self) -> &[u8] {
        self.mls.group_id().as_slice()
    }

    pub fn epoch(&self) -> u64 {
        self.mls.epoch().as_u64()
    }

    pub fn kind(&self) -> &GroupKind {
        &self.kind
    }

    /// Whether a commit it took removed this endpoint's own leaf: it reads
    /// nothing more here, and rejoins (a channel's Claktory) or is added
    /// again.
    pub fn removed(&self) -> bool {
        !self.mls.is_active()
    }

    /// The members as their certificates say, each checked now.
    pub fn members(&self, trust: &impl Trust) -> Vec<Result<Member, Error>> {
        self.mls.members().map(|member| validation::member(&self.kind, trust, &member.credential, &member.signature_key)).collect()
    }

    /// Every member as a joiner checks it (`validation::present`): as
    /// `members`, but a device the person kept when they made a new key,
    /// still under the old one and named on their current list, passes
    /// (inert until it moves). For who's in a group being joined, never
    /// for trusting what a member says.
    pub fn present(&self, trust: &impl Trust) -> Vec<Result<Member, Error>> {
        self.mls.members().map(|member| validation::present(&self.kind, trust, &member.credential, &member.signature_key)).collect()
    }

    /// Every leaf's certificate, in tree order, **unchecked**, as
    /// `leaves_unchecked`, with the identity that signed it: for telling a
    /// leaf under a replaced key apart (never sign one out on a sweep;
    /// Fugo), never for trusting anyone.
    pub fn certificates_unchecked(&self) -> Vec<LeafCertificate> {
        self.mls.members().filter_map(|member| LeafCertificate::from_credential(&member.credential).ok()).collect()
    }

    /// The group's dead leaves as `me` sees them (`validation::dead`), in
    /// tree order.
    pub fn dead(&self, trust: &impl Trust, me: &Me) -> Vec<LeafCertificate> {
        self.mls
            .members()
            .filter_map(|member| validation::dead(trust, &me.certificate, &member.credential, &member.signature_key))
            .collect()
    }

    /// The group's stale leaves (`validation::stale`: former installs of
    /// devices now listed under a new key), in tree order.
    pub fn stale(&self, trust: &impl Trust) -> Vec<LeafCertificate> {
        self.mls.members().filter_map(|member| validation::stale(trust, &member.credential, &member.signature_key)).collect()
    }

    /// The account whose dead device leaf or stale leaf is still in the
    /// group, while one is, and which: nothing is sent here until it's
    /// taken out, since it would read it (row 630, row 202; Fugo), at the
    /// person's own devices and at friends'. A Claktory's leaf under an old
    /// key doesn't hold sends: after a new key it waits, off the list, until
    /// a device enrolls it again, and it's the person's own.
    pub fn held(&self, trust: &impl Trust, me: &Me) -> Option<(String, validation::Held)> {
        // A device's; a Claktory's only under a key the person said didn't
        // match (a liar's leaf, planted while it was pinned: Fugo 41542),
        // never one under a key a real rotation retired.
        let dead = self
            .dead(trust, me)
            .into_iter()
            .find(|dead| dead.leaf.kind == LeafKind::Device || trust.refused(&dead.leaf.account_id, &dead.identity));
        let dead = dead.map(|dead| (dead.leaf.account_id, validation::Held::Replaced));
        dead.or_else(|| self.stale(trust).into_iter().next().map(|stale| (stale.leaf.account_id, validation::Held::Reinstalled)))
    }

    /// The account `held` names.
    pub fn waiting(&self, trust: &impl Trust, me: &Me) -> Option<String> {
        self.held(trust, me).map(|(account, _)| account)
    }

    /// Whether `member` is a leaf of `me`'s own account that `me` takes out
    /// (`remove_dead`): dead or stale.
    fn ours_to_take_out(trust: &impl Trust, me: &Me, member: &openmls::group::Member) -> bool {
        let own = &me.certificate.leaf.account_id;
        let dead = validation::dead(trust, &me.certificate, &member.credential, &member.signature_key);
        let stale = || validation::stale(trust, &member.credential, &member.signature_key);
        dead.or_else(stale).is_some_and(|gone| gone.leaf.account_id == *own)
    }

    /// Every leaf's id as its certificate names it, in tree order,
    /// **unchecked**: for telling who came and went across a commit, not
    /// for trusting anyone (`members` does that). A leaf under a key its
    /// account replaced is still here; one whose credential isn't a
    /// certificate isn't.
    pub fn leaf_ids(&self) -> Vec<String> {
        self.leaves_unchecked().into_iter().map(|leaf| leaf.leaf_id).collect()
    }

    /// Every leaf as its certificate claims (account, kind, id, name),
    /// in tree order, **unchecked**: for removal decisions only, since
    /// removing a leaf takes access away, never grants it (a leaf taken
    /// off its list, revoked or under a replaced key fails `members`' checks
    /// but must still be removable). Never for trusting what a leaf says or
    /// showing who's in (`members` does that). One whose credential isn't a
    /// certificate isn't here, as in `leaf_ids`.
    pub fn leaves_unchecked(&self) -> Vec<Leaf> {
        self.mls
            .members()
            .filter_map(|member| LeafCertificate::from_credential(&member.credential).ok())
            .map(|certificate| certificate.leaf)
            .collect()
    }

    fn leaf(&self, index: LeafNodeIndex) -> Option<(Credential, Vec<u8>)> {
        self.mls.member_at(index).map(|member| (member.credential, member.signature_key))
    }

    /// Who sent a message OpenMLS checked (Melone's F1, Fugo 40159): the
    /// leaf at `index` now, its credential and key, only while it's the very
    /// leaf that signed it. `signed` is OpenMLS's credential for the sender
    /// in the message's own epoch. A message from a past epoch whose index
    /// another leaf holds now (a removed device's, a revoked Claktory's), or
    /// whose leaf has changed its certificate since, is refused: it would
    /// otherwise pass as whoever sits there now.
    fn signer(&self, index: LeafNodeIndex, signed: &Credential) -> Result<(Credential, Vec<u8>), Error> {
        let (credential, key) = self.leaf(index).ok_or_else(|| Error::Refused("a message from no member".to_owned()))?;
        if credential != *signed {
            return Err(Error::Refused("a message from a leaf that has left the group".to_owned()));
        }
        Ok((credential, key))
    }

    /// Whether this endpoint's own leaf here carries `me`'s certificate:
    /// not yet in a group it hasn't moved to the person's new identity key
    /// in (`update_certificate`).
    pub fn holds(&self, me: &Me) -> Result<bool, Error> {
        let own = self.mls.own_leaf_node().ok_or_else(|| Error::Refused("this endpoint isn't in the group".to_owned()))?;
        Ok(*own.credential() == me.certificate.to_credential()?)
    }

    /// Adds leaves from their key packages (as the worker handed them out):
    /// each checked as a member first, and a Claktory only by a device of
    /// the same person.
    pub fn add(
        &mut self,
        provider: &impl OpenMlsProvider,
        me: &Me,
        trust: &impl Trust,
        key_packages: &[Vec<u8>],
    ) -> Result<Outgoing, Error> {
        let mut packages: Vec<KeyPackage> = Vec::with_capacity(key_packages.len());
        for bytes in key_packages {
            let MlsMessageBodyIn::KeyPackage(package) = read(bytes)? else { return Err(Error::NotMls("not a key package".to_owned())) };
            let package: KeyPackage = KeyPackageIn::validate(package, provider.crypto(), ProtocolVersion::Mls10)
                .map_err(|_| Error::Refused("a key package that doesn't verify".to_owned()))?;
            our_suite(package.ciphersuite())?;
            let leaf = package.leaf_node();
            // A leaf already in the group (it rejoined while its add was on
            // the way, or the view here is behind), or a second package for
            // the same leaf: nothing to add. MLS would refuse the whole
            // commit for it, and every add after it the same way.
            let key = leaf.signature_key().as_slice();
            let there = self.mls.members().any(|member| member.signature_key == key)
                || packages.iter().any(|kept| kept.leaf_node().signature_key().as_slice() == key);
            if there {
                continue;
            }
            let new = validation::member(&self.kind, trust, leaf.credential(), leaf.signature_key().as_slice())?;
            if new.kind() == LeafKind::Claktory
                && (me.certificate.leaf.kind != LeafKind::Device || me.certificate.leaf.account_id != new.account())
            {
                return Err(Error::Refused("only a device of the same person adds a Claktory".to_owned()));
            }
            packages.push(package);
        }
        if packages.is_empty() {
            return Err(Error::NothingToAdd);
        }
        let (commit, welcome, info) = self.mls.add_members(provider, &me.key, &packages).map_err(mls)?;
        Ok(Outgoing {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            welcome: Some(welcome.tls_serialize_detached().map_err(mls)?),
            group_info: group_info(info)?,
        })
    }

    /// Removes this person's own leaves with these leaf ids (a device
    /// removed, a grant taken back): every leaf of their account with one,
    /// so never for a reinstalled device's former install, which shares its
    /// id with the new one (`remove_dead` takes that out by index). Another
    /// person's leaf is never taken out, even under the same id (Fugo, row
    /// 671).
    pub fn remove(&mut self, provider: &impl OpenMlsProvider, me: &Me, leaf_ids: &[&str]) -> Result<Outgoing, Error> {
        let account = me.certificate.leaf.account_id.clone();
        self.remove_of(provider, me, &account, leaf_ids)
    }

    /// Removes `account`'s leaves with these leaf ids, as `remove` does its
    /// own: only that account's.
    pub fn remove_of(&mut self, provider: &impl OpenMlsProvider, me: &Me, account: &str, leaf_ids: &[&str]) -> Result<Outgoing, Error> {
        let indexes: Vec<LeafNodeIndex> = self
            .mls
            .members()
            .filter(|member| {
                LeafCertificate::from_credential(&member.credential).is_ok_and(|certificate| {
                    certificate.leaf.account_id == account && leaf_ids.contains(&certificate.leaf.leaf_id.as_str())
                })
            })
            .map(|member| member.index)
            .collect();
        if indexes.is_empty() {
            return Err(Error::Refused("no such leaf in the group".to_owned()));
        }
        let (commit, welcome, info) = self.mls.remove_members(provider, &me.key, &indexes).map_err(mls)?;
        Ok(Outgoing {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            welcome: welcome.map(|welcome| welcome.tls_serialize_detached()).transpose().map_err(mls)?,
            group_info: group_info(info)?,
        })
    }

    /// Takes out this person's own dead leaves (`validation::dead`), after a
    /// start-over's rejoin (row 630), and stale ones (`validation::stale`),
    /// a reinstalled device's former install (row 202), whichever of their
    /// devices commits it: an ordinary commit of Removes, by tree index (a
    /// dead or stale leaf may have this leaf's own id, or the new install's),
    /// of nothing else. None when there are none. A leaf the current list
    /// names (one the person kept, the new install) and another person's
    /// leaf are never taken out here.
    pub fn remove_dead(&mut self, provider: &impl OpenMlsProvider, me: &Me, trust: &impl Trust) -> Result<Option<Outgoing>, Error> {
        let here = self.mls.own_leaf_index();
        let indexes: Vec<LeafNodeIndex> = self
            .mls
            .members()
            .filter(|member| member.index != here && Self::ours_to_take_out(trust, me, member))
            .map(|member| member.index)
            .collect();
        if indexes.is_empty() {
            return Ok(None);
        }
        let bundle = self
            .mls
            .commit_builder()
            .consume_proposal_store(false)
            .propose_removals(indexes)
            .load_psks(provider.storage())
            .map_err(mls)?
            .build(provider.rand(), provider.crypto(), &me.key, |_| true)
            .map_err(mls)?
            .stage_commit(provider)
            .map_err(mls)?;
        let (commit, welcome, info) = bundle.into_contents();
        Ok(Some(Outgoing {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            welcome: welcome.map(|welcome| welcome.tls_serialize_detached()).transpose().map_err(mls)?,
            group_info: group_info(info)?,
        }))
    }

    /// Pads only the next message this leaf makes, in its authenticated
    /// data, with `bytes` zeros: for tests of what an oversized commit costs.
    #[doc(hidden)]
    pub fn pad_next_for_test(&mut self, bytes: usize) {
        self.mls.set_aad(vec![0; bytes]);
    }

    /// This leaf's own Update: new keys, for post-compromise security.
    pub fn update(&mut self, provider: &impl OpenMlsProvider, me: &Me) -> Result<Outgoing, Error> {
        let bundle = self.mls.self_update(provider, &me.key, Default::default()).map_err(mls)?;
        let (commit, _, info) = bundle.into_contents();
        Ok(Outgoing { commit: commit.tls_serialize_detached().map_err(mls)?, welcome: None, group_info: group_info(info)? })
    }

    /// This leaf's move to the person's new identity key (dms-e2ee §2.1):
    /// an Update carrying `me`'s new certificate, with the same signature
    /// key, and nothing else (no waiting proposal rides along), the one
    /// commit a member under a replaced key may make.
    pub fn update_certificate(&mut self, provider: &impl OpenMlsProvider, me: &Me) -> Result<Outgoing, Error> {
        let parameters = LeafNodeParameters::builder().with_credential_with_key(me.credential()?).build();
        let bundle = self
            .mls
            .commit_builder()
            .consume_proposal_store(false)
            .force_self_update(true)
            .leaf_node_parameters(parameters)
            .load_psks(provider.storage())
            .map_err(mls)?
            .build(provider.rand(), provider.crypto(), &me.key, |_| true)
            .map_err(mls)?
            .stage_commit(provider)
            .map_err(mls)?;
        let (commit, _, info) = bundle.into_contents();
        Ok(Outgoing { commit: commit.tls_serialize_detached().map_err(mls)?, welcome: None, group_info: group_info(info)? })
    }

    /// Asks to leave: a proposal a device of the person's commits (MLS
    /// doesn't let a member commit its own removal). How a Claktory leaves.
    pub fn leave(&mut self, provider: &impl OpenMlsProvider, me: &Me) -> Result<Vec<u8>, Error> {
        let proposal = self.mls.leave_group(provider, &me.key).map_err(mls)?;
        proposal.tls_serialize_detached().map_err(mls)
    }

    /// The worker took this endpoint's commit: merge it.
    pub fn merge_pending(&mut self, provider: &impl OpenMlsProvider) -> Result<(), Error> {
        self.mls.merge_pending_commit(provider).map_err(mls)
    }

    /// The worker refused this endpoint's commit (another came first): drop it.
    pub fn clear_pending(&mut self, provider: &impl OpenMlsProvider) -> Result<(), Error> {
        self.mls.clear_pending_commit(provider.storage()).map_err(mls)
    }

    /// Encrypts `content` (`envelope`'s bytes) for the group: not while it
    /// holds a dead device leaf (`waiting`), which would read it.
    pub fn send(&mut self, provider: &impl OpenMlsProvider, me: &Me, trust: &impl Trust, content: &[u8]) -> Result<Vec<u8>, Error> {
        if matches!(self.kind, GroupKind::Conversation { .. }) && me.certificate.leaf.kind == LeafKind::Claktory {
            return Err(Error::Refused("a Claktory doesn't send messages in a conversation".to_owned()));
        }
        if let Some((account, reason)) = self.held(trust, me) {
            return Err(Error::Waiting { account, reason });
        }
        let message = self.mls.create_message(provider, &me.key, content).map_err(mls)?;
        message.tls_serialize_detached().map_err(mls)
    }

    /// The certificate of the leaf behind a commit or proposal this group
    /// can't take yet (`Error::UnknownIdentity`): its signature checked by
    /// MLS, its certificate by the identity it names, and nothing kept or
    /// merged. None for anything else. For `Pins::replaced`'s proof.
    pub fn held_signer(&mut self, provider: &impl OpenMlsProvider, bytes: &[u8]) -> Result<Option<LeafCertificate>, Error> {
        let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| Error::NotMls(e.to_string()))?;
        let Ok(protocol) = message.try_into_protocol_message() else { return Ok(None) };
        if matches!(protocol.content_type(), openmls::prelude::ContentType::Application) {
            return Ok(None);
        }
        let processed = self.mls.process_message(provider, protocol).map_err(mls)?;
        Ok(LeafCertificate::from_credential(processed.credential()).ok())
    }

    /// A message from the worker, in order: checked, then used. A commit
    /// that fails the checks isn't merged; a message that fails is dropped.
    pub fn process(&mut self, provider: &impl OpenMlsProvider, trust: &impl Trust, bytes: &[u8]) -> Result<Incoming, Error> {
        let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| Error::NotMls(e.to_string()))?;
        let protocol =
            message.try_into_protocol_message().map_err(|_| Error::NotMls("not a handshake or application message".to_owned()))?;
        let processed = self.mls.process_message(provider, protocol).map_err(mls)?;
        let sender = processed.sender().clone();
        let credential = processed.credential().clone();
        let epoch = processed.epoch().as_u64();
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => {
                let Sender::Member(index) = sender else { return Err(Error::Refused("a message from outside the group".to_owned())) };
                let (credential, key) = self.signer(index, &credential)?;
                let from = validation::application(&self.kind, trust, &credential, &key)?;
                Ok(Incoming::Application { from: Box::new(from), content: message.into_bytes(), epoch })
            }
            ProcessedMessageContent::ProposalMessage(proposal) => {
                let Sender::Member(index) = sender else { return Err(Error::Refused("a proposal from outside the group".to_owned())) };
                let (credential, key) = self.signer(index, &credential)?;
                validation::member(&self.kind, trust, &credential, &key)?;
                self.mls.store_pending_proposal(provider.storage(), *proposal).map_err(mls)?;
                Ok(Incoming::Proposal)
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                let leaf = |index: LeafNodeIndex| self.leaf(index);
                validation::commit(&self.kind, trust, &sender, &credential, &staged, &leaf)?;
                self.mls.merge_staged_commit(provider, *staged).map_err(mls)?;
                Ok(Incoming::Committed { epoch: self.epoch() })
            }
            // Its own commit back from the worker, the one it made and kept
            // pending (its confirmation tag says so): taken, so merged. How
            // an endpoint that never heard the worker's answer carries on.
            ProcessedMessageContent::OwnPendingCommit => {
                self.mls.merge_pending_commit(provider).map_err(mls)?;
                Ok(Incoming::Committed { epoch: self.epoch() })
            }
            _ => Err(Error::Refused("a kind of message Claktory doesn't use".to_owned())),
        }
    }

    /// A commit refused only because its account's latest device list
    /// doesn't name the leaf it brings in, taken anyway when that person's
    /// own device took the leaf out again (row 561, Fugo's rule): a device
    /// that was away while a friend added a device and removed it reads
    /// what was said meanwhile, as one that was there did, instead of
    /// rejoining past it.
    ///
    /// `items` are the group's next items in order, `items[0]` the refused
    /// commit. They're taken with that one leaf (the window's) counting as
    /// listed, every other check as ever, until a commit takes it out. It
    /// closes the window only when:
    /// - it's an ordinary commit (not an external one) by a leaf of the
    ///   same person that their latest list names and that passes every
    ///   check;
    /// - its committer has been in the group throughout, from before the
    ///   window's leaf came in: a rejoin's clean-up never closes one.
    ///
    /// It never opens outside a conversation (a self group carries keys and
    /// lists, a channel instructions to the person's own Claktory). The
    /// window's leaf may say things (marked `departed`) and commit its own
    /// Update with nothing else; anything else it proposes or commits
    /// aborts. So does a commit that fails, or more than [`WINDOW_MOST`]
    /// items.
    ///
    /// The caller runs it in a transaction (a savepoint) of its own: only
    /// `Window::Closed` is kept. After `Open` or `Aborted` the group here
    /// has moved on in memory: roll back and load it again. `Err` is the
    /// refusal as `process` gives it, when there's no window to open.
    pub fn process_window(&mut self, provider: &impl OpenMlsProvider, trust: &impl Trust, items: &[&[u8]]) -> Result<Window, Error> {
        let first = items.first().ok_or_else(|| Error::Refused("nothing to take".to_owned()))?;
        let processed = self.mls.process_message(provider, protocol(first)?).map_err(mls)?;
        let sender = processed.sender().clone();
        let credential = processed.credential().clone();
        let ProcessedMessageContent::StagedCommitMessage(staged) = processed.into_content() else {
            return Err(Error::Refused("a window opens only at a commit".to_owned()));
        };
        let (departed, before) = {
            let leaf = |index: LeafNodeIndex| self.leaf(index);
            let refusal = match validation::commit(&self.kind, trust, &sender, &credential, &staged, &leaf) {
                Err(refusal @ Error::Unlisted { .. }) => refusal,
                Err(refusal) => return Err(refusal),
                Ok(()) => return Err(Error::Refused("a commit taken as it is opens no window".to_owned())),
            };
            if !matches!(self.kind, GroupKind::Conversation { .. }) {
                return Err(refusal);
            }
            // The one leaf it's refused for: the leaf joining by itself, or
            // one it adds.
            let mut coming: Vec<&openmls::prelude::LeafNode> = staged
                .queued_proposals()
                .filter_map(|queued| match queued.proposal() {
                    openmls::prelude::Proposal::Add(add) => Some(add.key_package().leaf_node()),
                    _ => None,
                })
                .collect();
            if matches!(sender, Sender::NewMemberCommit) {
                coming.extend(staged.update_path_leaf_node());
            }
            let unlisted: Vec<LeafCertificate> = coming
                .into_iter()
                .filter(|node| {
                    let checked = validation::member(&self.kind, trust, node.credential(), node.signature_key().as_slice());
                    matches!(checked, Err(Error::Unlisted { .. }))
                })
                .filter_map(|node| LeafCertificate::from_credential(node.credential()).ok())
                .collect();
            let [departed] = unlisted.as_slice() else { return Err(refusal) };
            validation::commit_tolerating(&self.kind, trust, Some(departed), &sender, &credential, &staged, &leaf).map_err(|_| refusal)?;
            // Who was there before it came in, by tree index: who may close
            // the window, unless taken out meanwhile.
            let before: std::collections::BTreeMap<u32, String> = self
                .mls
                .members()
                .filter_map(|member| {
                    LeafCertificate::from_credential(&member.credential)
                        .ok()
                        .map(|certificate| (member.index.u32(), certificate.leaf.leaf_id))
                })
                .collect();
            (departed.clone(), before)
        };
        let mut window = Opened { departed, before, out: std::collections::BTreeSet::new(), index: None };
        window.out.extend(removals(&staged));
        self.mls.merge_staged_commit(provider, *staged).map_err(mls)?;
        let key = window.departed.leaf.signature_key;
        window.index = self.mls.members().find(|member| member.signature_key == key).map(|member| member.index);
        if window.index.is_none() {
            return Ok(Window::Aborted { why: "the leaf it brought in isn't in the group".to_owned() });
        }
        let mut taken = vec![Windowed { incoming: Ok(Incoming::Committed { epoch: self.epoch() }), departed: false }];
        for item in &items[1..] {
            if taken.len() >= WINDOW_MOST {
                return Ok(Window::Aborted { why: format!("no removal within {WINDOW_MOST} items") });
            }
            match self.take_in_window(provider, trust, &mut window, item)? {
                Step::Took(windowed) => taken.push(windowed),
                Step::Closed(windowed) => {
                    taken.push(windowed);
                    return Ok(Window::Closed { taken });
                }
                Step::Abort(why) => return Ok(Window::Aborted { why }),
            }
        }
        Ok(Window::Open)
    }

    /// One item past a window's opening commit.
    fn take_in_window(
        &mut self,
        provider: &impl OpenMlsProvider,
        trust: &impl Trust,
        window: &mut Opened,
        bytes: &[u8],
    ) -> Result<Step, Error> {
        let tolerated = Some(&window.departed);
        let dropped = |error: Error| Ok(Step::Took(Windowed { incoming: Err(error), departed: false }));
        let message = match protocol(bytes) {
            Ok(message) => message,
            Err(error) => return dropped(error),
        };
        let handshake = message.content_type() != openmls::prelude::ContentType::Application;
        let processed = match self.mls.process_message(provider, message) {
            Ok(processed) => processed,
            Err(error) if handshake => return Ok(Step::Abort(format!("a change it couldn't take: {:?}", mls(error)))),
            Err(error) => return dropped(mls(error)),
        };
        let sender = processed.sender().clone();
        let credential = processed.credential().clone();
        let epoch = processed.epoch().as_u64();
        let from_departed = matches!(sender, Sender::Member(index) if Some(index) == window.index);
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => {
                let Sender::Member(index) = sender else { return dropped(Error::Refused("a message from outside the group".to_owned())) };
                let (credential, key) = match self.signer(index, &credential) {
                    Ok(signer) => signer,
                    Err(error) => return dropped(error),
                };
                let incoming = validation::application_tolerating(&self.kind, trust, tolerated, &credential, &key)
                    .map(|from| Incoming::Application { from: Box::new(from), content: message.into_bytes(), epoch });
                Ok(Step::Took(Windowed { incoming, departed: from_departed }))
            }
            ProcessedMessageContent::ProposalMessage(proposal) => {
                if from_departed {
                    return Ok(Step::Abort("the removed device proposed a change".to_owned()));
                }
                let Sender::Member(index) = sender else { return Ok(Step::Abort("a proposal from outside the group".to_owned())) };
                let Ok((credential, key)) = self.signer(index, &credential) else {
                    return Ok(Step::Abort("a proposal from no member".to_owned()));
                };
                if let Err(error) = validation::member_tolerating(&self.kind, trust, tolerated, &credential, &key) {
                    return Ok(Step::Abort(format!("a proposal it couldn't take: {error}")));
                }
                self.mls.store_pending_proposal(provider.storage(), *proposal).map_err(mls)?;
                Ok(Step::Took(Windowed { incoming: Ok(Incoming::Proposal), departed: false }))
            }
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                // Its own Update, with nothing else, is all the window's leaf
                // commits.
                if from_departed && (staged.queued_proposals().next().is_some() || staged.update_path_leaf_node().is_none()) {
                    return Ok(Step::Abort("the removed device committed a change".to_owned()));
                }
                let leaf = |index: LeafNodeIndex| self.leaf(index);
                if let Err(error) = validation::commit_tolerating(&self.kind, trust, tolerated, &sender, &credential, &staged, &leaf) {
                    return Ok(Step::Abort(format!("a change it couldn't take: {error}")));
                }
                let gone = removals(&staged);
                let closes = match window.index {
                    Some(index) if gone.contains(&index.u32()) => {
                        // Taken out: by its person's own device that was
                        // there throughout and is listed now, or it can't
                        // close.
                        let Sender::Member(by) = sender else {
                            return Ok(Step::Abort("the removed device went out by someone joining".to_owned()));
                        };
                        let throughout = window.before.get(&by.u32()).is_some_and(|id| {
                            !window.out.contains(&by.u32())
                                && LeafCertificate::from_credential(&credential).is_ok_and(|certificate| certificate.leaf.leaf_id == *id)
                        });
                        let listed = leaf(by).is_some_and(|(credential, key)| {
                            validation::member(&self.kind, trust, &credential, &key)
                                .is_ok_and(|member| member.account() == window.departed.leaf.account_id)
                        });
                        if !throughout || !listed {
                            return Ok(Step::Abort(
                                "the removed device went out by a device that wasn't there throughout, or isn't listed".to_owned(),
                            ));
                        }
                        true
                    }
                    _ => false,
                };
                window.out.extend(gone);
                self.mls.merge_staged_commit(provider, *staged).map_err(mls)?;
                let windowed = Windowed { incoming: Ok(Incoming::Committed { epoch: self.epoch() }), departed: false };
                Ok(if closes { Step::Closed(windowed) } else { Step::Took(windowed) })
            }
            _ => Ok(Step::Abort("a kind of message a window doesn't take".to_owned())),
        }
    }
}

/// A window being taken (`Group::process_window`).
struct Opened {
    /// The leaf it was opened for.
    departed: LeafCertificate,
    /// Its tree index once in.
    index: Option<LeafNodeIndex>,
    /// Who was in the group before it came in: tree index, leaf id.
    before: std::collections::BTreeMap<u32, String>,
    /// Tree indexes taken out since.
    out: std::collections::BTreeSet<u32>,
}

/// What one item did to a window.
enum Step {
    Took(Windowed),
    Closed(Windowed),
    Abort(String),
}

/// The tree indexes a commit takes out.
fn removals(staged: &openmls::prelude::StagedCommit) -> Vec<u32> {
    staged
        .queued_proposals()
        .filter_map(|queued| match queued.proposal() {
            openmls::prelude::Proposal::Remove(remove) => Some(remove.removed().u32()),
            _ => None,
        })
        .collect()
}

/// An MLS message as a handshake or application message.
fn protocol(bytes: &[u8]) -> Result<openmls::prelude::ProtocolMessage, Error> {
    let message = MlsMessageIn::tls_deserialize_exact(bytes).map_err(|e| Error::NotMls(e.to_string()))?;
    message.try_into_protocol_message().map_err(|_| Error::NotMls("not a handshake or application message".to_owned()))
}

#[cfg(test)]
mod tests {
    use openmls::prelude::{NewSignerBundle, PURE_CIPHERTEXT_WIRE_FORMAT_POLICY, WireFormat};
    use openmls_libcrux_crypto::Provider as Libcrux;

    use super::*;
    use crate::certificate::{Leaf, claktory_leaf_id};
    use crate::device_list::{DeviceList, Listed};
    use crate::identity::IdentityKey;
    use crate::validation::Pins;

    /// One endpoint: its storage, its leaf, its pins.
    struct End {
        provider: Libcrux,
        me: Me,
    }

    fn end(identity: &IdentityKey, kind: LeafKind, account: &str, leaf_id: &str) -> End {
        let key = LeafKey::generate().unwrap();
        let leaf = Leaf {
            kind,
            account_id: account.to_owned(),
            leaf_id: leaf_id.to_owned(),
            signature_key: key.public(),
            name: leaf_id.to_owned(),
            created_ms: 1,
        };
        let certificate = LeafCertificate::issue(identity, leaf).unwrap();
        End { provider: Libcrux::default(), me: Me::new(key, certificate).unwrap() }
    }

    fn listed(end: &End) -> Listed {
        let leaf = &end.me.certificate.leaf;
        Listed { kind: leaf.kind, leaf_id: leaf.leaf_id.clone(), signature_key: leaf.signature_key }
    }

    /// Matt (a phone and a Claktory) and Ana (an iPad), all pinned as at
    /// first contact, and the conversation the phone starts with both.
    struct World {
        phone: End,
        mini: End,
        ipad: End,
        pins: Pins,
        chat: GroupKind,
        groups: Vec<Group>,
        ana: IdentityKey,
        matt: IdentityKey,
    }

    fn world() -> World {
        let (matt, ana) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let phone = end(&matt, LeafKind::Device, "matt", "phone");
        let mini = end(&matt, LeafKind::Claktory, "matt", &claktory_leaf_id("mini", 1));
        let ipad = end(&ana, LeafKind::Device, "ana", "ipad");
        let mut pins = Pins::default();
        pins.first_contact("matt", matt.public()).unwrap();
        pins.first_contact("ana", ana.public()).unwrap();
        let list = |identity: &IdentityKey, account: &str, version: u64, leaves: Vec<Listed>| {
            DeviceList { account_id: account.to_owned(), version, created_ms: 1, leaves }.sign(identity).unwrap().to_bytes().unwrap()
        };
        pins.accept_list("matt", &list(&matt, "matt", 1, vec![listed(&phone), listed(&mini)])).unwrap();
        pins.accept_list("ana", &list(&ana, "ana", 1, vec![listed(&ipad)])).unwrap();
        let chat = GroupKind::Conversation { people: ["matt".to_owned(), "ana".to_owned()] };

        let mut started = Group::create(&phone.provider, &phone.me, chat.clone(), b"conversation-1").unwrap();
        let packages = vec![ipad.me.key_package(&ipad.provider).unwrap(), mini.me.key_package(&mini.provider).unwrap()];
        let out = started.add(&phone.provider, &phone.me, &pins, &packages).unwrap();
        assert!(out.group_info.is_some(), "a GroupInfo for the worker to keep");
        started.merge_pending(&phone.provider).unwrap();
        let welcome = out.welcome.unwrap();
        let at_ipad = Group::join(&ipad.provider, &pins, chat.clone(), b"conversation-1", &welcome).unwrap();
        let at_mini = Group::join(&mini.provider, &pins, chat.clone(), b"conversation-1", &welcome).unwrap();
        World { groups: vec![started, at_ipad, at_mini], phone, mini, ipad, pins, chat, ana, matt }
    }

    #[test]
    fn a_new_group_gives_its_group_info_and_a_leaf_its_last_resort_package() {
        let w = world();
        let matt = IdentityKey::generate().unwrap();
        let phone = end(&matt, LeafKind::Device, "matt", "phone-2");
        let group = Group::create(&phone.provider, &phone.me, w.chat.clone(), b"conversation-9").unwrap();
        let info = group.group_info(&phone.provider, &phone.me).unwrap();
        let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::tls_deserialize_exact(info).unwrap().extract() else {
            panic!("not a group info")
        };
        assert_eq!((info.group_context().epoch().as_u64(), info.group_context().group_id().as_slice()), (0, b"conversation-9".as_slice()));
        let package = phone.me.last_resort_key_package(&phone.provider).unwrap();
        let MlsMessageBodyIn::KeyPackage(package) = MlsMessageIn::tls_deserialize_exact(package).unwrap().extract() else { panic!() };
        let package = package.validate(phone.provider.crypto(), ProtocolVersion::Mls10).unwrap();
        assert!(package.last_resort());
    }

    #[test]
    fn people_talk_and_a_claktory_reads_but_never_speaks_in_a_conversation() {
        let mut w = world();
        let said = w.groups[0].send(&w.phone.provider, &w.phone.me, &w.pins, b"hi Ana").unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            let Incoming::Application { from, content, .. } = w.groups[n].process(&end.provider, &w.pins, &said).unwrap() else { panic!() };
            assert_eq!((from.account(), from.leaf_id(), content.as_slice()), ("matt", "phone", b"hi Ana".as_slice()));
        }
        // The Claktory won't send there, and if it did, nobody takes it (H1).
        assert!(w.groups[2].send(&w.mini.provider, &w.mini.me, &w.pins, b"psst").is_err());
        let forced = w.groups[2].mls.create_message(&w.mini.provider, &w.mini.me.key, b"psst").unwrap().tls_serialize_detached().unwrap();
        let dropped = w.groups[1].process(&w.ipad.provider, &w.pins, &forced);
        assert_eq!(dropped.err().map(|e| e.to_string()), Some("refused: a Claktory doesn't send messages in a conversation".to_owned()));

        // A Welcome to another group than the one expected is refused.
        let tablet = end(&w.ana, LeafKind::Device, "ana", "ipad-2");
        let mut pins = w.pins.clone();
        let list = DeviceList { account_id: "ana".to_owned(), version: 2, created_ms: 2, leaves: vec![listed(&w.ipad), listed(&tablet)] };
        pins.accept_list("ana", &list.sign(&w.ana).unwrap().to_bytes().unwrap()).unwrap();
        let mut other = Group::create(&w.phone.provider, &w.phone.me, w.chat.clone(), b"conversation-2").unwrap();
        let out = other.add(&w.phone.provider, &w.phone.me, &pins, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
        let joined = Group::join(&tablet.provider, &pins, w.chat.clone(), b"conversation-1", &out.welcome.unwrap());
        assert_eq!(joined.err().map(|e| e.to_string()), Some("refused: a welcome to another group than this one".to_owned()));

        // Its own Update is fine (new keys) and both take it.
        let update = w.groups[2].update(&w.mini.provider, &w.mini.me).unwrap();
        w.groups[2].merge_pending(&w.mini.provider).unwrap();
        for (n, end) in [(0, &w.phone), (1, &w.ipad)] {
            assert!(matches!(w.groups[n].process(&end.provider, &w.pins, &update.commit).unwrap(), Incoming::Committed { .. }));
        }

        // What people say is encrypted; commits go in the clear for the
        // delivery service to check, and an encrypted one is refused.
        let format = |bytes: &[u8]| MlsMessageIn::tls_deserialize_exact(bytes).unwrap().wire_format();
        assert_eq!((format(&said), format(&update.commit)), (WireFormat::PrivateMessage, WireFormat::PublicMessage));
        let hidden =
            MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).wire_format_policy(PURE_CIPHERTEXT_WIRE_FORMAT_POLICY).build();
        w.groups[1].mls.set_configuration(w.ipad.provider.storage(), &hidden).unwrap();
        let secret = w.groups[1].update(&w.ipad.provider, &w.ipad.me).unwrap();
        assert_eq!(format(&secret.commit), WireFormat::PrivateMessage);
        assert!(w.groups[0].process(&w.phone.provider, &w.pins, &secret.commit).is_err());
    }

    /// A message sent just before a commit is still read after it, up to
    /// five epochs on, as the worker takes it; past that it's gone.
    #[test]
    fn a_message_that_raced_a_commit_is_still_read() {
        let mut w = world();
        let raced = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"sent at epoch 1").unwrap();
        let older = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"also at epoch 1").unwrap();
        for _ in 0..PAST_EPOCHS {
            let update = w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
            w.groups[0].merge_pending(&w.phone.provider).unwrap();
            w.groups[2].process(&w.mini.provider, &w.pins, &update.commit).unwrap();
        }
        assert_eq!(w.groups[0].epoch(), 1 + PAST_EPOCHS as u64);
        for (n, end) in [(0, &w.phone), (2, &w.mini)] {
            let Incoming::Application { content, epoch, .. } = w.groups[n].process(&end.provider, &w.pins, &raced).unwrap() else {
                panic!()
            };
            assert_eq!((content.as_slice(), epoch), (b"sent at epoch 1".as_slice(), 1), "the epoch it was said in (F6)");
        }
        // One more epoch on, that epoch's secrets are gone.
        w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        assert!(w.groups[0].process(&w.phone.provider, &w.pins, &older).is_err());
    }

    /// Melone's F1 (Fugo 40159): a leaf taken out speaks at an epoch before
    /// it went, after another leaf took its place in the tree. Taken as the
    /// newcomer's before; refused now, since the leaf there isn't the one
    /// that signed.
    #[test]
    fn a_removed_claktory_cant_speak_as_the_friends_device_that_took_its_place() {
        let mut w = world();
        let ipad3 = end(&w.ana, LeafKind::Device, "ana", "ipad3");
        let list = DeviceList { account_id: "ana".to_owned(), version: 2, created_ms: 2, leaves: vec![listed(&w.ipad), listed(&ipad3)] };
        w.pins.accept_list("ana", &list.sign(&w.ana).unwrap().to_bytes().unwrap()).unwrap();
        // Matt takes his Claktory out; Ana's new iPad comes into its place.
        let out = w.groups[0].remove(&w.phone.provider, &w.phone.me, &[&claktory_leaf_id("mini", 1)]).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &w.pins, &out.commit).unwrap();
        let package = ipad3.me.key_package(&ipad3.provider).unwrap();
        let out = w.groups[1].add(&w.ipad.provider, &w.ipad.me, &w.pins, &[package]).unwrap();
        w.groups[1].merge_pending(&w.ipad.provider).unwrap();
        w.groups[0].process(&w.phone.provider, &w.pins, &out.commit).unwrap();
        assert_eq!(w.groups[0].leaf_ids(), vec!["phone", "ipad", "ipad3"], "the new iPad sits where the Claktory was");
        // What the removed Claktory still holds sends at epoch 1, as an
        // attacker with its keys could (its own send refuses a conversation,
        // so as its channel).
        let channel = GroupKind::Channel { account: "matt".to_owned(), claktory_id: "mini".to_owned() };
        let mut kept = Group::load(&w.mini.provider, channel, b"conversation-1").unwrap().unwrap();
        let forged = kept.send(&w.mini.provider, &w.mini.me, &Pins::default(), b"wire the money to IBAN X").unwrap();
        let heard = w.groups[0].process(&w.phone.provider, &w.pins, &forged);
        assert!(matches!(&heard, Err(Error::Refused(why)) if why.contains("left the group")), "{heard:?}");
    }

    /// Melone's F1, in the self group: a stolen phone, taken out and replaced,
    /// sends at an epoch before it went (pins, say). Never taken as the
    /// replacement's.
    #[test]
    fn a_stolen_phone_cant_speak_as_the_device_that_replaced_it() {
        let matt = IdentityKey::generate().unwrap();
        let phone = end(&matt, LeafKind::Device, "matt", "phone");
        let stolen = end(&matt, LeafKind::Device, "matt", "old-phone");
        let tablet = end(&matt, LeafKind::Device, "matt", "tablet");
        let list = |version: u64, leaves: Vec<Listed>| {
            DeviceList { account_id: "matt".to_owned(), version, created_ms: version, leaves }.sign(&matt).unwrap().to_bytes().unwrap()
        };
        let mut pins = Pins::default();
        pins.first_contact("matt", matt.public()).unwrap();
        pins.accept_list("matt", &list(1, vec![listed(&phone), listed(&stolen)])).unwrap();
        let kind = GroupKind::SelfGroup { account: "matt".to_owned() };
        let mut at_phone = Group::create(&phone.provider, &phone.me, kind.clone(), b"self").unwrap();
        let out = at_phone.add(&phone.provider, &phone.me, &pins, &[stolen.me.key_package(&stolen.provider).unwrap()]).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();
        let mut at_stolen = Group::join(&stolen.provider, &pins, kind.clone(), b"self", &out.welcome.unwrap()).unwrap();
        pins.accept_list("matt", &list(2, vec![listed(&phone), listed(&tablet)])).unwrap();
        at_phone.remove(&phone.provider, &phone.me, &["old-phone"]).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();
        at_phone.add(&phone.provider, &phone.me, &pins, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();
        assert_eq!(at_phone.leaf_ids(), vec!["phone", "tablet"]);
        let forged = at_stolen.send(&stolen.provider, &stolen.me, &Pins::default(), br#"{"v":1,"kind":"pins"}"#).unwrap();
        let heard = at_phone.process(&phone.provider, &pins, &forged);
        assert!(matches!(&heard, Err(Error::Refused(why)) if why.contains("left the group")), "{heard:?}");
    }

    /// Melone's F2: a rejoin never goes back below the epoch this device
    /// already reached, so a sequencer can't fork it past a Remove.
    #[test]
    fn a_rejoin_is_refused_a_group_info_below_its_floor() {
        let mut w = world();
        let before = w.groups[0].group_info(&w.phone.provider, &w.phone.me).unwrap();
        let out = w.groups[0].remove(&w.phone.provider, &w.phone.me, &[&claktory_leaf_id("mini", 1)]).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &w.pins, &out.commit).unwrap();
        let fresh = Libcrux::default();
        let refused = Group::rejoin(&fresh, &w.ipad.me, &w.pins, w.chat.clone(), b"conversation-1", &before, 2);
        assert!(matches!(&refused, Err(Error::Refused(why)) if why.contains("before epoch 2")), "{:?}", refused.err());
        let after = out.group_info.unwrap();
        assert!(Group::rejoin(&fresh, &w.ipad.me, &w.pins, w.chat.clone(), b"conversation-1", &after, 2).is_ok());
    }

    /// Melone's F5: a Claktory takes only itself out, in its channel as in
    /// a conversation, never one of the person's devices.
    #[test]
    fn a_claktory_never_takes_the_persons_device_out_of_its_channel() {
        let mut w = world();
        let tablet = end(&w.matt, LeafKind::Device, "matt", "tablet");
        let list = DeviceList {
            account_id: "matt".to_owned(),
            version: 2,
            created_ms: 2,
            leaves: vec![listed(&w.phone), listed(&w.mini), listed(&tablet)],
        };
        w.pins.accept_list("matt", &list.sign(&w.matt).unwrap().to_bytes().unwrap()).unwrap();
        let channel = GroupKind::Channel { account: "matt".to_owned(), claktory_id: "mini".to_owned() };
        let mut at_phone = Group::create(&w.phone.provider, &w.phone.me, channel.clone(), b"channel-1").unwrap();
        let packages = [w.mini.me.key_package(&w.mini.provider).unwrap(), tablet.me.key_package(&tablet.provider).unwrap()];
        let out = at_phone.add(&w.phone.provider, &w.phone.me, &w.pins, &packages).unwrap();
        at_phone.merge_pending(&w.phone.provider).unwrap();
        let mut at_mini = Group::join(&w.mini.provider, &w.pins, channel.clone(), b"channel-1", &out.welcome.unwrap()).unwrap();
        let out = at_mini.remove(&w.mini.provider, &w.mini.me, &["tablet"]).unwrap();
        let refused = at_phone.process(&w.phone.provider, &w.pins, &out.commit);
        assert!(matches!(&refused, Err(Error::Refused(why)) if why.contains("removes only itself")), "{refused:?}");
        assert_eq!(at_phone.leaf_ids().len(), 3, "nothing merged");
    }

    /// Melone's F4 and F7: key packages last 30 days, and nothing in
    /// another ciphersuite is taken.
    #[test]
    fn key_packages_last_thirty_days_and_another_suite_is_refused() {
        let mut w = world();
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        for bytes in [w.ipad.me.key_package(&w.ipad.provider).unwrap(), w.ipad.me.last_resort_key_package(&w.ipad.provider).unwrap()] {
            let MlsMessageBodyIn::KeyPackage(package) = read(&bytes).unwrap() else { panic!() };
            let package = package.validate(w.ipad.provider.crypto(), ProtocolVersion::Mls10).unwrap();
            let ends = package.life_time().not_after();
            assert!((now + KEY_PACKAGE_LIFETIME - 60..=now + KEY_PACKAGE_LIFETIME + 60).contains(&ends), "{ends} vs {now}");
        }
        let tablet = end(&w.matt, LeafKind::Device, "matt", "tablet");
        let other = openmls::prelude::Ciphersuite::MLS_128_DHKEMX25519_CHACHA20POLY1305_SHA256_Ed25519;
        let bundle = KeyPackage::builder().build(other, &tablet.provider, &tablet.me.key, tablet.me.credential().unwrap()).unwrap();
        let foreign = MlsMessageOut::from(bundle.key_package().clone()).tls_serialize_detached().unwrap();
        let refused = w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[foreign]);
        assert!(matches!(&refused, Err(Error::Refused(why)) if why.contains("another ciphersuite")), "{:?}", refused.err());
    }

    #[test]
    fn a_leaf_the_service_made_or_a_claktory_the_friend_adds_is_refused() {
        let mut w = world();
        // A key package from a key the service made for Ana: the phone won't add it.
        let fake = IdentityKey::generate().unwrap();
        let ghost = end(&fake, LeafKind::Device, "ana", "ipad-2");
        let package = ghost.me.key_package(&ghost.provider).unwrap();
        assert_eq!(
            w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, std::slice::from_ref(&package)).err().map(|e| e.to_string()),
            Some("refused: ana's certificate isn't from their security key".to_owned())
        );
        // Ana's own new device, not on her list yet: refused too.
        let tablet = end(&w.ana, LeafKind::Device, "ana", "ipad-2");
        let package = tablet.me.key_package(&tablet.provider).unwrap();
        assert!(w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[package]).is_err());

        // Ana's device adds another Claktory of Matt's (not hers to add):
        // forced past her own checks, every other member refuses the commit.
        let studio = end(&w.matt, LeafKind::Claktory, "matt", &claktory_leaf_id("studio", 1));
        let mut pins = w.pins.clone();
        let list = DeviceList {
            account_id: "matt".to_owned(),
            version: 2,
            created_ms: 2,
            leaves: vec![listed(&w.phone), listed(&w.mini), listed(&studio)],
        };
        pins.accept_list("matt", &list.sign(&w.matt).unwrap().to_bytes().unwrap()).unwrap();
        let package = studio.me.key_package(&studio.provider).unwrap();
        assert!(
            w.groups[1].add(&w.ipad.provider, &w.ipad.me, &pins, std::slice::from_ref(&package)).is_err(),
            "her own side refuses it first"
        );
        let MlsMessageBodyIn::KeyPackage(package) = read(&package).unwrap() else { panic!() };
        let package = KeyPackageIn::validate(package, w.ipad.provider.crypto(), ProtocolVersion::Mls10).unwrap();
        let (commit, welcome, _) = w.groups[1].mls.add_members(&w.ipad.provider, &w.ipad.me.key, &[package]).unwrap();
        let commit = commit.tls_serialize_detached().unwrap();
        // Nor does that Claktory join from Ana's Welcome.
        let joined = Group::join(&studio.provider, &pins, w.chat.clone(), b"conversation-1", &welcome.tls_serialize_detached().unwrap());
        assert_eq!(joined.err().map(|e| e.to_string()), Some("refused: only a device of the same person adds a Claktory".to_owned()));
        let refused = w.groups[0].process(&w.phone.provider, &pins, &commit);
        assert_eq!(refused.err().map(|e| e.to_string()), Some("refused: only a device of the same person adds a Claktory".to_owned()));
        assert_eq!(w.groups[0].epoch(), 1, "not merged");

        // Ana's device removing Matt's Claktory: Matt's phone refuses it.
        let indexes: Vec<_> = w.groups[1]
            .mls
            .members()
            .filter(|member| LeafCertificate::from_credential(&member.credential).is_ok_and(|c| c.leaf.kind == LeafKind::Claktory))
            .map(|member| member.index)
            .collect();
        w.groups[1].mls.clear_pending_commit(w.ipad.provider.storage()).unwrap();
        let (commit, _, _) = w.groups[1].mls.remove_members(&w.ipad.provider, &w.ipad.me.key, &indexes).unwrap();
        let refused = w.groups[0].process(&w.phone.provider, &w.pins, &commit.tls_serialize_detached().unwrap());
        assert_eq!(refused.err().map(|e| e.to_string()), Some("refused: a device removes only its own person's leaves".to_owned()));

        // A Claktory committing an Add in a conversation: refused.
        let w = &mut world();
        let tablet = end(&w.ana, LeafKind::Device, "ana", "ipad-2");
        let mut pins = w.pins.clone();
        let list = DeviceList { account_id: "ana".to_owned(), version: 2, created_ms: 2, leaves: vec![listed(&w.ipad), listed(&tablet)] };
        pins.accept_list("ana", &list.sign(&w.ana).unwrap().to_bytes().unwrap()).unwrap();
        let package = tablet.me.key_package(&tablet.provider).unwrap();
        let MlsMessageBodyIn::KeyPackage(package) = read(&package).unwrap() else { panic!() };
        let package = KeyPackageIn::validate(package, w.mini.provider.crypto(), ProtocolVersion::Mls10).unwrap();
        let (commit, welcome, _) = w.groups[2].mls.add_members(&w.mini.provider, &w.mini.me.key, &[package]).unwrap();
        // The new device never joins from that Welcome: a Claktory can't add it.
        let joined = Group::join(&tablet.provider, &pins, w.chat.clone(), b"conversation-1", &welcome.tls_serialize_detached().unwrap());
        assert_eq!(joined.err().map(|e| e.to_string()), Some("refused: only a member's device adds a leaf".to_owned()));
        let refused = w.groups[1].process(&w.ipad.provider, &pins, &commit.tls_serialize_detached().unwrap());
        assert_eq!(refused.err().map(|e| e.to_string()), Some("refused: only a member's device adds a leaf".to_owned()))
    }

    #[test]
    fn a_claktory_leaves_by_asking_and_a_removed_leaf_hears_nothing_more() {
        let mut w = world();
        // The Claktory asks to leave; Matt's phone commits it.
        let ask = w.groups[2].leave(&w.mini.provider, &w.mini.me).unwrap();
        for (n, end) in [(0, &w.phone), (1, &w.ipad)] {
            assert!(matches!(w.groups[n].process(&end.provider, &w.pins, &ask).unwrap(), Incoming::Proposal));
        }
        let (commit, _, _) = w.groups[0].mls.commit_to_pending_proposals(&w.phone.provider, &w.phone.me.key).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        let commit = commit.tls_serialize_detached().unwrap();
        assert!(matches!(w.groups[1].process(&w.ipad.provider, &w.pins, &commit).unwrap(), Incoming::Committed { epoch: 2 }));
        assert_eq!(w.groups[0].members(&w.pins).len(), 2);
        let later = w.groups[0].send(&w.phone.provider, &w.phone.me, &w.pins, b"after").unwrap();
        assert!(w.groups[2].process(&w.mini.provider, &w.pins, &later).is_err(), "removed: can't read what came after");
        let _ = &w.chat;
    }

    #[test]
    fn a_device_rejoins_by_itself_but_a_claktory_never_does() {
        let mut w = world();
        // The phone's latest GroupInfo, as the worker keeps it.
        let update = w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            w.groups[n].process(&end.provider, &w.pins, &update.commit).unwrap();
        }
        let info = update.group_info.unwrap();

        // Ana's iPad lost its state: it rejoins with the same leaf, which
        // replaces the old one.
        let fresh = End {
            provider: Libcrux::default(),
            me: Me::new(LeafKey::from_secret(&w.ipad.me.key.secret()), w.ipad.me.certificate.clone()).unwrap(),
        };
        // Not from another group's GroupInfo, the same people in it.
        let mut other = Group::create(&w.phone.provider, &w.phone.me, w.chat.clone(), b"conversation-2").unwrap();
        let added = other.add(&w.phone.provider, &w.phone.me, &w.pins, &[w.ipad.me.key_package(&w.ipad.provider).unwrap()]).unwrap();
        let swapped = added.group_info.unwrap();
        assert_eq!(
            Group::rejoin(&fresh.provider, &fresh.me, &w.pins, w.chat.clone(), b"conversation-1", &swapped, 0)
                .err()
                .map(|error| error.to_string()),
            Some("refused: a GroupInfo of another group than this one".to_owned())
        );
        let (mut back, out) = Group::rejoin(&fresh.provider, &fresh.me, &w.pins, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        back.merge_pending(&fresh.provider).unwrap();
        assert!(matches!(w.groups[0].process(&w.phone.provider, &w.pins, &out.commit).unwrap(), Incoming::Committed { .. }));
        assert_eq!(w.groups[0].members(&w.pins).len(), 3, "the old leaf went");
        let said = w.groups[0].send(&w.phone.provider, &w.phone.me, &w.pins, b"welcome back").unwrap();
        assert!(matches!(back.process(&fresh.provider, &w.pins, &said).unwrap(), Incoming::Application { .. }));

        // The Claktory can't rejoin a conversation by itself, and a commit it
        // forces is refused (H1).
        assert!(Group::rejoin(&w.mini.provider, &w.mini.me, &w.pins, w.chat.clone(), b"conversation-1", &info, 0).is_err());
        let latest = out.group_info.unwrap();
        let MlsMessageBodyIn::GroupInfo(vgi) = read(&latest).unwrap() else { panic!() };
        let other = Libcrux::default();
        let (_, bundle) = MlsGroup::external_commit_builder()
            .with_config(join_config())
            .build_group(&other, vgi, w.mini.me.credential().unwrap())
            .unwrap()
            .load_psks(other.storage())
            .unwrap()
            .build(other.rand(), other.crypto(), &w.mini.me.key, |_| true)
            .unwrap()
            .finalize(&other)
            .unwrap();
        let forced = bundle.into_contents().0.tls_serialize_detached().unwrap();
        let refused = w.groups[0].process(&w.phone.provider, &w.pins, &forced);
        assert_eq!(refused.err().map(|e| e.to_string()), Some("refused: a Claktory doesn't join a conversation by itself".to_owned()));
    }

    /// The setup race (Mista, Abbacchio's live run): Ana's iPad, added with
    /// its KeyPackage, rejoins by external commit from the GroupInfo that
    /// already holds that leaf before it took its Welcome. Its leaf is in
    /// the tree once (the rejoin takes the added one out), and the phone
    /// can still change who's in the group.
    #[test]
    fn a_device_that_rejoins_past_its_own_add_is_in_the_group_once() {
        let w = world();
        let mut started = Group::create(&w.phone.provider, &w.phone.me, w.chat.clone(), b"conversation-7").unwrap();
        let package = w.ipad.me.key_package(&w.ipad.provider).unwrap();
        let added = started.add(&w.phone.provider, &w.phone.me, &w.pins, &[package]).unwrap();
        started.merge_pending(&w.phone.provider).unwrap();
        // The iPad lists the group before it fetches the Welcome: it rejoins
        // from the GroupInfo at epoch 1, which holds its leaf already.
        let info = added.group_info.unwrap();
        let (mut back, out) = Group::rejoin(&w.ipad.provider, &w.ipad.me, &w.pins, w.chat.clone(), b"conversation-7", &info, 0).unwrap();
        back.merge_pending(&w.ipad.provider).unwrap();
        assert!(matches!(started.process(&w.phone.provider, &w.pins, &out.commit).unwrap(), Incoming::Committed { .. }));
        let key = w.ipad.me.key.public();
        let with_key = started.mls.members().filter(|member| member.signature_key == key.as_slice()).count();
        assert_eq!(with_key, 1, "the iPad's leaf once");
        // Its Welcome, taken late, doesn't make a second group.
        assert!(Group::join(&w.ipad.provider, &w.pins, w.chat.clone(), b"conversation-7", &added.welcome.unwrap()).is_err());
        // The phone adds Ana's tablet: that commit goes through, and both
        // take it.
        let tablet = end(&w.ana, LeafKind::Device, "ana", "ipad-2");
        let mut pins = w.pins.clone();
        let list = DeviceList { account_id: "ana".to_owned(), version: 2, created_ms: 2, leaves: vec![listed(&w.ipad), listed(&tablet)] };
        pins.accept_list("ana", &list.sign(&w.ana).unwrap().to_bytes().unwrap()).unwrap();
        // Even when its view still has the iPad to add (a package claimed
        // before the rejoin, or two for one leaf): that one is left out.
        let stale = w.ipad.me.key_package(&w.ipad.provider).unwrap();
        let fresh = tablet.me.key_package(&tablet.provider).unwrap();
        let more = started.add(&w.phone.provider, &w.phone.me, &pins, &[stale.clone(), fresh.clone(), fresh]).unwrap();
        started.merge_pending(&w.phone.provider).unwrap();
        assert!(matches!(back.process(&w.ipad.provider, &pins, &more.commit).unwrap(), Incoming::Committed { .. }));
        assert_eq!(started.members(&pins).len(), 3);
        let nothing = started.add(&w.phone.provider, &w.phone.me, &pins, &[stale]).err();
        assert!(matches!(nothing, Some(Error::NothingToAdd)), "{nothing:?}");
        assert_eq!(nothing.map(|error| error.to_string()).as_deref(), Some("refused: everyone it would add is in the group already"));
    }

    /// Ana made a new security key (dms-e2ee §2.1): a certificate for her
    /// iPad's leaf, the same key under the new identity, and a version-1
    /// list under it. Matt's pins once they've taken the change in.
    struct Rotation {
        ana2: IdentityKey,
        me: Me,
        pins: Pins,
    }

    fn rotate(w: &World) -> Rotation {
        let ana2 = IdentityKey::generate().unwrap();
        let certificate = LeafCertificate::issue(&ana2, w.ipad.me.certificate.leaf.clone()).unwrap();
        let me = w.ipad.me.recertified(certificate).unwrap();
        let mut pins = w.pins.clone();
        pins.key_changed("ana", ana2.public()).unwrap();
        let list = DeviceList { account_id: "ana".to_owned(), version: 1, created_ms: 3, leaves: vec![listed(&w.ipad)] };
        pins.accept_list("ana", &list.sign(&ana2).unwrap().to_bytes().unwrap()).unwrap();
        Rotation { ana2, me, pins }
    }

    /// A commit the iPad forges with OpenMLS directly, past its own checks:
    /// an Update carrying `credential` (signed with `new_key` if it moves to
    /// another one), with `adds` riding along. Left pending, then dropped.
    fn forge(w: &mut World, credential: CredentialWithKey, new_key: Option<&LeafKey>, adds: Vec<KeyPackage>) -> Vec<u8> {
        let parameters = LeafNodeParameters::builder().with_credential_with_key(credential.clone()).build();
        let builder = w.groups[1]
            .mls
            .commit_builder()
            .consume_proposal_store(false)
            .force_self_update(true)
            .propose_adds(adds)
            .leaf_node_parameters(parameters)
            .load_psks(w.ipad.provider.storage())
            .unwrap();
        let (rand, crypto) = (w.ipad.provider.rand(), w.ipad.provider.crypto());
        let built = match new_key {
            Some(new) => builder
                .build_with_new_signer(
                    rand,
                    crypto,
                    &w.ipad.me.key,
                    NewSignerBundle { signer: new, credential_with_key: credential },
                    |_| true,
                )
                .unwrap(),
            None => builder.build(rand, crypto, &w.ipad.me.key, |_| true).unwrap(),
        };
        let commit = built.stage_commit(&w.ipad.provider).unwrap().into_contents().0.tls_serialize_detached().unwrap();
        w.groups[1].clear_pending(&w.ipad.provider).unwrap();
        commit
    }

    fn refusal(result: Result<Incoming, Error>) -> String {
        result.err().map(|error| error.to_string()).unwrap_or_else(|| "taken".to_owned())
    }

    /// Hostile 27 (R1, with Fugo's three tightenings): once Ana's key is
    /// replaced, her iPad's leaf, still under the old one, may do one
    /// thing: move itself to the new key with the same signature key, and
    /// nothing else in that commit. Its messages, proposals and any other
    /// commit are refused, and so is a move to another key, another
    /// signature key, a leaf her new list doesn't name, or one with an Add.
    #[test]
    fn a_leaf_moves_to_its_persons_new_key_and_does_nothing_else_under_the_old_one() {
        let mut w = world();
        let r = rotate(&w);
        // The leaf key can't go under someone else's leaf, nor another key.
        let other = LeafCertificate::issue(&r.ana2, Leaf { leaf_id: "ipad-2".to_owned(), ..w.ipad.me.certificate.leaf.clone() }).unwrap();
        assert!(w.ipad.me.recertified(other).is_err());

        // Under the old key it's inert for Matt's phone, which saw the change:
        // what it says, and what it proposes.
        let said = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"from the old key").unwrap();
        assert!(refusal(w.groups[0].process(&w.phone.provider, &r.pins, &said)).contains("was replaced"));
        let leaving = w.groups[1].leave(&w.ipad.provider, &w.ipad.me).unwrap();
        assert!(refusal(w.groups[0].process(&w.phone.provider, &r.pins, &leaving)).contains("was replaced"));
        w.groups[1].mls.clear_pending_proposals(w.ipad.provider.storage()).unwrap();
        // An ordinary Update under the old key: refused.
        let plain = w.groups[1].update(&w.ipad.provider, &w.ipad.me).unwrap();
        w.groups[1].clear_pending(&w.ipad.provider).unwrap();
        assert!(refusal(w.groups[0].process(&w.phone.provider, &r.pins, &plain.commit)).contains("was replaced"));

        // A move with an Add riding along.
        let tablet = end(&r.ana2, LeafKind::Device, "ana", "ipad-2");
        let mut with_tablet = r.pins.clone();
        let list = DeviceList { account_id: "ana".to_owned(), version: 2, created_ms: 4, leaves: vec![listed(&w.ipad), listed(&tablet)] };
        with_tablet.accept_list("ana", &list.sign(&r.ana2).unwrap().to_bytes().unwrap()).unwrap();
        let MlsMessageBodyIn::KeyPackage(package) = read(&tablet.me.key_package(&tablet.provider).unwrap()).unwrap() else { panic!() };
        let package = KeyPackageIn::validate(package, w.ipad.provider.crypto(), ProtocolVersion::Mls10).unwrap();
        let riding = forge(&mut w, r.me.credential().unwrap(), None, vec![package]);
        assert_eq!(
            refusal(w.groups[0].process(&w.phone.provider, &with_tablet, &riding)),
            "refused: a leaf under a replaced key commits only its own move to the new one"
        );

        // A move to another signature key, though the new list names it.
        let new_key = LeafKey::generate().unwrap();
        let moved = Leaf { signature_key: new_key.public(), ..w.ipad.me.certificate.leaf.clone() };
        let moved = Me::new(LeafKey::from_secret(&new_key.secret()), LeafCertificate::issue(&r.ana2, moved).unwrap()).unwrap();
        let mut naming_it = w.pins.clone();
        naming_it.key_changed("ana", r.ana2.public()).unwrap();
        let list = DeviceList { account_id: "ana".to_owned(), version: 1, created_ms: 3, leaves: vec![listed(&moved_end(&moved))] };
        naming_it.accept_list("ana", &list.sign(&r.ana2).unwrap().to_bytes().unwrap()).unwrap();
        let rekeyed = forge(&mut w, moved.credential().unwrap(), Some(&new_key), Vec::new());
        assert_eq!(
            refusal(w.groups[0].process(&w.phone.provider, &naming_it, &rekeyed)),
            "refused: a leaf moving to the new key keeps its own signature key"
        );

        // A move that the new list doesn't name.
        let mut unlisted = w.pins.clone();
        unlisted.key_changed("ana", r.ana2.public()).unwrap();
        let list = DeviceList { account_id: "ana".to_owned(), version: 1, created_ms: 3, leaves: vec![listed(&tablet)] };
        unlisted.accept_list("ana", &list.sign(&r.ana2).unwrap().to_bytes().unwrap()).unwrap();
        let update = w.groups[1].update_certificate(&w.ipad.provider, &r.me).unwrap();
        w.groups[1].clear_pending(&w.ipad.provider).unwrap();
        assert_eq!(
            refusal(w.groups[0].process(&w.phone.provider, &unlisted, &update.commit)),
            "refused: ana's device list doesn't name this leaf"
        );

        // A move to a key that was replaced too: Ana went on to a third one.
        let ana3 = IdentityKey::generate().unwrap();
        let mut third = r.pins.clone();
        third.key_changed("ana", ana3.public()).unwrap();
        assert!(refusal(w.groups[0].process(&w.phone.provider, &third, &update.commit)).contains("was replaced"));

        // Before Matt's side knows the new key: it says whose it is, takes
        // nothing, and the same commit goes through once the change is in.
        let update = w.groups[1].update_certificate(&w.ipad.provider, &r.me).unwrap();
        let unknown = w.groups[0].process(&w.phone.provider, &w.pins, &update.commit);
        assert!(matches!(unknown, Err(Error::UnknownIdentity { ref account }) if account == "ana"), "{unknown:?}");
        assert_eq!(w.groups[0].epoch(), 1);
        for (n, end) in [(0, &w.phone), (2, &w.mini)] {
            assert!(matches!(w.groups[n].process(&end.provider, &r.pins, &update.commit).unwrap(), Incoming::Committed { epoch: 2 }));
        }
        // The iPad never heard the worker's answer: its own commit, back
        // from the worker, is the one it kept pending, so it merges that.
        assert!(!w.groups[1].holds(&r.me).unwrap() && w.groups[1].holds(&w.ipad.me).unwrap());
        let own = w.groups[1].process(&w.ipad.provider, &r.pins, &update.commit);
        assert!(matches!(own, Ok(Incoming::Committed { epoch: 2 })), "{own:?}");
        assert!(w.groups[1].holds(&r.me).unwrap());

        // Under a key the pins don't hold (here the new one), the iPad's
        // leaf fails member() but is the same leaf by its id.
        let ids = vec!["phone".to_owned(), "ipad".to_owned(), claktory_leaf_id("mini", 1)];
        assert_eq!(w.groups[0].leaf_ids(), ids);
        assert_eq!(
            w.groups[0].members(&w.pins).iter().filter(|member| member.is_ok()).count(),
            2,
            "the iPad's isn't trusted under these pins"
        );
        // Unchecked, its whole leaf is there too, as its certificate claims.
        let leaves = w.groups[0].leaves_unchecked();
        assert_eq!(leaves.iter().map(|leaf| leaf.leaf_id.clone()).collect::<Vec<_>>(), ids);
        assert_eq!(leaves[1], w.ipad.me.certificate.leaf);
        assert_eq!(leaves.iter().map(|leaf| leaf.account_id.as_str()).collect::<Vec<_>>(), ["matt", "ana", "matt"]);
        assert_eq!(leaves.iter().map(|leaf| leaf.kind).collect::<Vec<_>>(), [LeafKind::Device, LeafKind::Device, LeafKind::Claktory]);

        // Moved: she's heard again, under the new key.
        let said = w.groups[1].send(&w.ipad.provider, &r.me, &r.pins, b"from the new key").unwrap();
        let Incoming::Application { from, content, .. } = w.groups[0].process(&w.phone.provider, &r.pins, &said).unwrap() else { panic!() };
        assert_eq!((from.certificate.identity, content.as_slice()), (r.ana2.public(), b"from the new key".as_slice()));
    }

    fn moved_end(me: &Me) -> End {
        End { provider: Libcrux::default(), me: Me::new(LeafKey::from_secret(&me.key.secret()), me.certificate.clone()).unwrap() }
    }

    /// Row 611 (Fugo's a′): Matt's self group holds his phone and his
    /// tablet. He locks a lost device out from the phone, which makes a new
    /// security key and moves to it; the tablet, kept but offline, stays
    /// under the old one. The phone then adds a new phone ("new"), under the
    /// new key, from the KeyPackages it put up. Returns the phone's group,
    /// the tablet's (still at the epoch before), the new phone, its pins
    /// (the new key, the old one replaced, and the new list) and the
    /// phone's commits (the move, then the Add, with its Welcome).
    struct Kept {
        tablet: End,
        at_tablet: Group,
        new: End,
        pins: Pins,
        matt2: IdentityKey,
        moved: Outgoing,
        added: Outgoing,
    }

    fn kept(on_new_list: bool, stray: Option<&IdentityKey>) -> Kept {
        kept_in(GroupKind::SelfGroup { account: "matt".to_owned() }, on_new_list, stray)
    }

    /// As `kept`, in a group of `kind`: in a channel, the kept leaf is its
    /// Claktory ("mini"), not the tablet.
    fn kept_in(kind: GroupKind, on_new_list: bool, stray: Option<&IdentityKey>) -> Kept {
        let matt = IdentityKey::generate().unwrap();
        let list = |identity: &IdentityKey, version: u64, leaves: Vec<Listed>| {
            DeviceList { account_id: "matt".to_owned(), version, created_ms: 1, leaves }.sign(identity).unwrap().to_bytes().unwrap()
        };
        let phone = end(&matt, LeafKind::Device, "matt", "phone");
        let tablet = match kind {
            GroupKind::Channel { .. } => end(&matt, LeafKind::Claktory, "matt", &claktory_leaf_id("mini", 1)),
            _ => end(&matt, LeafKind::Device, "matt", "tablet"),
        };
        let mut old = Pins::default();
        old.first_contact("matt", matt.public()).unwrap();
        old.accept_list("matt", &list(&matt, 1, vec![listed(&phone), listed(&tablet)])).unwrap();
        let mut at_phone = Group::create(&phone.provider, &phone.me, kind.clone(), b"self-1").unwrap();
        let out = at_phone.add(&phone.provider, &phone.me, &old, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();
        let at_tablet = Group::join(&tablet.provider, &old, kind.clone(), b"self-1", &out.welcome.unwrap()).unwrap();

        // A new key on the phone; the tablet kept on the new list, or not.
        let matt2 = IdentityKey::generate().unwrap();
        let phone2 = phone.me.recertified(LeafCertificate::issue(&matt2, phone.me.certificate.leaf.clone()).unwrap()).unwrap();
        let new = end(&matt2, LeafKind::Device, "matt", "new-phone");
        let mut pins = old.clone();
        pins.key_changed("matt", matt2.public()).unwrap();
        let mut leaves = vec![listed(&phone), listed(&new)];
        if on_new_list {
            leaves.push(listed(&tablet));
        }
        pins.accept_list("matt", &list(&matt2, 1, leaves)).unwrap();
        let moved = at_phone.update_certificate(&phone.provider, &phone2).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();

        // A leaf under a key nobody pinned, which the phone took in as
        // `stray` vouched for it.
        if let Some(stray) = stray {
            let intruder = end(stray, LeafKind::Device, "matt", "intruder");
            let mut vouched = Pins::default();
            vouched.first_contact("matt", stray.public()).unwrap();
            vouched.accept_list("matt", &list(stray, 1, vec![listed(&intruder)])).unwrap();
            let out = at_phone.add(&phone.provider, &phone2, &vouched, &[intruder.me.key_package(&intruder.provider).unwrap()]).unwrap();
            at_phone.merge_pending(&phone.provider).unwrap();
            drop(out);
        }
        let added = at_phone.add(&phone.provider, &phone2, &pins, &[new.me.key_package(&new.provider).unwrap()]).unwrap();
        at_phone.merge_pending(&phone.provider).unwrap();
        Kept { tablet, at_tablet, new, pins, matt2, moved, added }
    }

    /// A device kept at a new key, offline: the new phone joins at once,
    /// past its leaf under the old key, which stays inert (what it says,
    /// proposes or commits but its own move is refused); it moves later and
    /// the new phone takes that in.
    #[test]
    fn a_new_device_joins_past_a_kept_device_still_under_the_old_key() {
        let Kept { tablet, mut at_tablet, new, pins, matt2, moved, added } = kept(true, None);
        let kind = GroupKind::SelfGroup { account: "matt".to_owned() };
        let mut at_new = Group::join(&new.provider, &pins, kind, b"self-1", &added.welcome.unwrap()).unwrap();
        assert_eq!(at_new.present(&pins).iter().filter(|member| member.is_ok()).count(), 3);
        assert_eq!(at_new.members(&pins).iter().filter(|member| member.is_err()).count(), 1, "the tablet isn't a full member yet");

        // The tablet comes online: it takes the phone's move and the Add in.
        for commit in [&moved.commit, &added.commit] {
            assert!(matches!(at_tablet.process(&tablet.provider, &pins, commit).unwrap(), Incoming::Committed { .. }));
        }
        // Under the old key it's inert for the new phone.
        let said = at_tablet.send(&tablet.provider, &tablet.me, &pins, b"from the old key").unwrap();
        assert!(refusal(at_new.process(&new.provider, &pins, &said)).contains("was replaced"));
        let leaving = at_tablet.leave(&tablet.provider, &tablet.me).unwrap();
        assert!(refusal(at_new.process(&new.provider, &pins, &leaving)).contains("was replaced"));
        at_tablet.mls.clear_pending_proposals(tablet.provider.storage()).unwrap();
        let plain = at_tablet.update(&tablet.provider, &tablet.me).unwrap();
        at_tablet.clear_pending(&tablet.provider).unwrap();
        assert!(refusal(at_new.process(&new.provider, &pins, &plain.commit)).contains("was replaced"));

        // Its own move goes through, and then it's a member like any other.
        let tablet2 = tablet.me.recertified(LeafCertificate::issue(&matt2, tablet.me.certificate.leaf.clone()).unwrap()).unwrap();
        let move_tablet = at_tablet.update_certificate(&tablet.provider, &tablet2).unwrap();
        at_tablet.merge_pending(&tablet.provider).unwrap();
        assert!(matches!(at_new.process(&new.provider, &pins, &move_tablet.commit).unwrap(), Incoming::Committed { .. }));
        assert!(at_new.members(&pins).iter().all(Result::is_ok));
    }

    /// A leaf under the old key that the new list doesn't name (one a thief
    /// could have made with the stolen key) still stops the join: it has to
    /// be swept first.
    #[test]
    fn a_stray_leaf_under_the_old_key_still_stops_a_join() {
        let Kept { new, pins, added, .. } = kept(false, None);
        let kind = GroupKind::SelfGroup { account: "matt".to_owned() };
        let refused = Group::join(&new.provider, &pins, kind, b"self-1", &added.welcome.unwrap()).err().map(|error| error.to_string());
        assert_eq!(refused.as_deref(), Some("refused: matt's certificate is from a security key that was replaced"));
    }

    /// A leaf under a key never pinned for its account isn't a kept device,
    /// whatever its list says: the join is refused.
    #[test]
    fn a_leaf_under_a_key_never_pinned_stops_a_join() {
        let stray = IdentityKey::generate().unwrap();
        let Kept { new, pins, added, .. } = kept(true, Some(&stray));
        let kind = GroupKind::SelfGroup { account: "matt".to_owned() };
        let refused = Group::join(&new.provider, &pins, kind, b"self-1", &added.welcome.unwrap());
        assert!(matches!(refused, Err(Error::UnknownIdentity { ref account }) if account == "matt"), "{:?}", refused.err());
    }

    /// The same for a Claktory the person kept (Fugo): in its channel, still
    /// under the old key until a device enrolls it again, it doesn't stop a
    /// new device joining while the new list names it; one the new list
    /// doesn't name still does.
    #[test]
    fn a_new_device_joins_a_channel_past_its_kept_claktory_still_under_the_old_key() {
        let channel = GroupKind::Channel { account: "matt".to_owned(), claktory_id: "mini".to_owned() };
        let Kept { new, pins, added, .. } = kept_in(channel.clone(), true, None);
        let at_new = Group::join(&new.provider, &pins, channel.clone(), b"self-1", &added.welcome.unwrap()).unwrap();
        assert!(at_new.present(&pins).iter().all(Result::is_ok));
        assert_eq!(at_new.members(&pins).iter().filter(|member| member.is_err()).count(), 1, "the Claktory isn't a full member yet");

        let Kept { new, pins, added, .. } = kept_in(channel.clone(), false, None);
        let refused = Group::join(&new.provider, &pins, channel, b"self-1", &added.welcome.unwrap()).err().map(|error| error.to_string());
        assert_eq!(refused.as_deref(), Some("refused: matt's certificate is from a security key that was replaced"));
    }

    /// Row 630: Matt lost his only device's keys and started over on it, as
    /// the same leaf id with a new leaf key, under a new identity he alone
    /// lists (and, `kept`, a tablet he kept, still under the old key). His
    /// device never pinned the old key; Ana took the change in.
    struct StartOver {
        w: World,
        fresh: End,
        /// The fresh device's pins: its own new key and list, Ana's.
        mine: Pins,
        /// Ana's, after the change.
        hers: Pins,
        info: Vec<u8>,
    }

    fn start_over(kept: bool) -> StartOver {
        let mut w = world();
        let list = |identity: &IdentityKey, account: &str, version: u64, leaves: Vec<Listed>| {
            DeviceList { account_id: account.to_owned(), version, created_ms: 1, leaves }.sign(identity).unwrap().to_bytes().unwrap()
        };
        let tablet = kept.then(|| {
            let tablet = end(&w.matt, LeafKind::Device, "matt", "tablet");
            w.pins.accept_list("matt", &list(&w.matt, "matt", 2, vec![listed(&w.phone), listed(&w.mini), listed(&tablet)])).unwrap();
            let out =
                w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
            w.groups[0].merge_pending(&w.phone.provider).unwrap();
            for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
                w.groups[n].process(&end.provider, &w.pins, &out.commit).unwrap();
            }
            w.groups.push(Group::join(&tablet.provider, &w.pins, w.chat.clone(), b"conversation-1", &out.welcome.unwrap()).unwrap());
            tablet
        });
        // The GroupInfo the worker keeps.
        let update = w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        for n in 1..w.groups.len() {
            let end = [&w.phone, &w.ipad, &w.mini, tablet.as_ref().unwrap_or(&w.ipad)][n];
            w.groups[n].process(&end.provider, &w.pins, &update.commit).unwrap();
        }
        let info = update.group_info.unwrap();

        let matt2 = IdentityKey::generate().unwrap();
        let fresh = end(&matt2, LeafKind::Device, "matt", "phone");
        let mut leaves = vec![listed(&fresh)];
        leaves.extend(tablet.iter().map(listed));
        let new_list = list(&matt2, "matt", 1, leaves);
        let mut mine = Pins::default();
        mine.first_contact("matt", matt2.public()).unwrap();
        mine.accept_list("matt", &new_list).unwrap();
        mine.first_contact("ana", w.ana.public()).unwrap();
        mine.accept_list("ana", &list(&w.ana, "ana", 1, vec![listed(&w.ipad)])).unwrap();
        let mut hers = w.pins.clone();
        hers.key_changed("matt", matt2.public()).unwrap();
        hers.accept_list("matt", &new_list).unwrap();
        StartOver { w, fresh, mine, hers, info }
    }

    /// What a member's own client does with MLS alone, past every check
    /// (a thief's, say): takes a commit in, or reads a message.
    fn unchecked_take(group: &mut Group, provider: &Libcrux, bytes: &[u8]) -> Result<Option<Vec<u8>>, String> {
        let message = MlsMessageIn::tls_deserialize_exact(bytes).unwrap().try_into_protocol_message().unwrap();
        let processed = group.mls.process_message(provider, message).map_err(|error| format!("{error:?}"))?;
        match processed.into_content() {
            ProcessedMessageContent::StagedCommitMessage(staged) => {
                group.mls.merge_staged_commit(provider, *staged).map_err(|error| format!("{error:?}"))?;
                Ok(None)
            }
            ProcessedMessageContent::ApplicationMessage(message) => Ok(Some(message.into_bytes())),
            _ => Ok(None),
        }
    }

    /// Row 630 (Fugo's mechanism): the fresh device rejoins the same group
    /// past its own dead leaves (the old phone's, the old Claktory's) and
    /// takes them out in its next commit; between the two, nobody sends.
    /// Ana takes both commits and the two go on chatting in the same group,
    /// while the old phone (a thief's, checks or not) reads nothing after.
    #[test]
    fn a_device_that_started_over_rejoins_takes_its_dead_leaves_out_and_the_chat_goes_on() {
        let StartOver { mut w, fresh, mine, hers, info, .. } = start_over(false);
        let (mut at_fresh, out) = Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        let dead: Vec<(String, LeafKind)> =
            at_fresh.dead(&mine, &fresh.me).into_iter().map(|dead| (dead.leaf.leaf_id, dead.leaf.kind)).collect();
        assert_eq!(dead, [("phone".to_owned(), LeafKind::Device), (claktory_leaf_id("mini", 1), LeafKind::Claktory)]);

        // Ana takes the rejoin; the old phone, past its checks, too.
        assert!(matches!(w.groups[1].process(&w.ipad.provider, &hers, &out.commit).unwrap(), Incoming::Committed { .. }));
        assert_eq!(unchecked_take(&mut w.groups[0], &w.phone.provider, &out.commit), Ok(None));
        // Until the dead leaves are out, neither side sends.
        let held = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &hers, b"too soon");
        assert!(matches!(held, Err(Error::Waiting { ref account, reason: validation::Held::Replaced }) if account == "matt"), "{held:?}");
        assert_eq!(w.groups[1].waiting(&hers, &w.ipad.me).as_deref(), Some("matt"));
        assert!(matches!(at_fresh.send(&fresh.provider, &fresh.me, &mine, b"too soon"), Err(Error::Waiting { .. })));

        // The removal: one ordinary commit, which Ana takes.
        let removal = at_fresh.remove_dead(&fresh.provider, &fresh.me, &mine).unwrap().expect("dead leaves to take out");
        at_fresh.merge_pending(&fresh.provider).unwrap();
        assert!(matches!(w.groups[1].process(&w.ipad.provider, &hers, &removal.commit).unwrap(), Incoming::Committed { .. }));
        assert_eq!(unchecked_take(&mut w.groups[0], &w.phone.provider, &removal.commit), Ok(None));
        assert!(at_fresh.dead(&mine, &fresh.me).is_empty());
        assert!(at_fresh.remove_dead(&fresh.provider, &fresh.me, &mine).unwrap().is_none(), "nothing more to take out");
        let left: Vec<_> = w.groups[1].members(&hers).into_iter().map(|member| member.unwrap().leaf_id().to_owned()).collect();
        assert_eq!(left, ["ipad", "phone"], "Ana, and the new phone, under the same leaf id, and Ana");
        assert_eq!((at_fresh.group_id(), at_fresh.epoch()), (w.groups[1].group_id(), w.groups[1].epoch()), "the same group");

        // The first message after: Ana's new phone reads it, the old phone can't.
        let said = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &hers, b"welcome back").unwrap();
        let Incoming::Application { content, .. } = at_fresh.process(&fresh.provider, &mine, &said).unwrap() else { panic!() };
        assert_eq!(content, b"welcome back");
        assert!(unchecked_take(&mut w.groups[0], &w.phone.provider, &said).is_err(), "the old phone was taken out");
        let back = at_fresh.send(&fresh.provider, &fresh.me, &mine, b"thanks").unwrap();
        let Incoming::Application { from, content, .. } = w.groups[1].process(&w.ipad.provider, &hers, &back).unwrap() else { panic!() };
        assert_eq!((from.certificate.identity, content.as_slice()), (fresh.me.certificate.identity, b"thanks".as_slice()));
    }

    /// The old phone wasn't taken out (no removal commit yet, or the device
    /// never got it): Ana sends nothing, so it reads nothing new; once the
    /// removal is in, it reads nothing either.
    #[test]
    fn an_old_key_device_reads_nothing_sent_after_a_start_over() {
        let StartOver { mut w, fresh, mine, hers, info, .. } = start_over(false);
        let (mut at_fresh, out) = Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &hers, &out.commit).unwrap();
        unchecked_take(&mut w.groups[0], &w.phone.provider, &out.commit).unwrap();
        // Ana's client won't seal anything for the group while it's in.
        assert!(matches!(w.groups[1].send(&w.ipad.provider, &w.ipad.me, &hers, b"x"), Err(Error::Waiting { .. })));
        // The removal; the old phone never hears it.
        let removal = at_fresh.remove_dead(&fresh.provider, &fresh.me, &mine).unwrap().unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &hers, &removal.commit).unwrap();
        let said = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &hers, b"after").unwrap();
        assert!(unchecked_take(&mut w.groups[0], &w.phone.provider, &said).is_err(), "an epoch it never reached");
    }

    /// Only the person's own dead leaves: a leaf of Ana's under a key the
    /// fresh device never pinned stops the rejoin (it isn't dead to it, and
    /// isn't its to remove), and a removal of Ana's leaf forced past the
    /// checks is refused by Ana.
    #[test]
    fn a_start_over_never_removes_another_persons_leaf() {
        let StartOver { w, fresh, mine, .. } = start_over(false);
        // A stray of Ana's account's, added by Matt's old phone.
        let stray_key = IdentityKey::generate().unwrap();
        let stray = end(&stray_key, LeafKind::Device, "ana", "ipad-x");
        let mut vouched = w.pins.clone();
        vouched.key_changed("ana", stray_key.public()).unwrap();
        let list = DeviceList { account_id: "ana".to_owned(), version: 1, created_ms: 1, leaves: vec![listed(&stray)] };
        vouched.accept_list("ana", &list.sign(&stray_key).unwrap().to_bytes().unwrap()).unwrap();
        let mut forked = Group::load(&w.phone.provider, w.chat.clone(), b"conversation-1").unwrap().unwrap();
        let added = forked.add(&w.phone.provider, &w.phone.me, &vouched, &[stray.me.key_package(&stray.provider).unwrap()]).unwrap();
        forked.merge_pending(&w.phone.provider).unwrap();
        let refused =
            Group::rejoin(&Libcrux::default(), &fresh.me, &mine, w.chat.clone(), b"conversation-1", &added.group_info.unwrap(), 0);
        assert!(matches!(refused, Err(Error::UnknownIdentity { ref account }) if account == "ana"), "{:?}", refused.err());

        // From the real GroupInfo: Ana's iPad is no dead leaf of Matt's to
        // take out, and a removal of it forced past the checks is refused.
        let StartOver { mut w, fresh, mine, hers, info, .. } = start_over(false);
        let (mut at_fresh, out) = Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &hers, &out.commit).unwrap();
        let ana: Vec<_> = at_fresh
            .mls
            .members()
            .filter(|member| LeafCertificate::from_credential(&member.credential).is_ok_and(|c| c.leaf.account_id == "ana"))
            .map(|member| member.index)
            .collect();
        assert!(at_fresh.dead(&mine, &fresh.me).iter().all(|dead| dead.leaf.account_id == "matt"));
        let (forced, _, _) = at_fresh.mls.remove_members(&fresh.provider, &fresh.me.key, &ana).unwrap();
        let refused = w.groups[1].process(&w.ipad.provider, &hers, &forced.tls_serialize_detached().unwrap());
        assert_eq!(refused.err().map(|e| e.to_string()), Some("refused: a device removes only its own person's leaves".to_owned()));
    }

    /// A device the person kept (on the new list, still under the old key)
    /// isn't dead: the fresh device rejoins past it, takes out only the
    /// rest, and Ana's sends aren't held for it.
    #[test]
    fn a_start_over_keeps_a_device_its_new_list_names() {
        let StartOver { mut w, fresh, mine, hers, info, .. } = start_over(true);
        let (mut at_fresh, out) = Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        let dead: Vec<String> = at_fresh.dead(&mine, &fresh.me).into_iter().map(|dead| dead.leaf.leaf_id).collect();
        assert_eq!(dead, ["phone".to_owned(), claktory_leaf_id("mini", 1)], "not the tablet");
        w.groups[1].process(&w.ipad.provider, &hers, &out.commit).unwrap();
        let removal = at_fresh.remove_dead(&fresh.provider, &fresh.me, &mine).unwrap().unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &hers, &removal.commit).unwrap();
        let ids = at_fresh.leaf_ids();
        assert!(ids.contains(&"tablet".to_owned()) && ids.len() == 3, "{ids:?}");
        assert_eq!(w.groups[1].waiting(&hers, &w.ipad.me), None);
        let said = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &hers, b"hi both").unwrap();
        let Incoming::Application { content, .. } = at_fresh.process(&fresh.provider, &mine, &said).unwrap() else { panic!() };
        assert_eq!(content, b"hi both");
    }

    /// Row 654: Matt starts over twice while Ana's iPad is away. Back, it
    /// holds the first rejoin, under a key it never saw (`UnknownIdentity`)
    /// even once it has Matt's newest; the service lists that key as
    /// replaced, the held commit proves it, and it's retired, so the commit
    /// is refused for good (`Replaced`) and the iPad rejoins the same group,
    /// where the two chat on.
    #[test]
    fn a_friend_behind_two_start_overs_retires_the_key_between_and_rejoins() {
        let StartOver { mut w, fresh, mine, info, .. } = start_over(false);
        // The first start-over: its rejoin and removal, which Ana's iPad misses.
        let (mut at_first, rejoined) =
            Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_first.merge_pending(&fresh.provider).unwrap();
        let removal = at_first.remove_dead(&fresh.provider, &fresh.me, &mine).unwrap().unwrap();
        at_first.merge_pending(&fresh.provider).unwrap();

        // The second, a minute later, from the same phone.
        let matt3 = IdentityKey::generate().unwrap();
        let second = end(&matt3, LeafKind::Device, "matt", "phone");
        // Signed after the first start-over's certificate (made at 1).
        let list3 = DeviceList { account_id: "matt".to_owned(), version: 1, created_ms: 2, leaves: vec![listed(&second)] };
        let list3 = list3.sign(&matt3).unwrap().to_bytes().unwrap();
        let mut theirs = mine.clone();
        theirs.key_changed("matt", matt3.public()).unwrap();
        theirs.seen("matt");
        theirs.accept_list("matt", &list3).unwrap();
        let (mut at_second, _) =
            Group::rejoin(&second.provider, &second.me, &theirs, w.chat.clone(), b"conversation-1", &removal.group_info.unwrap(), 0)
                .unwrap();
        at_second.merge_pending(&second.provider).unwrap();
        let removed_again = at_second.remove_dead(&second.provider, &second.me, &theirs).unwrap().unwrap();
        at_second.merge_pending(&second.provider).unwrap();

        // Back, Ana's iPad has Matt's newest key and list, as fetched.
        let mut hers = w.pins.clone();
        hers.key_changed("matt", matt3.public()).unwrap();
        hers.accept_list("matt", &list3).unwrap();
        let held = w.groups[1].process(&w.ipad.provider, &hers, &rejoined.commit);
        assert!(matches!(held, Err(Error::UnknownIdentity { ref account }) if account == "matt"), "{held:?}");
        let signer = w.groups[1].held_signer(&w.ipad.provider, &rejoined.commit).unwrap().expect("a certificate");
        assert_eq!(signer.identity, fresh.me.certificate.identity);
        let listed_keys = [w.matt.public().0, fresh.me.certificate.identity.0];
        assert!(hers.replaced("matt", matt3.public().0, &listed_keys, &signer).unwrap());
        assert!(!hers.replaced("matt", matt3.public().0, &listed_keys, &signer).unwrap(), "once is enough");
        assert_eq!(hers.get("matt").unwrap().identity, matt3.public(), "the current key as it was");
        let refused = w.groups[1].process(&w.ipad.provider, &hers, &rejoined.commit);
        assert!(matches!(refused, Err(Error::Replaced { ref account }) if account == "matt"), "{refused:?}");

        // It rejoins at the current epoch, and they chat on.
        let back = moved_end(&w.ipad.me);
        let (mut at_back, out) =
            Group::rejoin(&back.provider, &back.me, &hers, w.chat.clone(), b"conversation-1", &removed_again.group_info.unwrap(), 0)
                .unwrap();
        at_back.merge_pending(&back.provider).unwrap();
        assert!(matches!(at_second.process(&second.provider, &theirs, &out.commit).unwrap(), Incoming::Committed { .. }));
        let said = at_second.send(&second.provider, &second.me, &theirs, b"third time lucky").unwrap();
        let Incoming::Application { content, .. } = at_back.process(&back.provider, &hers, &said).unwrap() else { panic!() };
        assert_eq!(content, b"third time lucky");
    }

    /// Matt's keys for the conditions on what the service lists as
    /// replaced: one he had (`k1`), the next (`k2`), and a certificate
    /// under `k2` made at 5.
    struct Keys {
        k1: IdentityKey,
        k2: IdentityKey,
        under_k2: LeafCertificate,
    }

    fn keys() -> Keys {
        let (k1, k2) = (IdentityKey::generate().unwrap(), IdentityKey::generate().unwrap());
        let leaf = Leaf {
            kind: LeafKind::Device,
            account_id: "matt".to_owned(),
            leaf_id: "phone".to_owned(),
            signature_key: LeafKey::generate().unwrap().public(),
            name: "phone".to_owned(),
            created_ms: 5,
        };
        let under_k2 = LeafCertificate::issue(&k2, leaf).unwrap();
        Keys { k1, k2, under_k2 }
    }

    /// Ana's pins for Matt: `pinned` now, after `before`, with a list it
    /// signed at `listed_at`.
    fn pinned(pinned: &IdentityKey, before: &[&IdentityKey], listed_at: u64) -> Pins {
        let mut pins = Pins::default();
        let mut keys = before.iter().chain([&pinned]);
        pins.first_contact("matt", keys.next().unwrap().public()).unwrap();
        for key in keys {
            pins.key_changed("matt", key.public()).unwrap();
        }
        let list = DeviceList { account_id: "matt".to_owned(), version: 1, created_ms: listed_at, leaves: vec![] };
        pins.accept_list("matt", &list.sign(pinned).unwrap().to_bytes().unwrap()).unwrap();
        pins
    }

    /// Fugo's conditions on what the service lists as replaced: only a key
    /// a held commit proves is retired (not one listed without one, nor
    /// another account's), and only one older than the pinned key's list; a
    /// list naming the current key, pinned or served, is refused whole; and
    /// a listed key the friend hasn't seen yet stays acceptable later.
    #[test]
    fn the_service_retires_only_a_key_a_held_commit_proves() {
        let Keys { k1, k2, under_k2 } = keys();
        // Ana pins k3, listed at 10: k2's certificate (5) is older.
        let k3 = IdentityKey::generate().unwrap();
        let next = IdentityKey::generate().unwrap();
        let mut pins = pinned(&k3, &[&k1], 10);
        let before = pins.clone();
        let served = k3.public().0;

        // Listed, but no held commit under it: not retired.
        assert!(!pins.replaced("matt", served, &[next.public().0], &under_k2).unwrap());
        // Proven, but not listed; or another account's.
        assert!(!pins.replaced("matt", served, &[k1.public().0], &under_k2).unwrap());
        assert!(!pins.replaced("ana", served, &[k2.public().0], &under_k2).unwrap());
        // The current key, pinned or served, listed: refused whole.
        assert!(pins.replaced("matt", served, &[k2.public().0, k3.public().0], &under_k2).is_err());
        assert!(pins.replaced("matt", next.public().0, &[k2.public().0, next.public().0], &under_k2).is_err());
        assert_eq!(pins, before, "nothing changed");

        // Proven and older: retired, with a key not seen yet listed beside
        // it left alone, and taken later.
        assert!(pins.replaced("matt", served, &[k2.public().0, next.public().0], &under_k2).unwrap());
        let pin = pins.get("matt").unwrap();
        assert!(pin.retired.contains(&k2.public()) && !pin.retired.contains(&next.public()));
        let was = before.get("matt").unwrap();
        assert_eq!((pin.identity, pin.verified, pin.changed_unseen), (k3.public(), was.verified, was.changed_unseen), "nothing else");
        pins.key_changed("matt", next.public()).unwrap();
        assert_eq!(pins.get("matt").unwrap().identity, next.public());

        // No list taken under the pinned key yet: nothing until there is.
        let mut unlisted = before.clone();
        unlisted.key_changed("matt", next.public()).unwrap();
        assert!(!unlisted.replaced("matt", next.public().0, &[k2.public().0], &under_k2).unwrap());
    }

    /// Fugo's stale-answer wedge (vi): Matt moved from k1 to k2, and Ana
    /// still pins k1 and holds a genuine commit under k2. A service serving
    /// k1 and listing k2 as replaced gets nothing retired (k2's certificate
    /// is newer than any k1 list), and Ana takes k2 when it reaches her.
    #[test]
    fn a_service_serving_an_old_key_cant_retire_the_next_one() {
        let Keys { k1, k2, under_k2 } = keys();
        let mut pins = pinned(&k1, &[], 1);
        assert!(!pins.replaced("matt", k1.public().0, &[k2.public().0], &under_k2).unwrap_or(true));
        assert!(pins.get("matt").unwrap().retired.is_empty());
        pins.key_changed("matt", k2.public()).unwrap();
        assert_eq!(pins.get("matt").unwrap().identity, k2.public());
    }

    /// Row 202: Matt's tablet, in his conversation with Ana, is reinstalled.
    /// It keeps its leaf id (its account key) and comes back with a new leaf
    /// key, under Matt's same security key, which the phone's list v3 names
    /// in place of the old one. Returns the world (its pins at v3, as every
    /// endpoint has accepted it), the old tablet's group (the wiped install,
    /// as someone holding its keys would have it), the new install and the
    /// GroupInfo the worker keeps.
    struct Reinstall {
        w: World,
        old: End,
        at_old: Group,
        back: End,
        info: Vec<u8>,
    }

    fn reinstall() -> Reinstall {
        let mut w = world();
        let list = |identity: &IdentityKey, version: u64, leaves: Vec<Listed>| {
            DeviceList { account_id: "matt".to_owned(), version, created_ms: 1, leaves }.sign(identity).unwrap().to_bytes().unwrap()
        };
        let old = end(&w.matt, LeafKind::Device, "matt", "tablet");
        w.pins.accept_list("matt", &list(&w.matt, 2, vec![listed(&w.phone), listed(&w.mini), listed(&old)])).unwrap();
        let out = w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[old.me.key_package(&old.provider).unwrap()]).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            w.groups[n].process(&end.provider, &w.pins, &out.commit).unwrap();
        }
        let mut at_old = Group::join(&old.provider, &w.pins, w.chat.clone(), b"conversation-1", &out.welcome.unwrap()).unwrap();
        let update = w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            w.groups[n].process(&end.provider, &w.pins, &update.commit).unwrap();
        }
        at_old.process(&old.provider, &w.pins, &update.commit).unwrap();
        // Named exactly, the tablet is never stale.
        assert!(w.groups[1].stale(&w.pins).is_empty());

        let back = end(&w.matt, LeafKind::Device, "matt", "tablet");
        assert_ne!(back.me.certificate.leaf.signature_key, old.me.certificate.leaf.signature_key);
        w.pins.accept_list("matt", &list(&w.matt, 3, vec![listed(&w.phone), listed(&w.mini), listed(&back)])).unwrap();
        Reinstall { w, old, at_old, back, info: update.group_info.unwrap() }
    }

    /// The new install rejoins past its former install (stale, not dead:
    /// it's under Matt's current key), and nobody sends until it's taken it
    /// out, Ana included, though Ana never removes it herself. It goes by its
    /// tree index, and the new install, under the same leaf id, stays; the
    /// old one reads nothing after.
    #[test]
    fn a_reinstalled_device_rejoins_past_its_former_install_and_takes_it_out_by_index() {
        let Reinstall { mut w, old, mut at_old, back, info } = reinstall();
        let (mut at_back, out) = Group::rejoin(&back.provider, &back.me, &w.pins, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_back.merge_pending(&back.provider).unwrap();
        let stale: Vec<[u8; 32]> = at_back.stale(&w.pins).into_iter().map(|stale| stale.leaf.signature_key).collect();
        assert_eq!(stale, [old.me.certificate.leaf.signature_key], "the former install, by its own key");
        assert!(at_back.dead(&w.pins, &back.me).is_empty(), "not under a replaced key");
        for (n, end) in [(0, &w.phone), (1, &w.ipad), (2, &w.mini)] {
            assert!(matches!(w.groups[n].process(&end.provider, &w.pins, &out.commit).unwrap(), Incoming::Committed { .. }));
        }
        assert_eq!(unchecked_take(&mut at_old, &old.provider, &out.commit), Ok(None));

        // Until it's out, nobody sends: not the new install, not Ana.
        let held = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"too soon");
        assert!(
            matches!(held, Err(Error::Waiting { ref account, reason: validation::Held::Reinstalled }) if account == "matt"),
            "{held:?}"
        );
        assert_eq!(w.groups[1].held(&w.pins, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Reinstalled)));
        assert_eq!(held.unwrap_err().to_string(), "waiting for matt's reinstalled device to finish");
        assert!(matches!(at_back.send(&back.provider, &back.me, &w.pins, b"too soon"), Err(Error::Waiting { .. })));
        // Ana never takes another person's leaf out.
        assert!(w.groups[1].remove_dead(&w.ipad.provider, &w.ipad.me, &w.pins).unwrap().is_none());

        let removal = at_back.remove_dead(&back.provider, &back.me, &w.pins).unwrap().expect("the former install to take out");
        at_back.merge_pending(&back.provider).unwrap();
        for (n, end) in [(0, &w.phone), (1, &w.ipad), (2, &w.mini)] {
            assert!(matches!(w.groups[n].process(&end.provider, &w.pins, &removal.commit).unwrap(), Incoming::Committed { .. }));
        }
        assert!(at_back.stale(&w.pins).is_empty());
        assert!(at_back.remove_dead(&back.provider, &back.me, &w.pins).unwrap().is_none(), "nothing more to take out");
        let left: Vec<(String, [u8; 32])> = w.groups[1]
            .members(&w.pins)
            .into_iter()
            .map(|member| {
                let member = member.unwrap();
                (member.leaf_id().to_owned(), member.certificate.leaf.signature_key)
            })
            .collect();
        let tablets: Vec<_> = left.iter().filter(|(id, _)| id == "tablet").collect();
        assert_eq!(tablets, [&("tablet".to_owned(), back.me.certificate.leaf.signature_key)], "{left:?}");

        // Ana's next message: the new install reads it, the old one can't.
        let said = w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"welcome back").unwrap();
        let Incoming::Application { content, .. } = at_back.process(&back.provider, &w.pins, &said).unwrap() else { panic!() };
        assert_eq!(content, b"welcome back");
        assert!(unchecked_take(&mut at_old, &old.provider, &said).is_err(), "the former install was taken out");
    }

    /// The person's other device takes a former install out too (its sweep),
    /// even if the new install never rejoins this group; a friend never does.
    #[test]
    fn a_persons_other_device_takes_a_former_install_out_and_a_friend_never_does() {
        let Reinstall { mut w, back, .. } = reinstall();
        assert_eq!(w.groups[1].held(&w.pins, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Reinstalled)));
        assert!(w.groups[1].remove_dead(&w.ipad.provider, &w.ipad.me, &w.pins).unwrap().is_none());
        let removal = w.groups[0].remove_dead(&w.phone.provider, &w.phone.me, &w.pins).unwrap().expect("the former install");
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            w.groups[n].process(&end.provider, &w.pins, &removal.commit).unwrap();
        }
        assert!(w.groups[1].leaf_ids().iter().all(|id| id != "tablet"));
        assert_eq!(w.groups[1].held(&w.pins, &w.ipad.me), None);
        w.groups[1].send(&w.ipad.provider, &w.ipad.me, &w.pins, b"sendable again").unwrap();
        // The new install comes in as any added device does.
        let added = w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[back.me.key_package(&back.provider).unwrap()]).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &w.pins, &added.commit).unwrap();
        assert!(w.groups[1].leaf_ids().contains(&"tablet".to_owned()));
    }

    /// The adder's own check (Fugo's condition 1): a KeyPackage from the
    /// former install (same leaf id, its old key), which a stale worker
    /// might still hand out, is refused as off the list, with nothing
    /// staged.
    #[test]
    fn a_former_installs_key_package_is_refused_with_nothing_committed() {
        let Reinstall { mut w, old, .. } = reinstall();
        let removal = w.groups[0].remove_dead(&w.phone.provider, &w.phone.me, &w.pins).unwrap().unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        drop(removal);
        let epoch = w.groups[0].epoch();
        let refused = w.groups[0].add(&w.phone.provider, &w.phone.me, &w.pins, &[old.me.key_package(&old.provider).unwrap()]);
        assert!(matches!(refused, Err(Error::Unlisted { ref account }) if account == "matt"), "{refused:?}");
        assert_eq!(w.groups[0].epoch(), epoch);
        assert!(w.groups[0].mls.pending_commit().is_none(), "nothing staged");
    }

    /// A leaf under a replaced key is dead, never stale: stale is only a
    /// leaf under its account's current key.
    #[test]
    fn a_leaf_under_a_replaced_key_is_dead_not_stale() {
        let StartOver { mut w, fresh, mine, hers, info, .. } = start_over(false);
        let (mut at_fresh, out) = Group::rejoin(&fresh.provider, &fresh.me, &mine, w.chat.clone(), b"conversation-1", &info, 0).unwrap();
        at_fresh.merge_pending(&fresh.provider).unwrap();
        w.groups[1].process(&w.ipad.provider, &hers, &out.commit).unwrap();
        assert!(!at_fresh.dead(&mine, &fresh.me).is_empty());
        assert!(at_fresh.stale(&mine).is_empty());
        assert!(w.groups[1].stale(&hers).is_empty());
        assert_eq!(w.groups[1].held(&hers, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Replaced)));
    }

    /// E11-7 (Fugo 41542): a dead Claktory leaf holds sends only when it's
    /// under a key the person said didn't match (a liar's, planted while
    /// that key was pinned), never under one a real rotation retired.
    #[test]
    fn a_claktory_leaf_under_a_key_the_person_refused_holds_sends() {
        let mut w = world();
        // In Ana's view, Matt's Claktory alone is left with her.
        let phone: Vec<LeafNodeIndex> = w.groups[1]
            .mls
            .members()
            .filter(|member| LeafCertificate::from_credential(&member.credential).is_ok_and(|c| c.leaf.leaf_id == "phone"))
            .map(|member| member.index)
            .collect();
        w.groups[1].mls.remove_members(&w.ipad.provider, &w.ipad.me.key, &phone).unwrap();
        w.groups[1].mls.merge_pending_commit(&w.ipad.provider).unwrap();
        let fake = IdentityKey::generate().unwrap();

        // Matt's real key, refused: "the numbers don't match" when it came
        // back after a lie (here the leaf under it stands for the liar's).
        let mut refused = w.pins.clone();
        refused.key_changed("matt", fake.public()).unwrap();
        refused.key_changed("matt", w.matt.public()).unwrap();
        refused.undo_change("matt", &fake.public()).unwrap();
        assert!(refused.refused("matt", &w.matt.public()));
        assert_eq!(w.groups[1].held(&refused, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Replaced)));

        // Sticky (Fugo 41686): retired since by the service's word and a
        // held commit (`Pins::replaced`), it still holds.
        let mut proven = refused.clone();
        let later = DeviceList { account_id: "matt".to_owned(), version: 1, created_ms: 5, leaves: vec![listed(&w.phone)] };
        proven.accept_list("matt", &later.sign(&fake).unwrap().to_bytes().unwrap()).unwrap();
        assert!(proven.replaced("matt", fake.public().0, &[w.matt.public().0], &w.mini.me.certificate).unwrap());
        assert!(proven.get("matt").unwrap().retired.contains(&w.matt.public()));
        assert_eq!(w.groups[1].held(&proven, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Replaced)));
        // And through a device that has it retired.
        let mut retired_there = w.pins.clone();
        retired_there.key_changed("matt", fake.public()).unwrap();
        retired_there.retire_previous("matt");
        let mut imported = refused.clone();
        imported.import(&retired_there).unwrap();
        assert!(imported.get("matt").unwrap().retired.contains(&w.matt.public()));
        assert_eq!(w.groups[1].held(&imported, &w.ipad.me), Some(("matt".to_owned(), validation::Held::Replaced)));
        let mut synced = retired_there.clone();
        synced.import(&refused).unwrap();
        assert!(synced.refused("matt", &w.matt.public()), "the person's word reaches their other devices");
        // Verified as current after all: no longer refused.
        let mut verified = refused.clone();
        verified.key_changed("matt", w.matt.public()).unwrap();
        verified.verified("matt", &w.matt.public()).unwrap();
        assert!(!verified.refused("matt", &w.matt.public()));

        // Retired by a real rotation: the Claktory leaf left behind doesn't
        // hold the chat.
        let mut rotated = w.pins.clone();
        rotated.key_changed("matt", fake.public()).unwrap();
        rotated.retire_previous("matt");
        assert!(!rotated.refused("matt", &w.matt.public()));
        assert!(!w.groups[1].dead(&rotated, &w.ipad.me).is_empty(), "dead all the same");
        assert_eq!(w.groups[1].held(&rotated, &w.ipad.me), None);
    }

    /// Row 561: Ana adds a tablet and takes it out again while Matt's
    /// phone is away; the phone comes back holding her newest list, which
    /// no longer names the tablet.
    struct Away {
        w: World,
        tablet: End,
        at_tablet: Group,
        /// Ana's list naming the tablet, then her newest without it.
        with_tablet: Pins,
        add: Vec<u8>,
    }

    fn ana_list(w: &World, version: u64, leaves: Vec<Listed>) -> Vec<u8> {
        DeviceList { account_id: "ana".to_owned(), version, created_ms: version, leaves }.sign(&w.ana).unwrap().to_bytes().unwrap()
    }

    fn away() -> Away {
        let mut w = world();
        let tablet = end(&w.ana, LeafKind::Device, "ana", "tablet");
        let mut with_tablet = w.pins.clone();
        with_tablet.accept_list("ana", &ana_list(&w, 2, vec![listed(&w.ipad), listed(&tablet)])).unwrap();
        let out = w.groups[1].add(&w.ipad.provider, &w.ipad.me, &with_tablet, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
        w.groups[1].merge_pending(&w.ipad.provider).unwrap();
        let at_tablet = Group::join(&tablet.provider, &with_tablet, w.chat.clone(), b"conversation-1", &out.welcome.unwrap()).unwrap();
        // The phone has Ana's newest list by the time it reads on.
        w.pins.accept_list("ana", &ana_list(&w, 3, vec![listed(&w.ipad)])).unwrap();
        Away { w, tablet, at_tablet, with_tablet, add: out.commit }
    }

    impl Away {
        fn tablet_says(&mut self, text: &[u8]) -> Vec<u8> {
            self.at_tablet.send(&self.tablet.provider, &self.tablet.me, &self.with_tablet, text).unwrap()
        }

        fn ipad_removes_tablet(&mut self) -> Vec<u8> {
            let out = self.w.groups[1].remove(&self.w.ipad.provider, &self.w.ipad.me, &["tablet"]).unwrap();
            self.w.groups[1].merge_pending(&self.w.ipad.provider).unwrap();
            out.commit
        }

        /// The phone, away until now, reads the refused add, then `rest`.
        fn phone_reads(&mut self, rest: &[&[u8]]) -> Result<Window, Error> {
            let w = &mut self.w;
            assert!(matches!(w.groups[0].process(&w.phone.provider, &w.pins, &self.add), Err(Error::Unlisted { .. })));
            let mut items: Vec<&[u8]> = vec![&self.add];
            items.extend_from_slice(rest);
            w.groups[0].process_window(&w.phone.provider, &w.pins, &items)
        }
    }

    fn aborted(window: Result<Window, Error>) -> String {
        match window {
            Ok(Window::Aborted { why }) => why,
            other => panic!("expected it aborted, got {other:?}"),
        }
    }

    #[test]
    fn an_away_device_reads_what_a_friends_removed_device_said_marked() {
        let mut a = away();
        let said = a.tablet_says(b"from the tablet");
        let ipad_said = a.w.groups[1].send(&a.w.ipad.provider, &a.w.ipad.me, &a.with_tablet, b"from the iPad").unwrap();
        let remove = a.ipad_removes_tablet();
        let after = a.w.groups[1].send(&a.w.ipad.provider, &a.w.ipad.me, &a.w.pins, b"after").unwrap();
        let Ok(Window::Closed { taken }) = a.phone_reads(&[&said, &ipad_said, &remove, &after]) else { panic!("not closed") };
        assert_eq!(taken.len(), 4, "up to the removal");
        let read = |windowed: &Windowed| match &windowed.incoming {
            Ok(Incoming::Application { from, content, .. }) => (from.leaf_id().to_owned(), content.clone(), windowed.departed),
            other => panic!("{other:?}"),
        };
        assert_eq!(read(&taken[1]), ("tablet".to_owned(), b"from the tablet".to_vec(), true));
        assert_eq!(read(&taken[2]), ("ipad".to_owned(), b"from the iPad".to_vec(), false));
        assert!(matches!(taken[3].incoming, Ok(Incoming::Committed { epoch: 3 })));
        // Past it, as ever: the tablet is gone, and the rest reads.
        let w = &mut a.w;
        assert!(w.groups[0].leaf_ids().iter().all(|id| id != "tablet"));
        assert!(matches!(w.groups[0].process(&w.phone.provider, &w.pins, &after).unwrap(), Incoming::Application { .. }));
    }

    #[test]
    fn a_removed_devices_own_update_is_taken_and_anything_else_it_commits_aborts() {
        let mut a = away();
        let update = a.at_tablet.update(&a.tablet.provider, &a.tablet.me).unwrap();
        a.at_tablet.merge_pending(&a.tablet.provider).unwrap();
        a.w.groups[1].process(&a.w.ipad.provider, &a.with_tablet, &update.commit).unwrap();
        let remove = a.ipad_removes_tablet();
        assert!(matches!(a.phone_reads(&[&update.commit, &remove]), Ok(Window::Closed { .. })));

        // The tablet takes the iPad out: never in a window.
        let mut a = away();
        let out = a.at_tablet.remove(&a.tablet.provider, &a.tablet.me, &["ipad"]).unwrap();
        assert!(aborted(a.phone_reads(&[&out.commit])).contains("removed device committed"));
    }

    #[test]
    fn a_removal_by_a_device_that_rejoined_meanwhile_never_closes_a_window() {
        let mut a = away();
        // The iPad lost its state and rejoins (its leaf comes back by an
        // external commit), then takes the tablet out.
        let update = a.w.groups[1].update(&a.w.ipad.provider, &a.w.ipad.me).unwrap();
        a.w.groups[1].merge_pending(&a.w.ipad.provider).unwrap();
        let fresh = End {
            provider: Libcrux::default(),
            me: Me::new(LeafKey::from_secret(&a.w.ipad.me.key.secret()), a.w.ipad.me.certificate.clone()).unwrap(),
        };
        let (mut back, rejoin) =
            Group::rejoin(&fresh.provider, &fresh.me, &a.with_tablet, a.w.chat.clone(), b"conversation-1", &update.group_info.unwrap(), 0)
                .unwrap();
        back.merge_pending(&fresh.provider).unwrap();
        let out = back.remove(&fresh.provider, &fresh.me, &["tablet"]).unwrap();
        let why = aborted(a.phone_reads(&[&update.commit, &rejoin.commit, &out.commit]));
        assert!(why.contains("wasn't there throughout"), "{why}");
    }

    #[test]
    fn a_window_waits_for_its_removal_and_gives_up_past_its_cap() {
        let mut a = away();
        let said = a.tablet_says(b"one");
        assert!(matches!(a.phone_reads(&[&said]), Ok(Window::Open)));
        let mut a = away();
        let many: Vec<Vec<u8>> = (0..WINDOW_MOST).map(|n| a.tablet_says(format!("{n}").as_bytes())).collect();
        let items: Vec<&[u8]> = many.iter().map(Vec::as_slice).collect();
        let why = aborted(a.phone_reads(&items));
        assert!(why.contains("within 64"), "{why}");
    }

    #[test]
    fn a_window_never_opens_in_a_self_group_or_a_channel() {
        let w = world();
        let tablet = end(&w.ana, LeafKind::Device, "ana", "tablet");
        let laptop = end(&w.ana, LeafKind::Device, "ana", "laptop");
        let mut with_tablet = w.pins.clone();
        with_tablet.accept_list("ana", &ana_list(&w, 2, vec![listed(&w.ipad), listed(&laptop), listed(&tablet)])).unwrap();
        let mut newest = w.pins.clone();
        newest.accept_list("ana", &ana_list(&w, 3, vec![listed(&w.ipad), listed(&laptop)])).unwrap();
        let own = GroupKind::SelfGroup { account: "ana".to_owned() };
        let channel = GroupKind::Channel { account: "ana".to_owned(), claktory_id: "studio".to_owned() };
        for (kind, id) in [(own, b"self-1".as_slice()), (channel, b"channel-1".as_slice())] {
            let mut started = Group::create(&w.ipad.provider, &w.ipad.me, kind.clone(), id).unwrap();
            let out = started.add(&w.ipad.provider, &w.ipad.me, &with_tablet, &[laptop.me.key_package(&laptop.provider).unwrap()]).unwrap();
            started.merge_pending(&w.ipad.provider).unwrap();
            let mut at_laptop = Group::join(&laptop.provider, &with_tablet, kind.clone(), id, &out.welcome.unwrap()).unwrap();
            let add = started.add(&w.ipad.provider, &w.ipad.me, &with_tablet, &[tablet.me.key_package(&tablet.provider).unwrap()]).unwrap();
            started.merge_pending(&w.ipad.provider).unwrap();
            let remove = started.remove(&w.ipad.provider, &w.ipad.me, &["tablet"]).unwrap();
            let refused = at_laptop.process_window(&laptop.provider, &newest, &[&add.commit, &remove.commit]);
            assert!(matches!(refused, Err(Error::Unlisted { .. })), "{kind:?}: {refused:?}");
        }
    }

    #[test]
    fn a_removal_by_the_other_persons_device_never_closes_a_window() {
        // Matt's Claktory is the one away; Matt's phone takes Ana's tablet
        // out, which only Ana's own devices may do.
        let mut a = away();
        let w = &mut a.w;
        w.groups[0].process(&w.phone.provider, &a.with_tablet, &a.add).unwrap();
        let out = w.groups[0].remove_of(&w.phone.provider, &w.phone.me, "ana", &["tablet"]).unwrap();
        assert!(matches!(w.groups[2].process(&w.mini.provider, &w.pins, &a.add), Err(Error::Unlisted { .. })));
        let why = aborted(w.groups[2].process_window(&w.mini.provider, &w.pins, &[&a.add, &out.commit]));
        assert!(why.contains("removes only its own person's leaves"), "{why}");
    }
}
