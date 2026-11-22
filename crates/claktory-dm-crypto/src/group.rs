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
    Capabilities, Credential, CredentialWithKey, ExtensionType, GroupId, KeyPackage, KeyPackageIn, LeafNodeIndex, MlsGroup,
    MlsGroupCreateConfig, MlsGroupJoinConfig, MlsMessageBodyIn, MlsMessageIn, MlsMessageOut, PURE_PLAINTEXT_WIRE_FORMAT_POLICY,
    ProcessedMessageContent, ProtocolVersion, Sender, SignaturePublicKey, StagedWelcome,
};
use openmls_traits::OpenMlsProvider;
use openmls_traits::signatures::{Signer, SignerError};
use openmls_traits::types::SignatureScheme;
use tls_codec::{Deserialize as _, Serialize as _};
use zeroize::Zeroizing;

use crate::certificate::{LeafCertificate, LeafKind};
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

    fn credential(&self) -> Result<CredentialWithKey, Error> {
        Ok(CredentialWithKey {
            credential: self.certificate.to_credential()?,
            signature_key: SignaturePublicKey::from(self.key.public().to_vec()),
        })
    }

    /// A one-time key package for the worker's supply, as bytes.
    pub fn key_package(&self, provider: &impl OpenMlsProvider) -> Result<Vec<u8>, Error> {
        let bundle = KeyPackage::builder().build(CIPHERSUITE, provider, &self.key, self.credential()?).map_err(mls)?;
        let message = MlsMessageOut::from(bundle.key_package().clone());
        message.tls_serialize_detached().map_err(mls)
    }

    /// The last-resort KeyPackage: the worker hands it out again once the
    /// one-time ones are gone, so its keys stay until a new one replaces it.
    pub fn last_resort_key_package(&self, provider: &impl OpenMlsProvider) -> Result<Vec<u8>, Error> {
        // Its leaf says it understands the last-resort extension it carries.
        let capabilities = Capabilities::new(None, None, Some(&[ExtensionType::LastResort]), None, None);
        let bundle = KeyPackage::builder()
            .leaf_node_capabilities(capabilities)
            .mark_as_last_resort()
            .build(CIPHERSUITE, provider, &self.key, self.credential()?)
            .map_err(mls)?;
        let message = MlsMessageOut::from(bundle.key_package().clone());
        message.tls_serialize_detached().map_err(mls)
    }
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
    /// Someone said something: who, and the content's bytes (`envelope`'s
    /// to read).
    Application { from: Box<Member>, content: Vec<u8> },
    /// A proposal, kept until a commit covers it.
    Proposal,
    /// A commit, checked and merged: the group is at its next epoch.
    Committed { epoch: u64 },
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
        let staged = StagedWelcome::new_from_welcome(provider, &join_config(), welcome, None).map_err(mls)?;
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
    /// Every member is checked first; a Claktory never rejoins a
    /// conversation by itself (H1). The commit goes to the worker; once it
    /// takes it, `merge_pending`.
    pub fn rejoin(
        provider: &impl OpenMlsProvider,
        me: &Me,
        trust: &impl Trust,
        kind: GroupKind,
        info: &[u8],
    ) -> Result<(Group, Outgoing), Error> {
        if matches!(kind, GroupKind::Conversation { .. }) && me.certificate.leaf.kind == LeafKind::Claktory {
            return Err(Error::Refused("a Claktory doesn't join a conversation by itself".to_owned()));
        }
        let MlsMessageBodyIn::GroupInfo(info) = read(info)? else { return Err(Error::NotMls("not a group info".to_owned())) };
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
        // The members it joins, as the tree it was given has them.
        for member in group.mls.members() {
            if member.index == group.mls.own_leaf_index() {
                continue;
            }
            validation::member(&group.kind, trust, &member.credential, &member.signature_key)?;
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

    /// The members as their certificates say, each checked now.
    pub fn members(&self, trust: &impl Trust) -> Vec<Result<Member, Error>> {
        self.mls.members().map(|member| validation::member(&self.kind, trust, &member.credential, &member.signature_key)).collect()
    }

    fn leaf(&self, index: LeafNodeIndex) -> Option<(Credential, Vec<u8>)> {
        self.mls.member_at(index).map(|member| (member.credential, member.signature_key))
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
        let mut packages = Vec::with_capacity(key_packages.len());
        for bytes in key_packages {
            let MlsMessageBodyIn::KeyPackage(package) = read(bytes)? else { return Err(Error::NotMls("not a key package".to_owned())) };
            let package: KeyPackage = KeyPackageIn::validate(package, provider.crypto(), ProtocolVersion::Mls10)
                .map_err(|_| Error::Refused("a key package that doesn't verify".to_owned()))?;
            let leaf = package.leaf_node();
            let new = validation::member(&self.kind, trust, leaf.credential(), leaf.signature_key().as_slice())?;
            if new.kind() == LeafKind::Claktory
                && (me.certificate.leaf.kind != LeafKind::Device || me.certificate.leaf.account_id != new.account())
            {
                return Err(Error::Refused("only a device of the same person adds a Claktory".to_owned()));
            }
            packages.push(package);
        }
        let (commit, welcome, info) = self.mls.add_members(provider, &me.key, &packages).map_err(mls)?;
        Ok(Outgoing {
            commit: commit.tls_serialize_detached().map_err(mls)?,
            welcome: Some(welcome.tls_serialize_detached().map_err(mls)?),
            group_info: group_info(info)?,
        })
    }

    /// Removes the leaves with these leaf ids (a device removed, a grant
    /// taken back).
    pub fn remove(&mut self, provider: &impl OpenMlsProvider, me: &Me, leaf_ids: &[&str]) -> Result<Outgoing, Error> {
        let indexes: Vec<LeafNodeIndex> = self
            .mls
            .members()
            .filter(|member| {
                LeafCertificate::from_credential(&member.credential)
                    .is_ok_and(|certificate| leaf_ids.contains(&certificate.leaf.leaf_id.as_str()))
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

    /// This leaf's own Update: new keys, for post-compromise security.
    pub fn update(&mut self, provider: &impl OpenMlsProvider, me: &Me) -> Result<Outgoing, Error> {
        let bundle = self.mls.self_update(provider, &me.key, Default::default()).map_err(mls)?;
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

    /// Encrypts `content` (`envelope`'s bytes) for the group.
    pub fn send(&mut self, provider: &impl OpenMlsProvider, me: &Me, content: &[u8]) -> Result<Vec<u8>, Error> {
        if matches!(self.kind, GroupKind::Conversation { .. }) && me.certificate.leaf.kind == LeafKind::Claktory {
            return Err(Error::Refused("a Claktory doesn't send messages in a conversation".to_owned()));
        }
        let message = self.mls.create_message(provider, &me.key, content).map_err(mls)?;
        message.tls_serialize_detached().map_err(mls)
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
        match processed.into_content() {
            ProcessedMessageContent::ApplicationMessage(message) => {
                let Sender::Member(index) = sender else { return Err(Error::Refused("a message from outside the group".to_owned())) };
                let (credential, key) = self.leaf(index).ok_or_else(|| Error::Refused("a message from no member".to_owned()))?;
                let from = validation::application(&self.kind, trust, &credential, &key)?;
                Ok(Incoming::Application { from: Box::new(from), content: message.into_bytes() })
            }
            ProcessedMessageContent::ProposalMessage(proposal) => {
                let Sender::Member(index) = sender else { return Err(Error::Refused("a proposal from outside the group".to_owned())) };
                let (credential, key) = self.leaf(index).ok_or_else(|| Error::Refused("a proposal from no member".to_owned()))?;
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
            _ => Err(Error::Refused("a kind of message Claktory doesn't use".to_owned())),
        }
    }
}

#[cfg(test)]
mod tests {
    use openmls::prelude::{PURE_CIPHERTEXT_WIRE_FORMAT_POLICY, WireFormat};
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
        let said = w.groups[0].send(&w.phone.provider, &w.phone.me, b"hi Ana").unwrap();
        for (n, end) in [(1, &w.ipad), (2, &w.mini)] {
            let Incoming::Application { from, content } = w.groups[n].process(&end.provider, &w.pins, &said).unwrap() else { panic!() };
            assert_eq!((from.account(), from.leaf_id(), content.as_slice()), ("matt", "phone", b"hi Ana".as_slice()));
        }
        // The Claktory won't send there, and if it did, nobody takes it (H1).
        assert!(w.groups[2].send(&w.mini.provider, &w.mini.me, b"psst").is_err());
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
        let raced = w.groups[1].send(&w.ipad.provider, &w.ipad.me, b"sent at epoch 1").unwrap();
        let older = w.groups[1].send(&w.ipad.provider, &w.ipad.me, b"also at epoch 1").unwrap();
        for _ in 0..PAST_EPOCHS {
            let update = w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
            w.groups[0].merge_pending(&w.phone.provider).unwrap();
            w.groups[2].process(&w.mini.provider, &w.pins, &update.commit).unwrap();
        }
        assert_eq!(w.groups[0].epoch(), 1 + PAST_EPOCHS as u64);
        for (n, end) in [(0, &w.phone), (2, &w.mini)] {
            let Incoming::Application { content, .. } = w.groups[n].process(&end.provider, &w.pins, &raced).unwrap() else { panic!() };
            assert_eq!(content, b"sent at epoch 1");
        }
        // One more epoch on, that epoch's secrets are gone.
        w.groups[0].update(&w.phone.provider, &w.phone.me).unwrap();
        w.groups[0].merge_pending(&w.phone.provider).unwrap();
        assert!(w.groups[0].process(&w.phone.provider, &w.pins, &older).is_err());
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
        let later = w.groups[0].send(&w.phone.provider, &w.phone.me, b"after").unwrap();
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
        let (mut back, out) = Group::rejoin(&fresh.provider, &fresh.me, &w.pins, w.chat.clone(), &info).unwrap();
        back.merge_pending(&fresh.provider).unwrap();
        assert!(matches!(w.groups[0].process(&w.phone.provider, &w.pins, &out.commit).unwrap(), Incoming::Committed { .. }));
        assert_eq!(w.groups[0].members(&w.pins).len(), 3, "the old leaf went");
        let said = w.groups[0].send(&w.phone.provider, &w.phone.me, b"welcome back").unwrap();
        assert!(matches!(back.process(&fresh.provider, &w.pins, &said).unwrap(), Incoming::Application { .. }));

        // The Claktory can't rejoin a conversation by itself, and a commit it
        // forces is refused (H1).
        assert!(Group::rejoin(&w.mini.provider, &w.mini.me, &w.pins, w.chat.clone(), &info).is_err());
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
}
