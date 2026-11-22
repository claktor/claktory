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
    let worker = Worker::open(&dir.join("apps.db")).expect("the worker");
    tokio::spawn(worker::serve(listener, worker, root_uid));
    socket
}

/// A service whose worker listens at `socket` as `uid`.
async fn with_apps(socket: Option<PathBuf>, uid: u32) -> Instance {
    let user_apps = socket.map(|socket| UserApps { socket, uid });
    start_with(Config { trust_proxy: true, user_apps, ..Config::default() }).await
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
