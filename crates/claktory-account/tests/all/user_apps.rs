//! User-level apps (`/v1/apps/…`): the service authenticates each call and
//! forwards it to the user-apps worker, a process of its own reached over a
//! local socket; here the worker runs in the test, as the deploy runs it.

use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

use claktory_account::protocol::*;
use claktory_account::server::{Config, UserApps};
use claktory_user_apps::worker::{self, Worker};
use serde_json::{Value, json};

use crate::support::*;
use crate::web::{field, post_form, web_sign_in};

/// This process's user, as the worker checks its peers.
fn uid(dir: &Path) -> u32 {
    std::fs::metadata(dir).expect("the folder").uid()
}

/// A worker listening at `dir/apps.sock`, answering only `root_uid`.
fn worker(dir: &Path, root_uid: u32) -> PathBuf {
    let socket = dir.join("apps.sock");
    let listener = tokio::net::UnixListener::bind(&socket).expect("a socket");
    let keys = claktory_user_apps::dms::keys::Keys::from_bytes([7; 32]);
    let worker = Worker::open(&dir.join("apps.db"), keys).expect("the worker");
    tokio::spawn(worker::serve(listener, worker, root_uid));
    socket
}

/// A service whose worker listens at `socket` as `uid`.
async fn with_apps(socket: Option<PathBuf>, uid: u32) -> Instance {
    let user_apps = socket.map(|socket| UserApps { socket, uid });
    start_with(Config { trust_proxy: true, user_apps, ..Config::default() }).await
}

/// An MLS application message for `group` at `epoch`, framed as a device
/// would send it (the worker never reads inside), as an envelope.
fn sealed(group: &[u8], epoch: u64, text: &str) -> Value {
    use base64::Engine as _;
    fn vector(out: &mut Vec<u8>, bytes: &[u8]) {
        match bytes.len() {
            n if n < 64 => out.push(n as u8),
            n => out.extend_from_slice(&((n as u16) | 0x4000).to_be_bytes()),
        }
        out.extend_from_slice(bytes);
    }
    let mut bytes = vec![0, 1, 0, 2];
    vector(&mut bytes, group);
    bytes.extend_from_slice(&epoch.to_be_bytes());
    bytes.push(1);
    vector(&mut bytes, b"");
    vector(&mut bytes, &[0; 16]);
    vector(&mut bytes, text.as_bytes());
    json!({"format": 2, "bytes": base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)})
}

/// A backend-signed GET, with no body.
async fn get(instance: &Instance, claktory: &Claktory, path: &str) -> Answer {
    // A second apart, so the same call twice isn't taken for a replay.
    instance.advance(1);
    let time = instance.now();
    let headers = [
        ("authorization", format!("Bearer {}", claktory.token)),
        (TIME_HEADER, time.to_string()),
        (SIGNATURE_HEADER, sign_request(&claktory.key, "GET", path, time, b"")),
        ("x-forwarded-for", claktory.ip.clone()),
    ];
    instance.send("GET", path, Vec::new(), &headers).await
}

#[tokio::test]
async fn the_worker_hears_who_the_service_verified_and_nothing_else() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    let signed_in = instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let mut removed = Claktory::new("198.51.100.31");
    instance.sign_in(&mut removed, "matt@example.com", "Old Mac").await;
    rusqlite::Connection::open(&instance.db)
        .expect("the database")
        .execute("UPDATE claktories SET revoked_at = 1 WHERE id = ?1", [&removed.id])
        .expect("removed");

    // A backend, for one of its agents: the agent's claims come out of the
    // query, which its signature covers.
    let answer = get(&instance, &studio, "/v1/apps/whoami?agent=june&incarnation=inc-1").await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(
        answer.value(),
        json!({"kind": "backend", "account_id": signed_in.account_id, "claktory_id": studio.id, "epoch": studio.epoch,
               "agent": {"id": "june", "incarnation": "inc-1"}})
    );
    // A device: its account's Claktories signed in now, with their epochs.
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await;
    let answer = instance.device_signed(&phone, "GET", "/v1/apps/whoami", None, None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(answer.value()["registrations"], json!([{"claktory_id": studio.id, "epoch": studio.epoch}]));

    // Nobody the service doesn't know reaches the worker.
    let unsigned = instance.send("GET", "/v1/apps/whoami", Vec::new(), &[]).await;
    assert_eq!((unsigned.status, unsigned.error()), (401, "unauthorized".to_owned()));
    let gone = get(&instance, &removed, "/v1/apps/whoami").await;
    assert_eq!(gone.error(), "revoked");
    // Another agent named than the one signed: the signature fails.
    let time = instance.now();
    let signature = sign_request(&studio.key, "GET", "/v1/apps/whoami?agent=june&incarnation=inc-1", time, b"");
    let headers = [("authorization", format!("Bearer {}", studio.token)), (TIME_HEADER, time.to_string()), (SIGNATURE_HEADER, signature)];
    let swapped = instance.send("GET", "/v1/apps/whoami?agent=root&incarnation=inc-1", Vec::new(), &headers).await;
    assert_eq!(swapped.status, 401);
    for bad in ["/v1/apps/whoami?agent=june", "/v1/apps/whoami?agent=ju%20ne&incarnation=x"] {
        assert_eq!(get(&instance, &studio, bad).await.status, 400, "{bad}");
    }
    let device_for_agent = instance.device_signed(&phone, "GET", "/v1/apps/whoami?agent=june&incarnation=x", None, None).await;
    assert_eq!(device_for_agent.status, 400, "a device acts for no agent");
    // What no app answers is refused there, not here.
    assert_eq!(get(&instance, &studio, "/v1/apps/nothing").await.error(), "not_found");
}

#[tokio::test]
async fn grants_are_made_by_a_claktory_seen_and_revoked_by_the_person_and_die_with_the_registration() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await;

    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read"],
                       "resources": [{"kind": "conversation", "id": "c1"}]});
    let made = instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await;
    assert_eq!(made.status, 200, "{}", made.text());
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    // A device can't make one; nor can a grant ask for what the app hasn't.
    let refused = instance.device_signed(&phone, "PUT", "/v1/apps/grants", Some(&grant), None).await;
    assert_eq!(refused.status, 400);
    let mut wide = grant.clone();
    wide["scopes"] = json!(["dm.everything"]);
    assert_eq!(instance.signed(&studio, "PUT", "/v1/apps/grants", &wide, None).await.status, 400);

    let listed = instance.device_signed(&phone, "GET", "/v1/apps/grants", None, None).await;
    let grants: Value = listed.value()["grants"].clone();
    assert_eq!(grants.as_array().map(Vec::len), Some(1), "{grants}");
    assert_eq!(grants[0]["agent"], json!({"id": "june", "incarnation": "inc-1"}));
    assert_eq!(get(&instance, &studio, "/v1/apps/grants").await.value()["grants"][0]["id"], id);

    // Signed in again: a new registration, and the grant is dead.
    instance.advance(60);
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let listed = instance.device_signed(&phone, "GET", "/v1/apps/grants", None, None).await;
    assert_eq!(listed.value()["grants"], json!([]));

    // Made again, and revoked by the person.
    let made = instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await;
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    let stranger = Device::new("192.0.2.81");
    instance.sign_in_device(&stranger, "eve@example.com", "Eve's phone").await;
    let path = format!("/v1/apps/grants/{id}");
    assert_eq!(instance.device_signed(&stranger, "DELETE", &path, None, None).await.status, 404, "another account's");
    assert_eq!(instance.device_signed(&phone, "DELETE", &path, None, None).await.status, 204);
    assert_eq!(get(&instance, &studio, "/v1/apps/grants").await.value()["grants"], json!([]));
}

#[tokio::test]
async fn without_a_worker_that_answers_rightly_app_routes_answer_503() {
    let mut studio = Claktory::new("198.51.100.30");
    // None configured.
    let instance = with_apps(None, 0).await;
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let answer = get(&instance, &studio, "/v1/apps/whoami").await;
    assert_eq!((answer.status, answer.error()), (503, "app_unavailable".to_owned()));
    drop(instance);

    // Configured, but not running.
    let dir = tempfile::tempdir().expect("a folder");
    let socket = dir.path().join("apps.sock");
    let instance = with_apps(Some(socket.clone()), uid(dir.path())).await;
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    assert_eq!(get(&instance, &studio, "/v1/apps/whoami").await.status, 503);

    // A worker that answers with a page: never served.
    let listener = tokio::net::UnixListener::bind(&socket).expect("a socket");
    let page = tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            let _ = claktory_user_apps::ipc::read_request(&mut stream).await;
            let _ = claktory_user_apps::ipc::write_answer(&mut stream, 200, b"<script>alert(1)</script>").await;
        }
    });
    let answer = get(&instance, &studio, "/v1/apps/whoami").await;
    assert_eq!((answer.status, answer.error()), (503, "app_unavailable".to_owned()));
    page.abort();
    let _ = page.await;
    std::fs::remove_file(&socket).expect("the page's socket");

    // A worker that answers only another user: the service isn't that user.
    worker(dir.path(), uid(dir.path()).wrapping_add(1));
    assert_eq!(get(&instance, &studio, "/v1/apps/whoami").await.status, 503);
    std::fs::remove_file(&socket).expect("that worker's socket");

    // Something listening as another user than the worker's (it put a
    // socket there while the worker was down): it's sent nothing.
    worker(dir.path(), uid(dir.path()));
    let impostor = with_apps(Some(socket.clone()), uid(dir.path()).wrapping_add(1)).await;
    impostor.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let answer = get(&impostor, &studio, "/v1/apps/whoami").await;
    assert_eq!((answer.status, answer.error()), (503, "app_unavailable".to_owned()));
    let worker_db = rusqlite::Connection::open(dir.path().join("apps.db")).expect("the worker's database");
    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read"],
                       "resources": [{"kind": "conversation", "id": "c1"}]});
    assert_eq!(impostor.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await.status, 503);
    let held: i64 = worker_db.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get(0)).expect("grants");
    assert_eq!(held, 0, "the listener got nothing");
}

#[tokio::test]
async fn a_deleted_account_s_grants_are_purged() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read"],
                       "resources": [{"kind": "conversation", "id": "c1"}]});
    assert_eq!(instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await.status, 200);
    let apps = rusqlite::Connection::open(dir.path().join("apps.db")).expect("the worker's database");
    let held = || apps.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get::<_, i64>(0)).expect("grants");
    assert_eq!(held(), 1);

    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.51").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    let csrf = field(&page, "csrf");
    let done = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(done.text().contains("Your account was deleted."));
    assert_eq!(held(), 0, "forgotten by the worker too");
}

#[tokio::test]
async fn the_account_page_lists_agents_access_and_takes_it_back() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket.clone()), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.51").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    assert!(!page.contains("Agents' access"), "none yet: {page}");

    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read", "dm.draft"],
                       "resources": [{"kind": "conversation", "id": "c1"}, {"kind": "conversation", "id": "c2"}]});
    let made = instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await;
    let id = made.value()["id"].as_str().expect("an id").to_owned();
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("Agents' access"), "{page}");
    assert!(page.contains("june on Mac mini"), "{page}");
    assert!(page.contains("Direct messages: read, write drafts · 2 conversations"), "{page}");
    assert!(page.contains(&format!("name=\"grant\" value=\"{id}\"")), "{page}");

    // Another account's page can't take it back, nor a form without the
    // session's CSRF token.
    let eve = web_sign_in(&instance, "eve@example.com", "192.0.2.52").await;
    let eve_page = crate::web::get(&instance, "/account", &eve).await.text();
    let eve_csrf = field(&eve_page, "csrf");
    post_form(&instance, "/account/remove-grant", &eve, &format!("csrf={eve_csrf}&grant={id}")).await;
    post_form(&instance, "/account/remove-grant", &cookie, &format!("csrf=wrong&grant={id}")).await;
    assert_eq!(get(&instance, &studio, "/v1/apps/grants").await.value()["grants"][0]["id"], id);

    let csrf = field(&page, "csrf");
    let removed = post_form(&instance, "/account/remove-grant", &cookie, &format!("csrf={csrf}&grant={id}")).await;
    assert_eq!(removed.status, 303);
    assert_eq!(get(&instance, &studio, "/v1/apps/grants").await.value()["grants"], json!([]));
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    assert!(!page.contains("june on Mac mini"), "{page}");

    // The worker down: the page says so rather than showing none.
    std::fs::remove_file(&socket).expect("the socket");
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("Can't show agents' access right now."), "{page}");
}

#[tokio::test]
async fn a_person_s_own_dm_identity_reaches_only_their_signed_in_devices() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let phone = Device::new("192.0.2.80");
    instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await;
    let laptop = Device::new("192.0.2.82");
    instance.sign_in_device(&laptop, "matt@example.com", "Matt's MacBook").await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;

    let me = instance.device_signed(&phone, "GET", "/v1/apps/dms/me", None, None).await;
    assert_eq!(me.status, 200, "{}", me.text());
    assert_eq!(me.value()["privacy"], json!({"requests_on": true, "revision": 1}));
    let shown =
        instance.device_signed(&phone, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({"request_nonce": "a"})), None).await;
    assert_eq!(shown.status, 200, "{}", shown.text());
    let code = shown.value()["code"].as_str().expect("a code").to_owned();
    assert!(code.starts_with("cf1_") && code.len() == 36, "{code}");

    // Another of the person's devices sees the same, and a change one makes.
    let named = instance
        .device_signed(&phone, "PUT", "/v1/apps/dms/me/profile", Some(&json!({"name": "Matt", "revision": 1, "request_nonce": "b"})), None)
        .await;
    assert_eq!(named.status, 200, "{}", named.text());
    let there =
        instance.device_signed(&laptop, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({"request_nonce": "c"})), None).await;
    assert_eq!(there.value()["code"], code.as_str());
    let reset = instance
        .device_signed(&laptop, "POST", "/v1/apps/dms/me/friend-id/reset", Some(&json!({"generation": 1, "request_nonce": "d"})), None)
        .await;
    assert_eq!(reset.value()["generation"], 2, "{}", reset.text());
    // In the same second as the first GET: a nonce in the query tells it apart.
    let me = instance.device_signed(&phone, "GET", "/v1/apps/dms/me?request_nonce=f", None, None).await.value();
    assert_eq!((me["profile"]["name"].as_str(), me["friend_id"]["generation"].as_i64()), (Some("Matt"), Some(2)));
    let stale = instance
        .device_signed(&phone, "POST", "/v1/apps/dms/me/friend-id/reset", Some(&json!({"generation": 1, "request_nonce": "e"})), None)
        .await;
    assert_eq!((stale.status, stale.error()), (409, "stale".to_owned()));

    // A Claktory, or one of its agents, gets nothing; another account its own.
    let backend = get(&instance, &studio, "/v1/apps/dms/me").await;
    assert_eq!((backend.status, backend.error()), (403, "device_only".to_owned()));
    let agent = get(&instance, &studio, "/v1/apps/dms/me?agent=june&incarnation=inc-1").await;
    assert_eq!(agent.status, 403);
    let eve = Device::new("192.0.2.81");
    instance.sign_in_device(&eve, "eve@example.com", "Eve's phone").await;
    let theirs = instance.device_signed(&eve, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({})), None).await;
    assert_ne!(theirs.value()["code"], reset.value()["code"]);
    assert_eq!(instance.device_signed(&phone, "GET", "/v1/apps/dms/people", None, None).await.status, 404, "no lookup");

    // Nothing readable at rest.
    let raw = std::fs::read(dir.path().join("apps.db")).expect("the file");
    let wal = std::fs::read(dir.path().join("apps.db-wal")).unwrap_or_default();
    for bytes in [raw, wal] {
        assert!(!bytes.windows(4).any(|window| window == b"cf1_"), "a code in the clear");
    }
}

#[tokio::test]
async fn two_people_become_friends_only_by_request_and_acceptance_from_their_devices() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let matt = Device::new("192.0.2.80");
    instance.sign_in_device(&matt, "matt@example.com", "Matt's iPhone").await;
    let ana = Device::new("192.0.2.81");
    instance.sign_in_device(&ana, "ana@example.com", "Ana's phone").await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;

    let named = instance.device_signed(&ana, "PUT", "/v1/apps/dms/me/profile", Some(&json!({"name": "Ana", "revision": 1})), None).await;
    assert_eq!(named.status, 200, "{}", named.text());
    let code = instance.device_signed(&matt, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({})), None).await.value()["code"]
        .as_str()
        .expect("a code")
        .to_owned();

    // Any well-formed code: the same 202, whoever it is.
    let unknown = json!({"code": "cf1_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA", "action_id": "ana-request-0000000000001"});
    let nobody = instance.device_signed(&ana, "POST", "/v1/apps/dms/requests", Some(&unknown), None).await;
    let sent = json!({"code": code, "action_id": "ana-request-0000000000002"});
    let someone = instance.device_signed(&ana, "POST", "/v1/apps/dms/requests", Some(&sent), None).await;
    assert_eq!((nobody.status, someone.status), (202, 202), "{}", someone.text());
    assert_eq!(nobody.value()["message"], someone.value()["message"]);
    let request = someone.value()["receipt"]["id"].as_str().expect("an id").to_owned();

    // Matt sees who asked; his Claktory and its agents can't see or accept it.
    let listed = instance.device_signed(&matt, "GET", "/v1/apps/dms/requests", None, None).await.value();
    assert_eq!(listed["incoming"][0]["from"]["name"], "Ana");
    assert_eq!(get(&instance, &studio, "/v1/apps/dms/requests").await.status, 403);
    let path = format!("/v1/apps/dms/requests/{request}/accept");
    assert_eq!(instance.signed(&studio, "POST", &path, &json!({}), None).await.status, 403);
    assert_eq!(instance.device_signed(&ana, "POST", &path, Some(&json!({})), None).await.status, 404, "not hers to accept");

    let accepted = instance.device_signed(&matt, "POST", &path, Some(&json!({"request_nonce": "a"})), None).await;
    assert_eq!(accepted.status, 200, "{}", accepted.text());
    let friends = instance.device_signed(&ana, "GET", "/v1/apps/dms/contacts", None, None).await.value();
    assert_eq!(friends["contacts"][0]["conversation_id"], accepted.value()["conversation_id"]);

    // Removed: gone for both.
    let contact = accepted.value()["id"].as_str().expect("an id").to_owned();
    let removed = instance.device_signed(&ana, "DELETE", &format!("/v1/apps/dms/contacts/{contact}"), None, None).await;
    assert_eq!(removed.status, 204);
    instance.advance(1);
    let friends = instance.device_signed(&matt, "GET", "/v1/apps/dms/contacts", None, None).await.value();
    assert_eq!(friends["contacts"], json!([]));
}

#[tokio::test]
async fn a_person_reopens_to_someone_they_declined_and_reports_them_from_their_devices() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let matt = Device::new("192.0.2.82");
    instance.sign_in_device(&matt, "matt@example.com", "Matt's iPhone").await;
    let ana = Device::new("192.0.2.83");
    instance.sign_in_device(&ana, "ana@example.com", "Ana's phone").await;
    let mut studio = Claktory::new("198.51.100.31");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let code = instance.device_signed(&matt, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({})), None).await.value()["code"]
        .as_str()
        .expect("a code")
        .to_owned();
    let sent = json!({"code": code, "action_id": "ana-request-0000000000003"});
    let request = instance.device_signed(&ana, "POST", "/v1/apps/dms/requests", Some(&sent), None).await.value()["receipt"]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let decline = format!("/v1/apps/dms/requests/{request}/decline");
    assert_eq!(instance.device_signed(&matt, "POST", &decline, Some(&json!({})), None).await.status, 204);

    // Matt reopens to her: her request waits again. His Claktory can't.
    assert_eq!(get(&instance, &studio, "/v1/apps/dms/declines").await.status, 403);
    let declines = instance.device_signed(&matt, "GET", "/v1/apps/dms/declines", None, None).await.value();
    let id = declines["declines"][0]["id"].as_str().expect("an id").to_owned();
    let reopened = instance.device_signed(&matt, "DELETE", &format!("/v1/apps/dms/declines/{id}"), None, None).await;
    assert_eq!(reopened.status, 200, "{}", reopened.text());
    assert_eq!(reopened.value()["request"]["id"], request.as_str());
    instance.advance(1);
    let listed = instance.device_signed(&matt, "GET", "/v1/apps/dms/requests", None, None).await.value();
    assert_eq!(listed["incoming"][0]["id"], request.as_str());

    // He reports it; his Claktory can't, and Ana can't see it.
    let report = json!({"action_id": "matt-report-00000000000001", "request_id": request, "reason": "spam"});
    assert_eq!(instance.signed(&studio, "POST", "/v1/apps/dms/reports", &report, None).await.status, 403);
    let reported = instance.device_signed(&matt, "POST", "/v1/apps/dms/reports", Some(&report), None).await;
    assert_eq!(reported.status, 200, "{}", reported.text());
    assert_eq!(reported.value()["state"], "received");
    instance.advance(1);
    let hers = instance.device_signed(&ana, "GET", "/v1/apps/dms/reports", None, None).await.value();
    assert_eq!(hers["reports"], json!([]));
    let his = instance.device_signed(&matt, "GET", "/v1/apps/dms/reports", None, None).await.value();
    assert_eq!(his["reports"][0]["id"], reported.value()["id"]);
}

#[tokio::test]
async fn friends_send_messages_from_their_devices_and_both_read_the_history() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let matt = Device::new("192.0.2.84");
    instance.sign_in_device(&matt, "matt@example.com", "Matt's iPhone").await;
    let ana = Device::new("192.0.2.85");
    instance.sign_in_device(&ana, "ana@example.com", "Ana's phone").await;
    let mut studio = Claktory::new("198.51.100.32");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let code = instance.device_signed(&matt, "POST", "/v1/apps/dms/me/friend-id/reveal", Some(&json!({})), None).await.value()["code"]
        .as_str()
        .expect("a code")
        .to_owned();
    let sent = json!({"code": code, "action_id": "ana-request-0000000000004"});
    let request = instance.device_signed(&ana, "POST", "/v1/apps/dms/requests", Some(&sent), None).await.value()["receipt"]["id"]
        .as_str()
        .expect("an id")
        .to_owned();
    let path = format!("/v1/apps/dms/requests/{request}/accept");
    let contact = instance.device_signed(&matt, "POST", &path, Some(&json!({})), None).await.value();
    let conversation = contact["conversation_id"].as_str().expect("a conversation").to_owned();
    let generation = contact["generation"].as_i64().expect("a generation");
    // Each side sees the other's account, whose keys its devices fetch.
    let anas = contact["account_id"].as_str().expect("Ana's account").to_owned();
    let mut friends = instance.device_signed(&ana, "GET", "/v1/apps/dms/contacts", None, None).await.value();
    let matts = friends["contacts"][0]["account_id"].take().as_str().expect("Matt's account").to_owned();
    // The conversation's group and Matt's Claktory's channel, as their
    // devices would have made them (the worker's own tests make them with MLS).
    let apps = rusqlite::Connection::open(dir.path().join("apps.db")).expect("the worker's database");
    let (group, channel) = ([1u8; 16], [2u8; 16]);
    apps.execute(
        "INSERT INTO dm_groups (group_id, kind, subject, account_id, conversation_id, contact_generation, epoch, last_seq, group_info, created)
         VALUES (?1, 'conversation', ?2, ?3, ?4, ?5, 0, 0, x'00', 0)",
        rusqlite::params![group, format!("conversation:{conversation}:{generation}"), anas.min(matts.clone()), conversation, generation],
    )
    .expect("a group");
    apps.execute(
        "INSERT INTO dm_groups (group_id, kind, subject, account_id, claktory_id, epoch, last_seq, group_info, created)
         VALUES (?1, 'channel', ?2, ?3, ?4, 0, 0, x'00', 0)",
        rusqlite::params![channel, format!("channel:{matts}:{}", studio.id), matts, studio.id],
    )
    .expect("a channel");

    let messages = format!("/v1/apps/dms/conversations/{conversation}/messages");
    let send = json!({"client_message_id": "ana-message-000000000001", "contact_generation": generation,
                      "envelope": sealed(&group, 0, "Hello!"), "request_nonce": "1"});
    let receipt = instance.device_signed(&ana, "POST", &messages, Some(&send), None).await;
    assert_eq!(receipt.status, 200, "{}", receipt.text());
    assert_eq!(receipt.value()["seq"], 1);
    // A retry, with a fresh nonce: the same receipt.
    instance.advance(1);
    let mut again = send.clone();
    again["request_nonce"] = json!("2");
    assert_eq!(instance.device_signed(&ana, "POST", &messages, Some(&again), None).await.value(), receipt.value());

    let page = instance.device_signed(&matt, "GET", &format!("{messages}?after=0&limit=10&request_nonce=3"), None, None).await;
    assert_eq!(page.status, 200, "{}", page.text());
    assert_eq!((&page.value()["messages"][0]["envelope"], &page.value()["messages"][0]["epoch"]), (&send["envelope"], &json!(0)));
    assert_eq!(page.value()["messages"][0]["from"], "them");
    // Matt's Claktory can't read it, nor send as him.
    assert_eq!(get(&instance, &studio, &messages).await.status, 403);
    assert_eq!(instance.signed(&studio, "POST", &messages, &send, None).await.status, 403);

    // Matt's devices keep up: a snapshot, then a wait that answers as soon
    // as Ana writes again.
    let snapshot = instance.device_signed(&matt, "GET", "/v1/apps/dms/sync", None, None).await;
    assert_eq!(snapshot.status, 200, "{}", snapshot.text());
    assert_eq!(snapshot.value()["conversations"][0]["last_seq"], 1);
    let cursor = snapshot.value()["cursor"].as_str().expect("a cursor").to_owned();
    let waiting = format!("/v1/apps/dms/sync?since={cursor}&wait=20");
    let mut next = send.clone();
    next["client_message_id"] = json!("ana-message-000000000002");
    next["envelope"] = sealed(&group, 0, "Still there?");
    let started = std::time::Instant::now();
    let (changes, sent) = tokio::join!(instance.device_signed(&matt, "GET", &waiting, None, None), async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        instance.device_signed(&ana, "POST", &messages, Some(&next), None).await
    });
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert_eq!(changes.status, 200, "{}", changes.text());
    let message = changes.value()["events"].as_array().expect("events").iter().find(|event| event["kind"] == "message").cloned();
    assert_eq!(message.expect("the message")["message"]["envelope"], next["envelope"]);
    assert!(started.elapsed() < std::time::Duration::from_secs(10), "{:?}", started.elapsed());
    assert_eq!(get(&instance, &studio, "/v1/apps/dms/sync").await.status, 403, "a Claktory, as itself, has no sync");

    // Matt's agent June proposes; only Matt, on his phone, sends it.
    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.send"],
                       "resources": [{"kind": "conversation", "id": conversation}]});
    assert_eq!(instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await.status, 200);
    let propose = json!({"client_message_id": "june-proposal-0000000001", "contact_generation": generation,
                         "envelope": sealed(&channel, 0, "Matt says hi"), "agent_name": "June"});
    let path = format!("/v1/apps/dms/conversations/{conversation}/proposals?agent=june&incarnation=inc-1");
    let proposed = instance.signed(&studio, "POST", &path, &propose, None).await;
    assert_eq!(proposed.status, 200, "{}", proposed.text());
    let id = proposed.value()["proposal_id"].as_str().expect("an id").to_owned();
    let waiting = instance.device_signed(&matt, "GET", "/v1/apps/dms/proposals", None, None).await.value()["proposals"][0].clone();
    assert_eq!(waiting["envelope"], propose["envelope"]);
    // Matt's phone reads it from the channel, and seals it again for Ana.
    let approval = json!({"revision": 1, "proposal_digest": waiting["proposal_digest"], "envelope": sealed(&group, 0, "Matt says hi")});
    let approve = format!("/v1/apps/dms/proposals/{id}/approve");
    assert_eq!(instance.signed(&studio, "POST", &approve, &approval, None).await.status, 403, "the backend never approves");
    let sent = instance.device_signed(&matt, "POST", &approve, Some(&approval), None).await;
    assert_eq!(sent.status, 200, "{}", sent.text());
    assert_eq!(sent.value()["state"], "sent");
    let page = instance.device_signed(&ana, "GET", &format!("{messages}?after=2"), None, None).await;
    assert_eq!(page.value()["messages"][0]["envelope"], approval["envelope"]);
    assert_eq!(page.value()["messages"][0]["by_agent"], true);
}

#[tokio::test]
async fn an_account_deleted_while_the_worker_is_down_is_purged_once_it_answers() {
    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket.clone()), uid(dir.path())).await;
    let mut studio = Claktory::new("198.51.100.30");
    instance.sign_in(&mut studio, "matt@example.com", "Mac mini").await;
    let grant = json!({"app": "dms", "agent": {"id": "june", "incarnation": "inc-1"}, "scopes": ["dm.read"],
                       "resources": [{"kind": "conversation", "id": "c1"}]});
    assert_eq!(instance.signed(&studio, "PUT", "/v1/apps/grants", &grant, None).await.status, 200);
    let apps = rusqlite::Connection::open(dir.path().join("apps.db")).expect("the worker's database");
    let held = || apps.query_row("SELECT COUNT(*) FROM grants", [], |row| row.get::<_, i64>(0)).expect("grants");
    let queued = || {
        rusqlite::Connection::open(&instance.db)
            .expect("the service's database")
            .query_row("SELECT COUNT(*) FROM apps_purges", [], |row| row.get::<_, i64>(0))
            .expect("queued")
    };

    // The worker is down when the account is deleted: the deletion goes
    // ahead, and the purge waits for it.
    std::fs::remove_file(&socket).expect("the socket");
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.51").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    let csrf = field(&page, "csrf");
    let done = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(done.text().contains("Your account was deleted."));
    assert_eq!((held(), queued()), (1, 1));

    // Back up: asked again after a minute, not before, then done for good.
    worker(dir.path(), uid(dir.path()));
    assert_eq!(instance.service.retry_app_purges().await, 0, "not due yet");
    instance.advance(60);
    assert_eq!(instance.service.retry_app_purges().await, 1);
    assert_eq!((held(), queued()), (0, 0));
    instance.advance(3600);
    assert_eq!(instance.service.retry_app_purges().await, 0, "nothing left to ask");
}

/// Hostile test 24 (dms-e2ee.md §9, L0): a device another of the person's
/// devices removed (lost or stolen) still holds their identity key, but
/// every call it makes to the worker is answered 401 before the worker
/// hears it, so a device list it signs is never accepted, and its relay
/// keys drop.
#[tokio::test]
async fn test_24_a_removed_device_reaches_the_worker_no_more() {
    use claktory_dm_crypto::certificate::LeafKind;
    use claktory_dm_crypto::device_list::{DeviceList, Listed};
    use claktory_dm_crypto::identity::IdentityKey;

    let dir = tempfile::tempdir().expect("a folder");
    let socket = worker(dir.path(), uid(dir.path()));
    let instance = with_apps(Some(socket), uid(dir.path())).await;
    let phone = Device::new("192.0.2.86");
    let account = instance.sign_in_device(&phone, "matt@example.com", "Matt's iPhone").await.account_id;
    let stolen = Device::new("192.0.2.87");
    instance.sign_in_device(&stolen, "matt@example.com", "Matt's iPad").await;
    let identity = IdentityKey::generate().expect("a key");
    let list = |version: u64, devices: &[(&Device, u8)]| {
        let leaves = devices
            .iter()
            .map(|(device, n)| Listed { kind: LeafKind::Device, leaf_id: device.account_key(), signature_key: [*n; 32] })
            .collect();
        let signed = DeviceList { account_id: account.clone(), version, created_ms: 1, leaves }.sign(&identity).expect("signed");
        b64(signed.to_bytes().expect("bytes"))
    };
    let first = json!({"identity_key": b64(identity.public().0), "device_list": list(1, &[(&phone, 1), (&stolen, 2)]),
                       "version": 1, "previous_version": 0});
    let answer = instance.device_signed(&phone, "PUT", "/v1/apps/dms/me/keys", Some(&first), None).await;
    assert_eq!(answer.status, 200, "{}", answer.text());
    assert_eq!(instance.device_signed(&stolen, "GET", "/v1/apps/dms/me", None, None).await.status, 200, "signed in, it reaches it");
    instance.device_signed(&stolen, "POST", "/v1/devices/assertion", Some(&json!({"device_key": "c".repeat(64)})), None).await;
    assert_eq!(instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>().endpoints, vec!["c".repeat(64)]);

    // Lost: the phone removes it.
    let answer =
        instance.device_signed(&phone, "POST", "/v1/devices/remove", Some(&json!({"account_key": stolen.account_key()})), None).await;
    assert_eq!(answer.json::<DeviceRemoved>(), DeviceRemoved { removed: true });
    instance.advance(1);

    // With the identity it holds, it lists itself alone, or rotates to a
    // key of its own; it reads anything: 401, every time.
    let itself = json!({"device_list": list(2, &[(&stolen, 2)]), "version": 2, "previous_version": 1});
    let rotate = json!({"identity_key": b64([9; 32]), "device_list": list(1, &[(&stolen, 2)]), "version": 1});
    for (method, path, body) in [
        ("PUT", "/v1/apps/dms/me/keys", Some(itself)),
        ("POST", "/v1/apps/dms/me/keys/rotate", Some(rotate)),
        ("GET", "/v1/apps/dms/me", None),
        ("GET", "/v1/apps/dms/groups", None),
        ("GET", &format!("/v1/apps/dms/keys/{account}") as &str, None),
    ] {
        let answer = instance.device_signed(&stolen, method, path, body.as_ref(), None).await;
        assert_eq!((answer.status, answer.error().as_str()), (401, "unauthorized"), "{path}");
    }
    assert!(instance.relay_allowlist(None, &instance.relay_bearer).await.json::<Allowlist>().endpoints.is_empty());
    // The account's keys are as the phone left them.
    let keys = instance.device_signed(&phone, "GET", &format!("/v1/apps/dms/keys/{account}"), None, None).await.value();
    assert_eq!((keys["version"].as_i64(), keys["device_list"].as_str()), (Some(1), first["device_list"].as_str()), "{keys}");
}
