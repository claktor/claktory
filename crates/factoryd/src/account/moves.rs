//! Moving a Claktory to another computer with the account ("Moves", API.md,
//! "Moves with an account"). No code and no digits: the other computer
//! (`factoryd receive --email`) asks the service to receive one of the
//! account's Claktories; the person approves it on a device connected to the
//! Claktory that moves, in Settings → Move, with the device's own approval
//! signed with its account key; that Claktory (the source) checks the
//! approval against its own devices, takes the move, and dials the other
//! computer, which lets in only the endpoint the source named.
//!
//! - The source never moves on the service's word alone: only on an approval
//!   that checks with the key of a device of its own (`remote::approver`),
//!   for this account, this Claktory, the very request and the other
//!   computer's receive key.
//! - A move request is never pushed to the person (no card, bell or
//!   notification, Fugo's A): they find it in Settings → Move, which asks
//!   this Claktory for its word (ProveAllowedDevice); from then on it asks
//!   the service for approved moves every few seconds, for a while.

use std::sync::Arc;
use std::time::{Duration, Instant};

use claktory_account::protocol::{
    self as api, AuthorizePoll, AuthorizeStart, AuthorizeStarted, ClaktoryMoves, Ephemeral, MoveAuthorized, MoveTake, WaitingMove,
};
use ed25519_dalek::{SigningKey, VerifyingKey};

use super::{Failure, State, anchor, computer_name, configured_service, now_s, platform};
use crate::config::AccountConfig;
use crate::factory::Factory;
use crate::home::Home;

/// How often approved moves are asked for while a device of the person's is
/// connected, and how often for a while after one asked for this Claktory's
/// word (it may be approving a move from here).
const EVERY: Duration = Duration::from_secs(60);
const SOON_EVERY: Duration = Duration::from_secs(3);
const SOON_FOR: Duration = Duration::from_secs(180);
/// A move request lasts this long at most (the service's own `expires_in`
/// is what counts; this only bounds a wrong one).
const LONGEST: i64 = 15 * 60;

/// The longest an approval may last (its `exp` less its `iat`): a move
/// request's ten minutes, which an approval never outlasts.
const APPROVAL_LONGEST: i64 = 10 * 60;
/// How many moves taken here are remembered, so an approval is never used
/// twice (`moves-taken.json`).
const TAKEN_KEPT: usize = 100;

// ---- The other computer: `factoryd receive --email` ----

/// A move's destination waiting for the person's approval.
pub(crate) struct Receiving {
    state: State,
    service: String,
    verifying: VerifyingKey,
    pub(crate) request_id: String,
    /// When it runs out, in ms since 1970.
    pub(crate) expires_ms: i64,
    /// How often to ask whether the source took it.
    pub(crate) interval: Duration,
}

/// This computer's code for a move to it: the receive key's fingerprint
/// (`key_fingerprint` of its base64url), as the person's device shows it in
/// Settings → Move, worked out there from the endpoint it signs: upper-case
/// hex in four groups of four ("C77E F7E5 EBF2 A13B"). The one fact about
/// this computer an account service can't fake.
pub(crate) fn computer_code(key: &iroh::SecretKey) -> String {
    let fingerprint = api::key_fingerprint(&api::b64(key.public().as_bytes())).to_uppercase();
    fingerprint.as_bytes().chunks(4).map(|group| String::from_utf8_lossy(group).into_owned()).collect::<Vec<_>>().join(" ")
}

/// What the person reads while the other computer waits for them.
pub(crate) const APPROVE_THERE: &str =
    "On a device connected to the Claktory you're moving, open Settings → Move and approve moving it here.";

/// Asks the account service of `email` (Claktory's own, or `service`) to
/// receive one of the account's Claktories here, at the receive endpoint
/// whose key is `key` and which is reachable through `relays`. Why not, in
/// a sentence for the person.
pub(crate) async fn receive_start(
    home: &Home,
    service: Option<&str>,
    email: &str,
    key: &iroh::SecretKey,
    relays: Vec<String>,
) -> Result<Receiving, String> {
    let Some(email) = api::normalize_email(email) else {
        return Err("That isn't an email address. Give the one you sign in to Claktory with, like you@example.com.".to_owned());
    };
    let config = AccountConfig { service: service.unwrap_or_default().to_owned(), ..AccountConfig::default() };
    let service = configured_service(&config);
    if service.is_empty() {
        return Err("That isn't an account service's address.".to_owned());
    }
    let state = State::new(home, false);
    let known = state.well_known(&service).await.map_err(|failure| unreachable(&service, &failure))?;
    if matches!(anchor::vouch(&service, &known), anchor::Vouch::Refused(_)) {
        return Err(format!(
            "{}'s key isn't vouched for right now, so nothing can move here through it. Try again later.",
            service_name(&service)
        ));
    }
    let Some(verifying) = api::verifying_key(&known.service_key) else {
        return Err(unreachable(&service, &Failure::Unreachable("a bad service key".to_owned())));
    };
    let signing = SigningKey::from_bytes(&key.to_bytes());
    let start = AuthorizeStart {
        email,
        claktory_id: String::new(),
        backend_key: api::b64(signing.verifying_key().to_bytes()),
        name: computer_name(),
        version: crate::update::VERSION.to_owned(),
        platform: platform(),
        kind: api::KIND_MOVE.to_owned(),
        receive: Some(api::MoveReceive { endpoint: key.public().to_string(), relays }),
    };
    let asked = state
        .call::<AuthorizeStarted>(&service, Some(&verifying), "POST", "/v1/claktories/authorize/start", &start, Some(("", &signing)))
        .await;
    let started = asked.map_err(|failure| match failure {
        Failure::Refused(_, body) => match body.error.as_str() {
            "no_device_to_approve" => "None of your devices can approve a move yet. On your phone or Mac, sign in to your Claktory account with your email and connect it to the Claktory you're moving, then try again.".to_owned(),
            "two_at_once" => TWO_AT_ONCE.to_owned(),
            _ if body.message.is_empty() => format!("{} refused it ({}).", service_name(&service), body.error),
            _ => body.message,
        },
        failure => unreachable(&service, &failure),
    })?;
    Ok(Receiving {
        state,
        service,
        verifying,
        request_id: started.request_id,
        expires_ms: crate::transcript::now_ms() + started.expires_in.clamp(1, LONGEST) * 1000,
        interval: Duration::from_secs(started.interval.clamp(1, 60).unsigned_abs()),
    })
}

const TWO_AT_ONCE: &str = "Two computers asked your account at the same time, so neither can go ahead. Run factoryd receive again.";

impl Receiving {
    /// Whether the person approved the move and its source took it: what it
    /// said then (the only endpoint to let in), None while it waits, or why
    /// it ended.
    pub(crate) async fn poll(&self, key: &iroh::SecretKey) -> Result<Option<MoveAuthorized>, String> {
        if crate::transcript::now_ms() >= self.expires_ms {
            return Err(NOT_IN_TIME.to_owned());
        }
        let signing = SigningKey::from_bytes(&key.to_bytes());
        let asked = AuthorizePoll { request_id: self.request_id.clone(), eph_pub: Ephemeral::new().public_b64() };
        let path = "/v1/claktories/authorize/poll";
        match self.state.call::<MoveAuthorized>(&self.service, Some(&self.verifying), "POST", path, &asked, Some(("", &signing))).await {
            Ok(authorized) if authorized.kind == api::KIND_MOVE && is_endpoint(&authorized.source_key) => Ok(Some(authorized)),
            Ok(_) => Err(format!("{} answered with something that isn't a move.", service_name(&self.service))),
            Err(Failure::Refused(_, body)) => match body.error.as_str() {
                "authorization_pending" | "rate_limited" => Ok(None),
                "access_denied" => Err("Moving here was declined on one of your devices.".to_owned()),
                "two_at_once" => Err(TWO_AT_ONCE.to_owned()),
                "expired_token" => Err(NOT_IN_TIME.to_owned()),
                _ if body.message.is_empty() => Err(format!("{} refused it ({}).", service_name(&self.service), body.error)),
                _ => Err(body.message),
            },
            // Not reached: asked again until it runs out.
            Err(failure) => {
                tracing::info!(%failure, "couldn't ask whether the move here was approved; asking again");
                Ok(None)
            }
        }
    }
}

const NOT_IN_TIME: &str = "Moving here wasn't approved in time. Run factoryd receive again.";

fn unreachable(service: &str, failure: &Failure) -> String {
    format!("Can't reach {} ({failure}). Check this computer's internet connection and try again.", service_name(service))
}

fn service_name(service: &str) -> String {
    super::service_name(service)
}

/// An iroh endpoint id, as the service writes one: 64 hex digits.
fn is_endpoint(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit()) && text.parse::<iroh::EndpointId>().is_ok()
}

// ---- The source: this Claktory, moving ----

/// A device of the person's asked for this Claktory's word (ProveAllowedDevice),
/// maybe to approve a move from here: approved moves are asked for every few
/// seconds for a while.
pub(crate) fn look_soon(factory: &Factory) {
    factory.account.lock().moves_soon_until = Some(Instant::now() + SOON_FOR);
    factory.account.wake.notify_one();
}

/// The check-in's `moves_waiting`: asked for at once when there are any.
pub(super) fn heard_waiting(factory: &Factory, waiting: &[String]) {
    if !waiting.is_empty() {
        factory.account.lock().moves_asked = None;
        factory.account.wake.notify_one();
    }
}

/// How long until approved moves are asked for again, if they are.
pub(super) fn next_due(inner: &super::Inner, clients: bool) -> Option<Duration> {
    let soon = inner.moves_soon_until.is_some_and(|until| until > Instant::now());
    let every = if soon {
        SOON_EVERY
    } else if clients {
        EVERY
    } else {
        return None;
    };
    Some(inner.moves_asked.map_or(Duration::ZERO, |asked| every.saturating_sub(asked.elapsed())))
}

/// Asks the service for moves the person approved with this Claktory as
/// their source, when due, while signed in; the first whose approval checks
/// out (`checked`) is taken and starts (`move_run::start_approved`). Called
/// with `busy` held.
pub(super) async fn take_approved(factory: &Arc<Factory>) {
    let account = &factory.account;
    let (service, service_key, token) = {
        let mut inner = account.lock();
        let due = next_due(&inner, factory.clients() > 0).is_some_and(|due| due.is_zero());
        if !due || !inner.saved.signed_in() || inner.saved.revoked {
            return;
        }
        inner.moves_asked = Some(Instant::now());
        (inner.saved.service.clone(), inner.saved.service_key.clone(), inner.saved.machine_token.clone())
    };
    let (Some(verifying), Ok((_, key))) = (api::verifying_key(&service_key), account.identity()) else { return };
    let answer = account.send(&service, Some(&verifying), "GET", "/v1/claktories/moves", Vec::new(), Some((&token, &key))).await;
    let moves = match answer
        .and_then(|bytes| serde_json::from_slice::<ClaktoryMoves>(&bytes).map_err(|error| Failure::Unreachable(error.to_string())))
    {
        Ok(moves) => moves.moves,
        Err(failure) => {
            tracing::debug!(%failure, "couldn't ask for approved moves");
            return;
        }
    };
    for waiting in moves {
        if account.lock().moves_turned_down.contains(&waiting.request_id) {
            continue;
        }
        let approved = match checked(factory, &waiting) {
            Ok(approved) => approved,
            Err(why) => {
                tracing::warn!(request = %waiting.request_id, why, "turned down an approved move");
                turn_down(factory, &waiting.request_id);
                continue;
            }
        };
        if let Err(why) = crate::move_run::may_start(factory, &approved.code()) {
            tracing::warn!(request = %waiting.request_id, why, "an approved move can't start");
            turn_down(factory, &waiting.request_id);
            crate::move_run::failed_before_start(factory, &approved, &why);
            continue;
        }
        let endpoint = match crate::move_source::endpoint_id(&factory.home) {
            Ok(endpoint) => endpoint,
            Err(why) => {
                tracing::warn!(why, "couldn't make the move's key");
                return;
            }
        };
        let path = format!("/v1/claktories/moves/{}/take", waiting.request_id);
        match account.signed::<serde_json::Value>(&path, &MoveTake { endpoint }).await {
            Ok(_) => {
                note_taken(&factory.home, &waiting.request_id);
                tracing::info!(request = %waiting.request_id, target = %approved.machine, device = %approved.approver_id, "took an approved move");
                if let Err(why) = crate::move_run::start_approved(factory, approved.clone()) {
                    crate::move_run::failed_before_start(factory, &approved, &why);
                }
            }
            Err(failure) => {
                tracing::warn!(request = %waiting.request_id, %failure, "couldn't take an approved move");
                turn_down(factory, &waiting.request_id);
            }
        }
        // One move at a time.
        return;
    }
}

fn turn_down(factory: &Factory, request_id: &str) {
    let mut inner = factory.account.lock();
    inner.moves_turned_down.push(request_id.to_owned());
    let excess = inner.moves_turned_down.len().saturating_sub(100);
    inner.moves_turned_down.drain(..excess);
}

/// The moves this Claktory took, by request id: kept in
/// `<home>/account/moves-taken.json`, the last `TAKEN_KEPT`.
fn taken_path(home: &Home) -> std::path::PathBuf {
    home.root().join("account/moves-taken.json")
}

fn taken(home: &Home) -> Vec<String> {
    std::fs::read(taken_path(home)).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

fn note_taken(home: &Home, request_id: &str) {
    let mut ids = taken(home);
    ids.push(request_id.to_owned());
    let excess = ids.len().saturating_sub(TAKEN_KEPT);
    ids.drain(..excess);
    let written =
        serde_json::to_vec(&ids).map_err(std::io::Error::other).and_then(|json| crate::remote::write_private(&taken_path(home), &json));
    if let Err(error) = written {
        tracing::warn!(%error, "couldn't note a move taken");
    }
}

/// A move the person approved, as this Claktory checked it.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Approved {
    pub request_id: String,
    /// The other computer's receive endpoint (64 hex), and its relays.
    pub endpoint: String,
    pub relays: Vec<String>,
    /// What it calls itself, as the service cleaned it.
    pub machine: String,
    pub version: String,
    /// "linux-x86_64".
    pub platform: String,
    /// The device here that approved, its name, and the account key that
    /// signed the approval.
    pub approver_id: String,
    pub approver_name: String,
    pub approval_key: String,
}

impl Approved {
    /// The receive code this move would have had: where to dial, with no
    /// secret (the other computer lets in only this Claktory's endpoint).
    pub(crate) fn code(&self) -> crate::move_wire::ReceiveCode {
        let (os, arch) = self.platform.split_once('-').unwrap_or((self.platform.as_str(), ""));
        crate::move_wire::ReceiveCode {
            endpoint: self.endpoint.clone(),
            secret: String::new(),
            machine: if self.machine.is_empty() { "the other computer".to_owned() } else { self.machine.clone() },
            version: self.version.clone(),
            os: os.to_owned(),
            arch: arch.to_owned(),
            relays: self.relays.clone(),
            addresses: Vec::new(),
        }
    }
}

/// Whether `waiting` may move this Claktory: its approval is the device's
/// own (`verify_move_approval`: signed by the key it names, version 1, a
/// move, in time), for this account, this Claktory, this very request, the
/// other computer's receive key and the fingerprint its card showed, and
/// that key is a device of this Claktory's that may approve
/// (`remote::approver`), lasting no longer than a request and never taken
/// here before. The service's word is never enough.
fn checked(factory: &Factory, waiting: &WaitingMove) -> Result<Approved, String> {
    let (account_id, claktory_id, offset) = {
        let inner = factory.account.lock();
        (inner.saved.account_id.clone(), inner.saved.claktory_id.clone(), inner.clock_offset)
    };
    // This computer's clock, put right by the service's by no more than
    // the skew any signed time may have: the service can't make an old
    // approval current.
    check(&factory.home, waiting, &account_id, &claktory_id, now_s() + offset.clamp(-api::MAX_SKEW, api::MAX_SKEW))
}

pub(crate) fn check(home: &Home, waiting: &WaitingMove, account_id: &str, claktory_id: &str, now: i64) -> Result<Approved, String> {
    let approval = api::verify_move_approval(&waiting.approval, now).ok_or("its approval isn't a device's, or ran out")?;
    if account_id.is_empty() || approval.account_id != account_id {
        return Err("its approval is for another account".to_owned());
    }
    if claktory_id.is_empty() || approval.source_claktory_id != claktory_id {
        return Err("its approval is for another Claktory".to_owned());
    }
    if approval.request_id != waiting.request_id {
        return Err("its approval is for another request".to_owned());
    }
    if approval.exp - approval.iat > APPROVAL_LONGEST {
        return Err("its approval lasts longer than a move request".to_owned());
    }
    if taken(home).contains(&approval.request_id) {
        return Err("it was taken here before".to_owned());
    }
    if !is_endpoint(&waiting.receive.endpoint) || approval.endpoint != waiting.receive.endpoint {
        return Err("its approval is for another computer".to_owned());
    }
    let receive_key = crate::encoding::unhex(&waiting.receive.endpoint).map(api::b64).unwrap_or_default();
    let fingerprint = api::key_fingerprint(&receive_key);
    if approval.fingerprint != fingerprint || waiting.fingerprint != fingerprint {
        return Err("its approval names another key".to_owned());
    }
    let Some(device) = crate::remote::approver(home, &approval.device_key, account_id) else {
        return Err("the device that approved it isn't one of this Claktory's".to_owned());
    };
    Ok(Approved {
        request_id: waiting.request_id.clone(),
        endpoint: waiting.receive.endpoint.clone(),
        relays: waiting.receive.relays.iter().filter(|relay| relay.starts_with("https://")).take(4).cloned().collect(),
        machine: waiting.says.name.clone(),
        version: waiting.says.version.clone(),
        platform: waiting.says.platform.clone(),
        approver_id: device.id,
        approver_name: device.name,
        approval_key: approval.device_key,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::Device;

    const ACCOUNT: &str = "acct";
    const CLAKTORY: &str = "studio-claktory";

    fn home_with(devices: &[Device]) -> Home {
        let root = std::env::temp_dir().join(format!("factoryd-move-approval-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = Home::new(root);
        std::fs::create_dir_all(home.root().join("remote")).expect("remote");
        std::fs::write(home.root().join("remote/devices.json"), serde_json::to_vec(devices).expect("json")).expect("devices");
        home
    }

    fn device_key(seed: u8) -> (SigningKey, String) {
        let key = SigningKey::from_bytes(&[seed; 32]);
        let b64 = api::b64(key.verifying_key().to_bytes());
        (key, b64)
    }

    fn waiting(signer: &SigningKey, change: impl FnOnce(&mut api::MoveApproval)) -> WaitingMove {
        let receive = iroh::SecretKey::from_bytes(&[9; 32]);
        let endpoint = receive.public().to_string();
        let fingerprint = api::key_fingerprint(&api::b64(receive.public().as_bytes()));
        let mut approval = api::MoveApproval {
            v: 1,
            kind: api::KIND_MOVE.into(),
            request_id: "req-1".into(),
            account_id: ACCOUNT.into(),
            source_claktory_id: CLAKTORY.into(),
            endpoint: endpoint.clone(),
            fingerprint: fingerprint.clone(),
            device_key: api::b64(signer.verifying_key().to_bytes()),
            iat: 1000,
            exp: 1600,
        };
        change(&mut approval);
        WaitingMove {
            request_id: "req-1".into(),
            receive: api::MoveReceive { endpoint, relays: vec!["https://relay.example/".into()] },
            seen: api::AuthorizationSeen::default(),
            says: api::AuthorizationSays { name: "cloud-box".into(), platform: "linux-x86_64".into(), version: "v0.2.0".into() },
            fingerprint,
            approval: api::sign_move_approval(signer, &approval),
            expires_in: 500,
        }
    }

    /// An approval that isn't a device's own, from a device
    /// not in this Claktory's list or one removed, for another Claktory, or
    /// for another computer, starts nothing, nor one lasting longer than a
    /// request or taken here before; one from a confirmed device of its own
    /// does, naming that device. The code the person compares: the app's
    /// own vector, worked out apart from both.
    #[test]
    fn this_computers_code_is_the_receive_keys_fingerprint_in_groups_of_four() {
        let key = iroh::SecretKey::from_bytes(&[7; 32]);
        let code = computer_code(&key);
        assert_eq!(code.len(), 19);
        assert_eq!(code.replace(' ', ""), api::key_fingerprint(&api::b64(key.public().as_bytes())).to_uppercase());
        assert!(code.split(' ').all(|group| group.len() == 4 && group.chars().all(|c| c.is_ascii_digit() || ('A'..='F').contains(&c))));
        // The app's vector: an endpoint of ab12 sixteen times.
        let fingerprint = api::key_fingerprint(&api::b64([0xab, 0x12].repeat(16)));
        assert_eq!(fingerprint, "c77ef7e5ebf2a13b");
    }

    #[test]
    fn only_a_devices_own_approval_from_one_of_this_claktorys_devices_moves_it() {
        let (phone_key, phone) = device_key(1);
        let (stranger_key, _) = device_key(2);
        let (pending_key, pending) = device_key(3);
        let devices = [
            Device {
                id: "phone".into(),
                name: "Matt's iPhone".into(),
                endpoint_id: "p".into(),
                confirmed_as: ACCOUNT.into(),
                account_key: phone.clone(),
                ..Device::default()
            },
            Device { id: "waiting".into(), pending_endpoint_id: "w".into(), account_key: pending, ..Device::default() },
        ];
        let home = home_with(&devices);
        let at = 1200;

        let approved = check(&home, &waiting(&phone_key, |_| {}), ACCOUNT, CLAKTORY, at).expect("the phone's approval");
        assert_eq!((approved.approver_id.as_str(), approved.approver_name.as_str()), ("phone", "Matt's iPhone"));
        assert_eq!((approved.approval_key.as_str(), approved.machine.as_str()), (phone.as_str(), "cloud-box"));
        let code = approved.code();
        assert_eq!((code.os.as_str(), code.arch.as_str(), code.secret.as_str()), ("linux", "x86_64", ""));

        // Not in time, or signed by another key than it names.
        assert!(check(&home, &waiting(&phone_key, |_| {}), ACCOUNT, CLAKTORY, 1601).is_err(), "ran out");
        let mut forged = waiting(&stranger_key, |approval| approval.device_key = phone.clone());
        assert!(check(&home, &forged, ACCOUNT, CLAKTORY, at).is_err(), "signed by another key");
        // A device that isn't this Claktory's, or waits to be allowed.
        assert!(check(&home, &waiting(&stranger_key, |_| {}), ACCOUNT, CLAKTORY, at).is_err(), "a stranger");
        assert!(check(&home, &waiting(&pending_key, |_| {}), ACCOUNT, CLAKTORY, at).is_err(), "a device waiting");
        // Another Claktory, account or request.
        assert!(
            check(&home, &waiting(&phone_key, |approval| approval.source_claktory_id = "other".into()), ACCOUNT, CLAKTORY, at).is_err()
        );
        assert!(check(&home, &waiting(&phone_key, |approval| approval.account_id = "other".into()), ACCOUNT, CLAKTORY, at).is_err());
        assert!(check(&home, &waiting(&phone_key, |approval| approval.request_id = "req-2".into()), ACCOUNT, CLAKTORY, at).is_err());
        // Another computer than the one the service says to dial.
        let elsewhere = iroh::SecretKey::from_bytes(&[8; 32]).public().to_string();
        assert!(check(&home, &waiting(&phone_key, |approval| approval.endpoint = elsewhere.clone()), ACCOUNT, CLAKTORY, at).is_err());
        forged = waiting(&phone_key, |_| {});
        forged.receive.endpoint = elsewhere;
        assert!(check(&home, &forged, ACCOUNT, CLAKTORY, at).is_err(), "the service names another computer");
        // Lasting longer than a request.
        assert!(check(&home, &waiting(&phone_key, |approval| approval.exp = 1000 + 601), ACCOUNT, CLAKTORY, at).is_err(), "too long");
        // Taken here before: never again, whatever the service says.
        std::fs::create_dir_all(home.root().join("account")).expect("account");
        note_taken(&home, "req-1");
        assert!(check(&home, &waiting(&phone_key, |_| {}), ACCOUNT, CLAKTORY, at).is_err(), "taken before");
        std::fs::remove_file(taken_path(&home)).expect("forgotten");

        // This computer's own app: by the key its assertion proved for this
        // account, never one kept from another account.
        let (mac_key, mac) = device_key(4);
        let local = |account: &str| Device {
            id: crate::remote::LOCAL_DEVICE.into(),
            name: "Mac mini".into(),
            local: true,
            account_key: mac.clone(),
            account_key_for: account.into(),
            ..Device::default()
        };
        let mut with_mac = devices.to_vec();
        with_mac.push(local(ACCOUNT));
        let home = home_with(&with_mac);
        let approved = check(&home, &waiting(&mac_key, |_| {}), ACCOUNT, CLAKTORY, at).expect("this computer's app");
        assert_eq!(approved.approver_id, crate::remote::LOCAL_DEVICE);
        for account in ["another", ""] {
            with_mac.pop();
            with_mac.push(local(account));
            let home = home_with(&with_mac);
            assert!(check(&home, &waiting(&mac_key, |_| {}), ACCOUNT, CLAKTORY, at).is_err(), "proven for {account:?}");
        }
        let home = home_with(&devices);

        // Its key removed from the account (Lock out): no longer.
        std::fs::write(
            home.root().join("remote/removed-keys.json"),
            serde_json::to_vec(&[crate::remote::RemovedKey { account_key: phone, removed_ms: 1, why: "lock_out".into() }]).expect("json"),
        )
        .expect("removed");
        assert!(check(&home, &waiting(&phone_key, |_| {}), ACCOUNT, CLAKTORY, at).is_err(), "a removed device");
        let _ = std::fs::remove_dir_all(home.root());
    }
}
