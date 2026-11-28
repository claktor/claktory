//! Moving a Claktory to another computer with the account, no code and no
//! digits (API.md, "Moves with an account"): the destination asks to
//! receive one, the person approves it in Settings → Move with the
//! device's own signed approval, the source fetches it and takes it once,
//! and the destination hears the one endpoint to let in.

use claktory_account::protocol::*;
use serde_json::{Value, json};

use crate::authorize::{EMAIL, decide, key_signed, matts, poll, shown, word};
use crate::support::*;

/// A destination's receive key as an endpoint id.
fn endpoint(computer: &Claktory) -> String {
    computer.key.verifying_key().to_bytes().iter().map(|byte| format!("{byte:02x}")).collect()
}

fn fingerprint(computer: &Claktory) -> String {
    key_fingerprint(&b64(computer.key.verifying_key().to_bytes()))
}

/// `factoryd receive --email`'s start, from `computer`'s receive key.
async fn ask_move(instance: &Instance, computer: &Claktory, change: impl FnOnce(&mut AuthorizeStart)) -> Answer {
    let mut body = AuthorizeStart {
        email: EMAIL.to_owned(),
        claktory_id: String::new(),
        backend_key: b64(computer.key.verifying_key().to_bytes()),
        name: "cloud-box".to_owned(),
        version: "v0.2.0".to_owned(),
        platform: "linux-x86_64".to_owned(),
        kind: KIND_MOVE.to_owned(),
        receive: Some(MoveReceive {
            endpoint: endpoint(computer),
            relays: vec!["https://relay.example/".into(), "http://plain.example/".into()],
        }),
    };
    change(&mut body);
    key_signed(instance, computer, "/v1/claktories/authorize/start", &body).await
}

/// `device`'s own approval of moving `source_id` to `computer`.
fn approval(
    instance: &Instance,
    device: &Device,
    account: &str,
    source_id: &str,
    request_id: &str,
    computer: &Claktory,
    change: impl FnOnce(&mut MoveApproval),
) -> String {
    let mut approval = MoveApproval {
        v: 1,
        kind: KIND_MOVE.to_owned(),
        request_id: request_id.to_owned(),
        account_id: account.to_owned(),
        source_claktory_id: source_id.to_owned(),
        endpoint: endpoint(computer),
        fingerprint: fingerprint(computer),
        device_key: device.account_key(),
        iat: instance.now(),
        exp: instance.now() + 300,
    };
    change(&mut approval);
    sign_move_approval(&device.key, &approval)
}

async fn moves_shown(instance: &Instance, device: &Device) -> Vec<DeviceAuthorization> {
    instance.advance(1);
    let answer = instance.device_signed(device, "GET", "/v1/devices/moves", None, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<DeviceMoves>().moves
}

async fn approved_moves(instance: &Instance, claktory: &Claktory) -> Vec<WaitingMove> {
    instance.advance(1);
    let answer = instance.signed(claktory, "GET", "/v1/claktories/moves", &json!({}), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<ClaktoryMoves>().moves
}

async fn take(instance: &Instance, claktory: &Claktory, id: &str, endpoint: &str) -> Answer {
    instance.advance(1);
    instance.signed(claktory, "POST", &format!("/v1/claktories/moves/{id}/take"), &MoveTake { endpoint: endpoint.to_owned() }, None).await
}

async fn moves_waiting(instance: &Instance, claktory: &Claktory) -> Vec<String> {
    instance.advance(1);
    let check_in = CheckIn { version: "v0.2.0".into(), platform: "macos-arm64".into(), ..Default::default() };
    let answer = instance.signed(claktory, "POST", "/v1/check-in", &check_in, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    answer.json::<CheckedIn>().moves_waiting
}

fn refused(answer: &Answer) {
    assert_eq!((answer.status, answer.error()), (403, "not_allowed_to_approve".to_owned()), "{}", answer.text());
}

#[tokio::test]
async fn a_move_is_approved_with_the_devices_own_signature_taken_once_and_names_only_its_source() {
    let (instance, studio, phone, account) = matts().await;
    let target = Claktory::new("203.0.113.9");
    let mails = instance.mail.sent().len();
    let answer = ask_move(&instance, &target, |_| {}).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let id = answer.json::<AuthorizeStarted>().request_id;
    // The heads-up says "move" plainly, and approves nothing.
    let sent = instance.mail.sent();
    assert_eq!(sent.len(), mails + 1);
    let heads_up = sent.last().expect("a heads-up");
    assert_eq!(heads_up.subject, "A computer asked to receive one of your Claktories");
    assert!(heads_up.text.contains("Settings → Move") && !heads_up.html.contains("href"), "{}", heads_up.text);

    // Never pushed: no card with the bell, no id in the Claktory's
    // bell; only in Settings → Move.
    assert!(shown(&instance, &phone).await.is_empty(), "not with the joins");
    let bell = instance.signed(&studio, "GET", "/v1/claktories/authorizations", &json!({}), None).await;
    assert!(bell.json::<ClaktoryAuthorizations>().waiting.is_empty(), "no bell");
    let cards = moves_shown(&instance, &phone).await;
    assert_eq!(cards.len(), 1);
    let card = &cards[0];
    assert_eq!((card.id.as_str(), card.kind.as_str(), card.state.as_str()), (id.as_str(), "move", "waiting"));
    assert_eq!((card.endpoint.clone(), card.fingerprint.clone()), (endpoint(&target), fingerprint(&target)));
    assert_eq!((card.seen.ip.as_str(), card.says.name.as_str()), ("203.0.113.9", "cloud-box"));
    assert!(moves_waiting(&instance, &studio).await.is_empty());
    let (pending, _) = poll(&instance, &target, &id).await;
    assert_eq!((pending.status, pending.error()), (409, "authorization_pending".to_owned()));

    // Approving takes the studio's word and the phone's own approval of it,
    // for this request, this computer and the studio, within the request.
    let decision = |approval: Option<String>| -> Value {
        json!({"approve": true, "fingerprint": card.fingerprint, "allowed": word(&instance, &studio, &account, &phone, &id, None), "move_approval": approval})
    };
    refused(&decide(&instance, &phone, &id, decision(None)).await);
    let laptop = Device::new("192.0.2.81");
    let by_another = approval(&instance, &laptop, &account, &studio.id, &id, &target, |_| {});
    refused(&decide(&instance, &phone, &id, decision(Some(by_another))).await);
    let mut claimed =
        approval(&instance, &laptop, &account, &studio.id, &id, &target, |approval| approval.device_key = phone.account_key());
    refused(&decide(&instance, &phone, &id, decision(Some(claimed))).await);
    let elsewhere = Claktory::new("203.0.113.66");
    for change in [
        Box::new(|approval: &mut MoveApproval| approval.source_claktory_id = "another-claktory-id".into())
            as Box<dyn FnOnce(&mut MoveApproval)>,
        Box::new(|approval: &mut MoveApproval| approval.request_id = "another".into()),
        Box::new(|approval: &mut MoveApproval| approval.account_id = "another".into()),
        Box::new(|approval: &mut MoveApproval| approval.endpoint = endpoint(&elsewhere)),
        Box::new(|approval: &mut MoveApproval| approval.fingerprint = fingerprint(&elsewhere)),
        Box::new(|approval: &mut MoveApproval| approval.exp += 3600),
    ] {
        claimed = approval(&instance, &phone, &account, &studio.id, &id, &target, change);
        refused(&decide(&instance, &phone, &id, decision(Some(claimed))).await);
    }
    // None of that approved it.
    assert_eq!(moves_shown(&instance, &phone).await[0].state, "waiting");
    assert!(approved_moves(&instance, &studio).await.is_empty());

    let own = approval(&instance, &phone, &account, &studio.id, &id, &target, |_| {});
    let approved = decide(&instance, &phone, &id, decision(Some(own.clone()))).await;
    assert_eq!(approved.status, 200, "{}", approved.text());
    assert!(moves_shown(&instance, &phone).await.is_empty(), "no longer waiting");
    // Approved but not taken: the destination still waits.
    let (pending, _) = poll(&instance, &target, &id).await;
    assert_eq!(pending.error(), "authorization_pending");

    // The source: its check-in says so; fetching spends nothing; the
    // approval is as the phone signed it, for the source to check.
    assert_eq!(moves_waiting(&instance, &studio).await, std::slice::from_ref(&id));
    for _ in 0..2 {
        let moves = approved_moves(&instance, &studio).await;
        assert_eq!(moves.len(), 1);
        let waiting = &moves[0];
        assert_eq!((waiting.request_id.as_str(), waiting.approval.as_str()), (id.as_str(), own.as_str()));
        assert_eq!(waiting.receive, MoveReceive { endpoint: endpoint(&target), relays: vec!["https://relay.example/".into()] });
        assert_eq!((waiting.fingerprint.clone(), waiting.says.name.as_str()), (fingerprint(&target), "cloud-box"));
        assert!(waiting.expires_in > 0);
    }
    // Another Claktory, not its source, neither sees it nor takes it.
    let mut eves = Claktory::new("198.51.100.50");
    instance.sign_in(&mut eves, "eve@example.com", "Eve's Mac").await;
    assert!(approved_moves(&instance, &eves).await.is_empty());
    let source_key = endpoint(&Claktory::new("198.51.100.30"));
    assert_eq!(take(&instance, &eves, &id, &source_key).await.status, 404);
    assert_eq!(take(&instance, &studio, &id, "not-an-endpoint").await.status, 400);

    // Taken once: a second take, from any endpoint, is refused.
    let taken = take(&instance, &studio, &id, &source_key).await;
    assert_eq!(taken.status, 200, "{}", taken.text());
    let again = take(&instance, &studio, &id, &endpoint(&elsewhere)).await;
    assert_eq!((again.status, again.error()), (409, "move_taken".to_owned()));
    assert!(approved_moves(&instance, &studio).await.is_empty());
    assert!(moves_waiting(&instance, &studio).await.is_empty());

    // The destination hears the one endpoint to let in, once.
    let (answer, _) = poll(&instance, &target, &id).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    let authorized: MoveAuthorized = answer.json();
    assert_eq!(
        authorized,
        MoveAuthorized {
            kind: KIND_MOVE.into(),
            source_key: source_key.clone(),
            source_claktory_id: studio.id.clone(),
            approver: phone.account_key()
        }
    );
    let (spent, _) = poll(&instance, &target, &id).await;
    assert_eq!((spent.status, spent.error()), (410, "expired_token".to_owned()));
    assert_eq!(take(&instance, &studio, &id, &source_key).await.error(), "move_taken");
}

#[tokio::test]
async fn a_move_must_name_its_receive_key_and_no_claktory() {
    let (instance, _studio, _phone, _) = matts().await;
    let target = Claktory::new("203.0.113.9");
    let other = Claktory::new("203.0.113.66");
    for answer in [
        ask_move(&instance, &target, |body| body.receive = None).await,
        ask_move(&instance, &target, |body| body.claktory_id = "a-claktory-id-of-sixteen".into()).await,
        ask_move(&instance, &target, |body| body.receive.as_mut().expect("receive").endpoint = endpoint(&other)).await,
        ask_move(&instance, &target, |body| body.kind = "something".into()).await,
    ] {
        assert_eq!((answer.status, answer.error()), (400, "invalid_request".to_owned()), "{}", answer.text());
    }
    assert_eq!(ask_move(&instance, &target, |_| {}).await.status, 200);
}

#[tokio::test]
async fn a_move_shares_the_one_request_waiting_and_runs_out() {
    let (instance, studio, phone, account) = matts().await;
    // A join and a move at once end both.
    let server = Claktory::new("203.0.113.20");
    let join = crate::authorize::ask(&instance, &server, EMAIL, "box").await.json::<AuthorizeStarted>().request_id;
    let target = Claktory::new("203.0.113.9");
    let answer = ask_move(&instance, &target, |_| {}).await;
    assert_eq!((answer.status, answer.error()), (409, "two_at_once".to_owned()));
    let (polled, _) = poll(&instance, &server, &join).await;
    assert_eq!(polled.error(), "two_at_once");
    assert_eq!(moves_shown(&instance, &phone).await[0].state, "two_at_once", "shown for a while, as joins are");

    // Approved, then not taken within its ten minutes: gone.
    instance.advance(601);
    let id = ask_move(&instance, &target, |_| {}).await.json::<AuthorizeStarted>().request_id;
    let card = moves_shown(&instance, &phone).await.remove(0);
    let own = approval(&instance, &phone, &account, &studio.id, &id, &target, |approval| approval.exp = approval.iat + 590);
    let decision = json!({"approve": true, "fingerprint": card.fingerprint, "allowed": word(&instance, &studio, &account, &phone, &id, None), "move_approval": own});
    assert_eq!(decide(&instance, &phone, &id, decision).await.status, 200);
    instance.advance(600);
    assert!(approved_moves(&instance, &studio).await.is_empty());
    let late = take(&instance, &studio, &id, &endpoint(&Claktory::new("198.51.100.30"))).await;
    assert_eq!((late.status, late.error()), (410, "expired_token".to_owned()));
    let (polled, _) = poll(&instance, &target, &id).await;
    assert_eq!(polled.error(), "expired_token");
}
