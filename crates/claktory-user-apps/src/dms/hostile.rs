//! Hostile tests (dms-e2ee.md §9, E9) at the worker: what a stale or
//! removed device, an old copy of a Claktory, or a Claktory's leaf in a
//! conversation sends, and what the worker does with it. The person's
//! devices' own checks are in app/rust/src/dm/hostile.rs, the crate's in
//! claktory-dm-crypto's tests/all/hostile.rs; each test is named by its
//! number in the spec's list.

use claktory_dm_crypto::certificate::LeafCertificate;
use claktory_dm_crypto::device_list::{DeviceList, Listed};
use claktory_dm_crypto::envelope::Envelope;
use claktory_dm_crypto::group::{Group as Mls, LeafKey, Me, Outgoing};
use claktory_dm_crypto::identity::IdentityKey;
use claktory_dm_crypto::validation::{GroupKind as Says, Pins};
use openmls::prelude::tls_codec::{Deserialize as _, Serialize as _};
use openmls::prelude::{
    CredentialWithKey, Extensions, GroupId, LeafNodeIndex, LeafNodeParameters, MlsGroup, MlsGroupJoinConfig, MlsMessageBodyIn,
    MlsMessageIn, OpenMlsProvider as _, PURE_PLAINTEXT_WIRE_FORMAT_POLICY,
};
use serde_json::{Value, json};

use super::contacts::tests::World;
use super::leaves::b64;
use super::leaves::tests::{NOW, Person, call, claktory, device, set_up, upload};
use super::messages::tests::{client_id, friends, post};
use crate::grants::{Put, Resource};
use crate::ipc::{Actor, Caller, Registration};

fn env(bytes: Vec<u8>) -> Value {
    Envelope::new(bytes).expect("an envelope").to_json()
}

fn bytes_of(value: &Value) -> Vec<u8> {
    Envelope::from_json(value).expect("an envelope").into_bytes()
}

/// The same envelope, called format 1.
fn format_1(envelope: &Value) -> Value {
    let mut old = envelope.clone();
    old["format"] = json!(1);
    old
}

fn pins(people: &[&Person]) -> Pins {
    let mut pins = Pins::default();
    for person in people {
        pins.first_contact(&person.account, person.identity.public()).expect("pinned");
        pins.accept_list(&person.account, &person.signed.as_ref().expect("a list").to_bytes().expect("bytes")).expect("accepted");
    }
    pins
}

fn start(world: &mut World, person: &Person, leaf: usize, says: Says, id: [u8; 16], mut body: Value) -> Mls {
    let (group, info) = person.with(leaf, |provider, me| {
        let group = Mls::create(provider, me, says, &id).expect("made");
        let info = group.group_info(provider, me).expect("a GroupInfo");
        (group, info)
    });
    body["group_info"] = env(info);
    assert_eq!(call(world, &person.caller(leaf), "POST", "groups", body).0, 200);
    group
}

fn claim_one(world: &mut World, caller: &Caller, account: &str, leaf_id: &str) -> Vec<u8> {
    let (status, claimed) = call(world, caller, "POST", "key-packages/claim", json!({"account_id": account, "leaf_ids": [leaf_id]}));
    assert_eq!(status, 200, "{claimed}");
    bytes_of(&claimed["key_packages"][0]["key_package"])
}

fn post_commit(world: &mut World, caller: &Caller, id: &[u8], out: &Outgoing) -> (u16, Value) {
    let mut body = json!({"commit": env(out.commit.clone()), "group_info": env(out.group_info.clone().expect("a GroupInfo"))});
    if let Some(welcome) = &out.welcome {
        body["welcome"] = env(welcome.clone());
    }
    call(world, caller, "POST", &format!("groups/{}/commits", b64(id)), body)
}

fn mailbox(world: &mut World, caller: &Caller, id: &[u8]) -> (u16, Value) {
    call(world, caller, "GET", &format!("groups/{}/messages?after=0", b64(id)), Value::Null)
}

fn group_info(world: &mut World, caller: &Caller, id: &[u8]) -> Vec<u8> {
    let (status, info) = call(world, caller, "GET", &format!("groups/{}/group-info", b64(id)), Value::Null);
    assert_eq!(status, 200, "{info}");
    bytes_of(&info["group_info"])
}

fn phone_of(account: &str, epoch: u64) -> Caller {
    Caller::Device {
        account_id: account.into(),
        device_key: format!("{account}-phone"),
        registrations: vec![Registration { claktory_id: "mac".into(), epoch }],
        removed_devices: Vec::new(),
        signed_in_at: None,
        bound_devices: None,
    }
}

fn backend(account: &str, epoch: u64) -> Caller {
    Caller::Backend { account_id: account.into(), claktory_id: "mac".into(), epoch, agent: None }
}

/// `PUT me/keys`'s body for a list of `person`'s leaves but those in
/// `left_out`, at the next version.
fn list_without(person: &mut Person, left_out: &[usize]) -> Value {
    let previous = person.version;
    person.version += 1;
    let leaves = person
        .leaves
        .iter()
        .enumerate()
        .filter(|(n, _)| !left_out.contains(n))
        .map(|(_, (_, me))| Listed {
            kind: me.certificate.leaf.kind,
            leaf_id: me.certificate.leaf.leaf_id.clone(),
            signature_key: me.certificate.leaf.signature_key,
        })
        .collect();
    let list = DeviceList { account_id: person.account.clone(), version: person.version, created_ms: NOW as u64, leaves };
    let signed = list.sign(&person.identity).expect("signed");
    let bytes = signed.to_bytes().expect("bytes");
    person.signed = Some(signed);
    json!({"device_list": b64(&bytes), "version": person.version, "previous_version": previous})
}

/// An external commit made with OpenMLS alone, as `person`'s leaf
/// `leaf`, from `info`: past every check of the crate's own.
fn forced_rejoin(person: &Person, leaf: usize, info: &[u8]) -> Outgoing {
    let MlsMessageBodyIn::GroupInfo(info) = MlsMessageIn::tls_deserialize_exact(info).expect("a message").extract() else {
        panic!("not a GroupInfo")
    };
    // Whatever it kept of the group goes first, as a rejoin does.
    let group_id = openmls::prelude::GroupId::from_slice(info.group_context().group_id().as_slice());
    claktory_dm_crypto::storage::forget_group(&person.stores[leaf], &group_id).expect("forgot");
    person.with(leaf, |provider, me| {
        let config =
            MlsGroupJoinConfig::builder().use_ratchet_tree_extension(true).wire_format_policy(PURE_PLAINTEXT_WIRE_FORMAT_POLICY).build();
        let with_key = CredentialWithKey {
            credential: me.certificate.to_credential().expect("a credential"),
            signature_key: me.key.public().to_vec().into(),
        };
        let (_, bundle) = MlsGroup::external_commit_builder()
            .with_config(config)
            .build_group(provider, info, with_key)
            .expect("built")
            .load_psks(provider.storage())
            .expect("psks")
            .build(provider.rand(), provider.crypto(), &me.key, |_| true)
            .expect("a commit")
            .finalize(provider)
            .expect("finalized");
        let (commit, _, info) = bundle.into_contents();
        Outgoing {
            commit: commit.tls_serialize_detached().expect("bytes"),
            welcome: None,
            group_info: info.map(|info| openmls::prelude::MlsMessageOut::from(info).tls_serialize_detached().expect("bytes")),
        }
    })
}

/// Test 3: what a device sent, sent again: a commit at an epoch the group
/// left, a message more than five epochs old, are refused with the epoch
/// now; the same message under its own id is the one already kept.
#[test]
fn test_3_a_replayed_commit_or_old_message_is_refused_by_epoch() {
    let mut world = World::new();
    let ana = set_up(&mut world, "ana");
    let phone = ana.caller(0);
    let id = [1; 16];
    let mut group = start(&mut world, &ana, 0, Says::SelfGroup { account: "ana".into() }, id, json!({"kind": "self"}));
    let said = ana.with(0, |provider, me| group.send(provider, me, &Pins::default(), b"draft").expect("sent"));
    let path = format!("groups/{}/messages", b64(&id));
    let send = |id: &str| json!({"client_message_id": id, "envelope": env(said.clone())});
    assert_eq!(call(&mut world, &phone, "POST", &path, send("d-1")).1["seq"], 1);

    let mut commits = Vec::new();
    for epoch in 1..=6 {
        let update = ana.with(0, |provider, me| group.update(provider, me).expect("updated"));
        assert_eq!(post_commit(&mut world, &phone, &id, &update).1["epoch"], epoch);
        ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
        commits.push(update);
    }
    // Each commit again, and the first one last: refused, with the epoch.
    for commit in commits.iter().rev() {
        let (status, refused) = post_commit(&mut world, &phone, &id, commit);
        assert_eq!((status, &refused["error"], &refused["epoch"]), (409, &json!("epoch"), &json!(6)), "{refused}");
    }
    // The epoch-0 message, six epochs on, as a new one: refused.
    let (status, refused) = call(&mut world, &phone, "POST", &path, send("d-2"));
    assert_eq!((status, &refused["error"]), (409, &json!("epoch")));
    // Under its own id, it's the one kept, not another.
    assert_eq!(call(&mut world, &phone, "POST", &path, send("d-1")).1["seq"], 1);
    let (_, page) = mailbox(&mut world, &phone, &id);
    assert_eq!(page["messages"].as_array().map(Vec::len), Some(7), "the message and six commits, nothing twice");
}

/// Test 4: a format-1 body, on every route that takes an envelope: refused.
#[test]
fn test_4_format_1_is_refused_on_every_route() {
    let mut world = World::new();
    let mut ana = set_up(&mut world, "ana");
    let phone = ana.caller(0);
    let id = [2; 16];
    let says = Says::SelfGroup { account: "ana".into() };
    let info =
        ana.with(0, |provider, me| Mls::create(provider, me, says.clone(), &id).expect("made").group_info(provider, me).expect("info"));
    let old = json!({"format": 1, "text": "in the clear"});
    for group_info in [format_1(&env(info.clone())), old.clone()] {
        assert_eq!(call(&mut world, &phone, "POST", "groups", json!({"kind": "self", "group_info": group_info})).0, 400);
    }
    let mut group = start(&mut world, &ana, 0, says, [3; 16], json!({"kind": "self"}));
    let id = [3; 16];
    let said = ana.with(0, |provider, me| group.send(provider, me, &Pins::default(), b"hi").expect("sent"));
    let path = format!("groups/{}/messages", b64(&id));
    for envelope in [format_1(&env(said)), old.clone()] {
        assert_eq!(call(&mut world, &phone, "POST", &path, json!({"client_message_id": "m-1", "envelope": envelope})).0, 400);
    }
    let update = ana.with(0, |provider, me| group.update(provider, me).expect("updated"));
    let (commit, info) = (env(update.commit.clone()), env(update.group_info.clone().expect("info")));
    let commits = format!("groups/{}/commits", b64(&id));
    for body in [
        json!({"commit": format_1(&commit), "group_info": info}),
        json!({"commit": commit, "group_info": format_1(&info)}),
        json!({"commit": old, "group_info": info}),
    ] {
        assert_eq!(call(&mut world, &phone, "POST", &commits, body).0, 400);
    }
    let proposals = format!("groups/{}/proposals", b64(&id));
    assert_eq!(call(&mut world, &phone, "POST", &proposals, json!({"proposal": format_1(&commit)})).0, 400);
    let mac = ana.add(device("ana", "ana-mac"));
    assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(false)).0, 200);
    let packages: Vec<Value> = ana.key_packages(mac, 2).iter().map(format_1).collect();
    assert_eq!(upload(&mut world, &ana.caller(mac), packages.clone(), None).0, 400);
    assert_eq!(upload(&mut world, &ana.caller(mac), Vec::new(), Some(packages[0].clone())).0, 400);
    // Nothing of it was kept: the commit, in format 2, still goes.
    assert_eq!(post_commit(&mut world, &phone, &id, &update), (200, json!({"seq": 1, "epoch": 1})));
}

/// Test 5: a device taken off the person's list can't rejoin their groups
/// by an external commit, though it could while listed.
#[test]
fn test_5_a_removed_device_cant_rejoin() {
    let mut world = World::new();
    let mut ana = set_up(&mut world, "ana");
    let phone = ana.caller(0);
    let mac = ana.add(device("ana", "ana-mac"));
    assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(false)).0, 200);
    let says = Says::SelfGroup { account: "ana".into() };
    let id = [4; 16];
    let mut group = start(&mut world, &ana, 0, says.clone(), id, json!({"kind": "self"}));
    let trust = pins(&[&ana]);
    let info = group_info(&mut world, &ana.caller(mac), &id);
    let (_, joined) = ana.with(mac, |provider, me| Mls::rejoin(provider, me, &trust, says.clone(), &id, &info).expect("rejoined"));
    assert_eq!(post_commit(&mut world, &ana.caller(mac), &id, &joined).0, 200, "listed, it rejoins");
    ana.with(0, |provider, _| group.process(provider, &trust, &joined.commit).expect("processed"));

    // Lost: the phone lists Ana's devices without the Mac and removes it.
    assert_eq!(call(&mut world, &phone, "PUT", "me/keys", list_without(&mut ana, &[mac])).0, 200);
    let removed = ana.with(0, |provider, me| group.remove(provider, me, &["ana-mac"]).expect("removed"));
    assert_eq!(post_commit(&mut world, &phone, &id, &removed).0, 200);
    ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));

    // The Mac rejoins from the latest GroupInfo, by our code and forced.
    let info = group_info(&mut world, &ana.caller(mac), &id);
    claktory_dm_crypto::storage::forget_group(&ana.stores[mac], &openmls::prelude::GroupId::from_slice(&id)).expect("forgot");
    let fresh = ana.with(mac, |provider, me| Mls::rejoin(provider, me, &trust, says.clone(), &id, &info).expect("made").1);
    for commit in [fresh, forced_rejoin(&ana, mac, &info)] {
        let (status, refused) = post_commit(&mut world, &ana.caller(mac), &id, &commit);
        assert_eq!(status, 400, "{refused}");
        assert!(refused["message"].as_str().is_some_and(|message| message.contains("device list")), "{refused}");
    }
    assert_eq!(group_info(&mut world, &phone, &id), info, "nothing merged");
}

/// Test 13 (H1), the worker's part: a Claktory's leaf in a conversation
/// its agents are granted on sends nothing there, commits no change of
/// anyone's, and never joins by itself.
#[test]
fn test_13_a_claktory_in_a_conversation_reads_only() {
    let mut world = World::new();
    let (conversation, generation) = friends(&mut world, "ana", "bo");
    let mut ana = Person::new("ana");
    ana.add(phone_of("ana", 1));
    let mac = ana.add(claktory("ana", "mac"));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(true)).0, 200);
    let bo = set_up(&mut world, "bo");
    assert_eq!(upload(&mut world, &bo.caller(0), bo.key_packages(0, 1), None).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(mac), ana.key_packages(mac, 2), None).0, 200);
    let trust = pins(&[&ana, &bo]);
    let (phone, machine) = (ana.caller(0), ana.caller(mac));
    let says = Says::Conversation { people: ["ana".into(), "bo".into()] };
    let id = [6; 16];
    let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
    let mut chat = start(&mut world, &ana, 0, says.clone(), id, body);
    let package = claim_one(&mut world, &phone, "bo", "bo-phone");
    let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    ana.with(0, |provider, _| chat.merge_pending(provider).expect("merged"));
    let june = Actor { id: "june".into(), incarnation: "1".into() };
    let put = Put {
        app: "dms".into(),
        agent: june,
        scopes: vec!["dm.read".into(), "dm.send".into()],
        resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
        expires_ms: 0,
    };
    crate::grants::put(&mut world.db, &machine, put, world.now).expect("granted");
    let package = claim_one(&mut world, &phone, "ana", "mac#1");
    let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    ana.with(0, |provider, _| chat.merge_pending(provider).expect("merged"));
    let (_, welcomes) = call(&mut world, &machine, "GET", "welcomes", Value::Null);
    let mut its = ana.with(mac, |provider, _| {
        Mls::join(provider, &trust, says.clone(), &id, &bytes_of(&welcomes["welcomes"][0]["welcome"])).expect("joined")
    });
    assert_eq!(mailbox(&mut world, &machine, &id).0, 200, "it reads");

    // It speaks: neither through the group's route nor the conversation's.
    // (The worker can't open a message to see who wrote it: one framed as
    // the group's at its epoch stands for what the leaf would encrypt.)
    let said = super::bodies::tests::sealed(&id, its.epoch(), "psst");
    let (status, _) =
        call(&mut world, &machine, "POST", &format!("groups/{}/messages", b64(&id)), json!({"client_message_id": "c-1", "envelope": said}));
    assert_eq!(status, 400);
    let send = json!({"client_message_id": client_id(1), "contact_generation": generation, "envelope": said});
    assert_eq!(call(&mut world, &machine, "POST", &format!("conversations/{conversation}/messages"), send).0, 403);

    // It commits a change of someone else's: refused.
    let removal = ana.with(mac, |provider, me| its.remove_of(provider, me, "bo", &["bo-phone"]).expect("made"));
    let (status, refused) = post_commit(&mut world, &machine, &id, &removal);
    assert_eq!((status, &refused["error"]), (403, &json!("claktory_update_only")), "{refused}");
    ana.with(mac, |provider, _| its.clear_pending(provider).expect("cleared"));

    // It joins by itself, as itself: refused, however it's made.
    let info = group_info(&mut world, &machine, &id);
    let rejoin = forced_rejoin(&ana, mac, &info);
    assert_eq!(post_commit(&mut world, &machine, &id, &rejoin).0, 404);
    assert_eq!(group_info(&mut world, &phone, &id), info, "nothing merged");
}

/// Test 17, the worker's part: moved, a Claktory enrolls a new leaf under
/// its new registration; its old leaf, off the list, can't rejoin the
/// channel, and the new registration holds none of the old one's grants.
#[test]
fn test_17_an_old_copy_of_a_claktory_has_no_way_back() {
    let mut world = World::new();
    let (conversation, _) = friends(&mut world, "ana", "bo");
    let mut ana = Person::new("ana");
    ana.add(phone_of("ana", 1));
    let old = ana.add(backend("ana", 1));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(true)).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(old), ana.key_packages(old, 1), None).0, 200);
    let trust = pins(&[&ana]);
    let says = Says::Channel { account: "ana".into(), claktory_id: "mac".into() };
    let id = [7; 16];
    let mut channel = start(&mut world, &ana, 0, says.clone(), id, json!({"kind": "channel", "claktory_id": "mac"}));
    let package = claim_one(&mut world, &ana.caller(0), "ana", "mac#1");
    let out = ana.with(0, |provider, me| channel.add(provider, me, &trust, &[package]).expect("added"));
    assert_eq!(post_commit(&mut world, &ana.caller(0), &id, &out).0, 200);
    ana.with(0, |provider, _| channel.merge_pending(provider).expect("merged"));
    let june = Actor { id: "june".into(), incarnation: "1".into() };
    let put = Put {
        app: "dms".into(),
        agent: june,
        scopes: vec!["dm.read".into()],
        resources: vec![Resource { kind: "conversation".into(), id: conversation }],
        expires_ms: 0,
    };
    crate::grants::put(&mut world.db, &ana.caller(old), put, world.now).expect("granted");

    // Moved: registration 2, a new leaf listed in the old one's place, and
    // the old one removed from the channel.
    ana.leaves[0].0 = phone_of("ana", 2);
    let new = ana.add(backend("ana", 2));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", list_without(&mut ana, &[old])).0, 200);
    let removed = ana.with(0, |provider, me| channel.remove(provider, me, &["mac#1"]).expect("removed"));
    assert_eq!(post_commit(&mut world, &ana.caller(0), &id, &removed).0, 200);
    ana.with(0, |provider, _| channel.merge_pending(provider).expect("merged"));

    // The old copy rejoins the channel from its GroupInfo: refused.
    let info = group_info(&mut world, &ana.caller(old), &id);
    let (status, refused) = post_commit(&mut world, &ana.caller(old), &id, &forced_rejoin(&ana, old, &info));
    assert_eq!(status, 400, "{refused}");
    assert!(refused["message"].as_str().is_some_and(|message| message.contains("device list")), "{refused}");
    // The new registration has none of the old one's grants.
    let (_, listed) = call(&mut world, &ana.caller(new), "GET", "groups", Value::Null);
    let kinds: Vec<&str> = listed["groups"].as_array().expect("groups").iter().filter_map(|group| group["kind"].as_str()).collect();
    assert_eq!(kinds, ["channel"], "{listed}");
    let grants = crate::grants::of_claktory(&world.db, "ana", "mac", 2, "dms", world.now).expect("grants");
    assert!(grants.is_empty());
}

/// Test 18, the worker's part (an honest service: Fugo's ruling, §7): a
/// Claktory's mailbox for a conversation starts at its agents' earliest
/// live grant there, so a later grant, once the earlier one ends, shows
/// nothing from before it. Inside the Claktory, the leaf keeps the same
/// window (factoryd's local_page).
#[test]
fn test_18_a_later_grant_starts_the_mailbox_at_itself() {
    let mut world = World::new();
    let (conversation, generation) = friends(&mut world, "ana", "bo");
    let mut ana = Person::new("ana");
    ana.add(phone_of("ana", 1));
    let mac = ana.add(claktory("ana", "mac"));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(true)).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(mac), ana.key_packages(mac, 1), None).0, 200);
    let bo = set_up(&mut world, "bo");
    assert_eq!(upload(&mut world, &bo.caller(0), bo.key_packages(0, 1), None).0, 200);
    let trust = pins(&[&ana, &bo]);
    let (phone, machine) = (ana.caller(0), ana.caller(mac));
    let id = [8; 16];
    let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
    let mut chat = start(&mut world, &ana, 0, Says::Conversation { people: ["ana".into(), "bo".into()] }, id, body);
    let package = claim_one(&mut world, &phone, "bo", "bo-phone");
    let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    ana.with(0, |provider, _| chat.merge_pending(provider).expect("merged"));

    let grant = |world: &mut World, agent: &str| {
        let put = Put {
            app: "dms".into(),
            agent: Actor { id: agent.into(), incarnation: "1".into() },
            scopes: vec!["dm.read".into()],
            resources: vec![Resource { kind: "conversation".into(), id: conversation.clone() }],
            expires_ms: 0,
        };
        crate::grants::put(&mut world.db, &machine, put, world.now).expect("granted").id
    };
    world.now += 1_000;
    let june = grant(&mut world, "june");
    let package = claim_one(&mut world, &phone, "ana", "mac#1");
    let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &[package]).expect("added"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    world.now += 1_000;
    assert_eq!(post(&mut world, "bo", &conversation, generation, &client_id(1), "while June reads").0, 200);
    let seqs =
        |page: &Value| -> Vec<i64> { page["messages"].as_array().expect("messages").iter().filter_map(|m| m["seq"].as_i64()).collect() };
    assert_eq!(seqs(&mailbox(&mut world, &machine, &id).1), [2, 3], "June's grant on: the Add and Bo's message");

    // Mike's grant comes later; June's ends. The leaf is still in the group.
    world.now += 1_000;
    grant(&mut world, "mike");
    world.now += 1_000;
    crate::grants::revoke(&mut world.db, &phone, &june, world.now).expect("revoked");
    world.now += 1_000;
    assert_eq!(post(&mut world, "bo", &conversation, generation, &client_id(2), "while Mike reads").0, 200);
    assert_eq!(seqs(&mailbox(&mut world, &machine, &id).1), [4], "from Mike's grant on only");
}

/// How many commits `key` counted this hour (`dm_limits`).
fn counted(world: &World, key: &str) -> i64 {
    world.db.query_row("SELECT COALESCE(SUM(count), 0) FROM dm_limits WHERE key = ?1", [key], |row| row.get(0)).expect("counted")
}

/// `person`'s new security key, as the rotating device makes it: the
/// leaves `kept` certified again under it (same leaf, same signature key),
/// on a version-1 list it signs, sent with `me/keys/rotate` by the first
/// of them. The old key is returned, as a thief would still hold it; the
/// new certificates, by leaf, for each leaf's move. `person.leaves` keeps
/// the old ones, which its groups still hold.
fn rotate(world: &mut World, person: &mut Person, kept: &[usize]) -> (IdentityKey, Vec<Me>) {
    let old = std::mem::replace(&mut person.identity, IdentityKey::generate().expect("a key"));
    let moved: Vec<Me> = kept
        .iter()
        .map(|&n| {
            let me = &person.leaves[n].1;
            let certificate = LeafCertificate::issue(&person.identity, me.certificate.leaf.clone()).expect("issued");
            me.recertified(certificate).expect("the same leaf")
        })
        .collect();
    let leaves = moved
        .iter()
        .map(|me| Listed {
            kind: me.certificate.leaf.kind,
            leaf_id: me.certificate.leaf.leaf_id.clone(),
            signature_key: me.certificate.leaf.signature_key,
        })
        .collect();
    let signed = DeviceList { account_id: person.account.clone(), version: 1, created_ms: NOW as u64, leaves }
        .sign(&person.identity)
        .expect("signed");
    let body = json!({"identity_key": b64(&person.identity.public().0), "device_list": b64(&signed.to_bytes().expect("bytes")),
                      "version": 1});
    person.version = 1;
    person.signed = Some(signed);
    let (status, shown) = call(world, &person.caller(kept[0]), "POST", "me/keys/rotate", body);
    assert_eq!(status, 200, "{shown}");
    (old, moved)
}

/// A commit made with OpenMLS alone as `person`'s leaf `leaf` in group
/// `id`: its own leaf now `credential` (under `leaf`'s own signature key),
/// with `removals` and, if `context`, a GroupContext change alongside;
/// nothing the crate's own `update_certificate` would make. Left pending
/// nowhere.
fn forced_move(person: &Person, leaf: usize, id: &[u8], credential: CredentialWithKey, removals: &[u32], context: bool) -> Outgoing {
    person.with(leaf, |provider, me| {
        let mut mls = MlsGroup::load(provider.storage(), &GroupId::from_slice(id)).expect("loaded").expect("a group");
        let mut builder = mls
            .commit_builder()
            .consume_proposal_store(false)
            .force_self_update(true)
            .leaf_node_parameters(LeafNodeParameters::builder().with_credential_with_key(credential).build())
            .propose_removals(removals.iter().map(|&index| LeafNodeIndex::new(index)));
        if context {
            builder = builder.propose_group_context_extensions(Extensions::empty()).expect("proposed");
        }
        let bundle = builder
            .load_psks(provider.storage())
            .expect("psks")
            .build(provider.rand(), provider.crypto(), &me.key, |_| true)
            .expect("a commit")
            .stage_commit(provider)
            .expect("staged");
        let (commit, _, info) = bundle.into_contents();
        let out = Outgoing {
            commit: commit.tls_serialize_detached().expect("bytes"),
            welcome: None,
            group_info: info.map(|info| openmls::prelude::MlsMessageOut::from(info).tls_serialize_detached().expect("bytes")),
        };
        mls.clear_pending_commit(provider.storage()).expect("cleared");
        out
    })
}

/// The same leaf again: its key and certificate.
fn copy(me: &Me) -> Me {
    me.recertified(me.certificate.clone()).expect("a copy")
}

fn credential_of(certificate: &LeafCertificate, key: &LeafKey) -> CredentialWithKey {
    CredentialWithKey { credential: certificate.to_credential().expect("a credential"), signature_key: key.public().to_vec().into() }
}

/// Test 25, the worker's part (R3): after a new security key, nothing under
/// the old one is taken: no device list it signs, no way back to it, no
/// KeyPackage it certified, kept or new.
#[test]
fn test_25_after_a_new_key_nothing_under_the_old_one_is_taken() {
    let mut world = World::new();
    let mut ana = set_up(&mut world, "ana");
    let mac = ana.add(device("ana", "ana-mac"));
    let ipad = ana.add(device("ana", "ana-ipad"));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(mac), ana.key_packages(mac, 2), None).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(ipad), ana.key_packages(ipad, 2), None).0, 200);

    // The iPad was stolen: Ana's other devices go on under a new key.
    let (old, moved) = rotate(&mut world, &mut ana, &[0, mac]);
    // What the old key certified is gone: the Mac's and the iPad's alike.
    let (_, claimed) = call(&mut world, &ana.caller(0), "POST", "key-packages/claim", json!({"account_id": "ana"}));
    assert_eq!(claimed["key_packages"], json!([]), "{claimed}");
    // The thief, with the old key: a KeyPackage, a list naming the iPad, a
    // rotation back to it. All refused.
    assert_eq!(upload(&mut world, &ana.caller(ipad), ana.key_packages(ipad, 1), None).0, 400);
    let mut thief = Person::new("ana");
    thief.identity = old;
    thief.leaves = vec![(ana.caller(ipad), copy(&ana.leaves[ipad].1))];
    thief.version = 1;
    let (status, refused) = call(&mut world, &ana.caller(ipad), "PUT", "me/keys", thief.put(false));
    assert_eq!(status, 400, "{refused}");
    thief.version = 0;
    let list = thief.list();
    let back = json!({"identity_key": b64(&thief.identity.public().0), "device_list": list, "version": 1});
    let (status, refused) = call(&mut world, &ana.caller(ipad), "POST", "me/keys/rotate", back);
    assert_eq!(status, 400, "{refused}");
    assert!(refused["message"].as_str().is_some_and(|message| message.contains("replaced")), "{refused}");
    // Under the new key, the Mac's KeyPackages are taken again.
    let me = copy(&moved[1]);
    let fresh = ana.with(mac, |provider, _| Envelope::new(me.key_package(provider).expect("a KeyPackage")).expect("fits").to_json());
    assert_eq!(upload(&mut world, &ana.caller(mac), vec![fresh], None).0, 200);
}

/// Tests 26 and 27, the worker's part (R3, Fugo's three tightenings): in
/// a group, a leaf under the replaced key does one thing, move itself to
/// the new key, keeping its signature key, with nothing else in the
/// commit; its proposals and other commits are refused, and so is a
/// friend's commit that takes in one of them. The stolen device's leaf
/// can't rejoin. A new key's moves, and the Removes of the account's own
/// leaves its key no longer vouches for, count apart from the usual budget.
#[test]
fn test_26_27_under_a_replaced_key_a_leaf_only_moves_itself() {
    let mut world = World::new();
    let mut ana = set_up(&mut world, "ana");
    let phone = ana.caller(0);
    let mac = ana.add(device("ana", "ana-mac"));
    let ipad = ana.add(device("ana", "ana-ipad"));
    assert_eq!(call(&mut world, &phone, "PUT", "me/keys", ana.put(false)).0, 200);
    for leaf in [mac, ipad] {
        assert_eq!(upload(&mut world, &ana.caller(leaf), ana.key_packages(leaf, 1), None).0, 200);
    }
    let says = Says::SelfGroup { account: "ana".into() };
    let id = [9; 16];
    let mut group = start(&mut world, &ana, 0, says.clone(), id, json!({"kind": "self"}));
    let mut trust = pins(&[&ana]);
    let packages = [claim_one(&mut world, &phone, "ana", "ana-mac"), claim_one(&mut world, &phone, "ana", "ana-ipad")];
    let out = ana.with(0, |provider, me| group.add(provider, me, &trust, &packages).expect("added"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
    let mut on_mac = ana
        .with(mac, |provider, _| Mls::join(provider, &trust, says.clone(), &id, out.welcome.as_ref().expect("a Welcome")).expect("joined"));
    assert_eq!((counted(&world, "commit:ana-phone"), counted(&world, "rotation-commit:ana-phone")), (1, 0), "an Add: the usual budget");

    // The iPad is stolen: off the list, and out of the group. Its Remove
    // counts apart.
    assert_eq!(call(&mut world, &phone, "PUT", "me/keys", list_without(&mut ana, &[ipad])).0, 200);
    trust.accept_list("ana", &ana.signed.as_ref().expect("a list").to_bytes().expect("bytes")).expect("accepted");
    let removed = ana.with(0, |provider, me| group.remove(provider, me, &["ana-ipad"]).expect("removed"));
    assert_eq!(post_commit(&mut world, &phone, &id, &removed).0, 200);
    ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
    ana.with(mac, |provider, _| on_mac.process(provider, &trust, &removed.commit).expect("processed"));
    assert_eq!((counted(&world, "commit:ana-phone"), counted(&world, "rotation-commit:ana-phone")), (1, 1));

    // A new key for the phone and the Mac.
    let (_, moved) = rotate(&mut world, &mut ana, &[0, mac]);
    trust.key_changed("ana", ana.identity.public()).expect("changed");
    trust.accept_list("ana", &ana.signed.as_ref().expect("a list").to_bytes().expect("bytes")).expect("accepted");

    // Under the old key: the Mac asks to leave, the phone updates as
    // usual. Refused.
    let leave = ana.with(mac, |provider, me| on_mac.leave(provider, me).expect("a proposal"));
    let (status, refused) =
        call(&mut world, &ana.caller(mac), "POST", &format!("groups/{}/proposals", b64(&id)), json!({"proposal": env(leave)}));
    assert_eq!((status, &refused["error"]), (403, &json!("replaced_key")), "{refused}");
    let update = ana.with(0, |provider, me| group.update(provider, me).expect("updated"));
    assert_eq!(post_commit(&mut world, &phone, &id, &update).0, 403, "an update that keeps the old certificate");
    ana.with(0, |provider, _| group.clear_pending(provider).expect("cleared"));

    // The move with anything else in it: a Remove, a GroupContext change.
    let new = credential_of(&moved[0].certificate, &moved[0].key);
    for (removals, context) in [(&[1u32][..], false), (&[][..], true)] {
        let mixed = forced_move(&ana, 0, &id, new.clone(), removals, context);
        let (status, refused) = post_commit(&mut world, &phone, &id, &mixed);
        assert_eq!((status, &refused["error"]), (403, &json!("replaced_key")), "{refused}");
    }
    // A move to a certificate for another signature key; one to another
    // leaf's; one under a key the commit brings itself: refused.
    let other = LeafKey::generate().expect("a key");
    let mut leaf = moved[0].certificate.leaf.clone();
    leaf.signature_key = other.public();
    let for_other_key = LeafCertificate::issue(&ana.identity, leaf).expect("issued");
    let mut mac_leaf = moved[1].certificate.leaf.clone();
    mac_leaf.signature_key = moved[0].certificate.leaf.signature_key;
    let as_the_mac = LeafCertificate::issue(&ana.identity, mac_leaf).expect("issued");
    let unknown = LeafCertificate::issue(&IdentityKey::generate().expect("a key"), moved[0].certificate.leaf.clone()).expect("issued");
    for certificate in [for_other_key, as_the_mac, unknown] {
        let wrong = forced_move(&ana, 0, &id, credential_of(&certificate, &moved[0].key), &[], false);
        let (status, refused) = post_commit(&mut world, &phone, &id, &wrong);
        assert_eq!((status, &refused["error"]), (403, &json!("not_current")), "{refused}");
    }
    // The stolen iPad's leaf rejoins: refused.
    let info = group_info(&mut world, &phone, &id);
    let (status, refused) = post_commit(&mut world, &ana.caller(ipad), &id, &forced_rejoin(&ana, ipad, &info));
    assert_eq!(status, 400, "{refused}");
    assert_eq!((counted(&world, "commit:ana-phone"), counted(&world, "rotation-commit:ana-phone")), (1, 1), "nothing refused counts");

    // The phone's own move: taken, in the rotation's budget.
    let out = ana.with(0, |provider, _| group.update_certificate(provider, &moved[0]).expect("moved"));
    assert_eq!(post_commit(&mut world, &phone, &id, &out).0, 200);
    ana.with(0, |provider, _| group.merge_pending(provider).expect("merged"));
    ana.leaves[0].1 = copy(&moved[0]);
    assert_eq!((counted(&world, "commit:ana-phone"), counted(&world, "rotation-commit:ana-phone")), (1, 2));
    // The Mac, after it: the same.
    ana.with(mac, |provider, _| on_mac.process(provider, &trust, &out.commit).expect("processed"));
    let out = ana.with(mac, |provider, _| on_mac.update_certificate(provider, &moved[1]).expect("moved"));
    assert_eq!(post_commit(&mut world, &ana.caller(mac), &id, &out).0, 200);
    ana.with(mac, |provider, _| on_mac.merge_pending(provider).expect("merged"));
    ana.with(0, |provider, _| group.process(provider, &trust, &out.commit).expect("processed"));
    ana.leaves[mac].1 = copy(&moved[1]);
    assert_eq!(counted(&world, "rotation-commit:ana-mac"), 1);
    // Moved, the phone updates as usual, in the usual budget.
    let update = ana.with(0, |provider, me| group.update(provider, me).expect("updated"));
    assert_eq!(post_commit(&mut world, &phone, &id, &update).0, 200);
    assert_eq!((counted(&world, "commit:ana-phone"), counted(&world, "rotation-commit:ana-phone")), (2, 2));
}

/// Test 27, a friend's part at the worker: Ana's Mac asked to leave a
/// conversation before her new key; Bo's next commit takes the request in
/// by reference, from a leaf now under a replaced key. Refused: friends'
/// devices would refuse it too, and the group would split.
#[test]
fn test_27_a_friend_s_commit_takes_in_no_proposal_from_a_replaced_key() {
    let mut world = World::new();
    let (conversation, generation) = friends(&mut world, "ana", "bo");
    let mut ana = set_up(&mut world, "ana");
    let mac = ana.add(device("ana", "ana-mac"));
    assert_eq!(call(&mut world, &ana.caller(0), "PUT", "me/keys", ana.put(false)).0, 200);
    assert_eq!(upload(&mut world, &ana.caller(mac), ana.key_packages(mac, 1), None).0, 200);
    let bo = set_up(&mut world, "bo");
    assert_eq!(upload(&mut world, &bo.caller(0), bo.key_packages(0, 1), None).0, 200);
    let trust = pins(&[&ana, &bo]);
    let says = Says::Conversation { people: ["ana".into(), "bo".into()] };
    let id = [10; 16];
    let body = json!({"kind": "conversation", "conversation_id": conversation, "contact_generation": generation});
    let mut chat = start(&mut world, &ana, 0, says.clone(), id, body);
    let packages = [claim_one(&mut world, &ana.caller(0), "bo", "bo-phone"), claim_one(&mut world, &ana.caller(0), "ana", "ana-mac")];
    let out = ana.with(0, |provider, me| chat.add(provider, me, &trust, &packages).expect("added"));
    assert_eq!(post_commit(&mut world, &ana.caller(0), &id, &out).0, 200);
    ana.with(0, |provider, _| chat.merge_pending(provider).expect("merged"));
    let welcome = out.welcome.as_ref().expect("a Welcome");
    let mut on_mac = ana.with(mac, |provider, _| Mls::join(provider, &trust, says.clone(), &id, welcome).expect("joined"));
    let mut on_bo = bo.with(0, |provider, _| Mls::join(provider, &trust, says.clone(), &id, welcome).expect("joined"));

    // The Mac asks to leave, under the key Ana has now: taken.
    let leave = ana.with(mac, |provider, me| on_mac.leave(provider, me).expect("a proposal"));
    let proposals = format!("groups/{}/proposals", b64(&id));
    assert_eq!(call(&mut world, &ana.caller(mac), "POST", &proposals, json!({"proposal": env(leave.clone())})).0, 200);
    bo.with(0, |provider, _| on_bo.process(provider, &trust, &leave).expect("kept"));

    // Ana's new key; Bo, not yet told, commits with the request in it.
    rotate(&mut world, &mut ana, &[0, mac]);
    let commit = bo.with(0, |provider, me| on_bo.update(provider, me).expect("updated"));
    let (status, refused) = post_commit(&mut world, &bo.caller(0), &id, &commit);
    assert_eq!((status, &refused["error"]), (403, &json!("replaced_key")), "{refused}");
}
