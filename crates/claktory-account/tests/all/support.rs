//! A local instance of the service, with mail kept in memory and a clock
//! the test sets, and a client that checks every answer's signature.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};

use claktory_account::protocol::{self, *};
use claktory_account::server::{self, Config, Secrets, Service, mail};
use ed25519_dalek::{SigningKey, VerifyingKey};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// 20 Nov 2026, 14:02 UTC.
pub const START: i64 = 1_795_183_320;

pub struct Instance {
    pub url: String,
    pub mail: Arc<mail::Memory>,
    pub time: Arc<AtomicI64>,
    pub k_share: [u8; 32],
    pub relay_bearer: String,
    pub service_key: VerifyingKey,
    pub db: std::path::PathBuf,
    pub service: Arc<claktory_account::server::Service>,
    client: reqwest::Client,
    _dir: tempfile::TempDir,
}

/// An answer, its signature checked.
pub struct Answer {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: reqwest::header::HeaderMap,
}

impl std::fmt::Debug for Answer {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(f, "{} {}", self.status, self.text())
    }
}

impl Answer {
    pub fn json<T: DeserializeOwned>(&self) -> T {
        serde_json::from_slice(&self.body).unwrap_or_else(|error| panic!("{error}: {}", String::from_utf8_lossy(&self.body)))
    }

    pub fn value(&self) -> Value {
        self.json()
    }

    pub fn error(&self) -> String {
        self.value()["error"].as_str().unwrap_or_default().to_owned()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

pub async fn start() -> Instance {
    start_with(Config { trust_proxy: true, ..Config::default() }).await
}

/// Lets the tests open as many files as the system allows: each runs a
/// service, its database and clients at once, past the 256 a shell may
/// start with.
fn more_files() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let mut limit = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
        // SAFETY: getrlimit and setrlimit only read and write `limit`.
        unsafe {
            if libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) == 0 && limit.rlim_cur < 4096 {
                limit.rlim_cur = limit.rlim_max.min(4096);
                libc::setrlimit(libc::RLIMIT_NOFILE, &limit);
            }
        }
    });
}

pub async fn start_with(config: Config) -> Instance {
    more_files();
    let dir = tempfile::tempdir().expect("a folder");
    let secrets = Secrets::generate();
    let k_share = *secrets.k_share;
    let relay_bearer = secrets.relay_bearer.clone().expect("a bearer");
    let service_key = secrets.signing_key.verifying_key();
    let mail = Arc::new(mail::Memory::default());
    let time = Arc::new(AtomicI64::new(START));
    let clock_time = time.clone();
    let db = dir.path().join("account.db");
    let service =
        Service::new(&db, secrets, config, mail.clone(), Arc::new(move || clock_time.load(Ordering::SeqCst))).expect("the service");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    tokio::spawn(server::serve(listener, service.clone()));
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none()).build().expect("a client");
    Instance { url, mail, time, k_share, relay_bearer, service_key, db, service, client, _dir: dir }
}

/// A Claktory as the service sees it: its id, backend key and, once signed
/// in, its token and epoch.
pub struct Claktory {
    pub id: String,
    pub key: SigningKey,
    pub token: String,
    pub epoch: u64,
    /// Where its requests come from (X-Forwarded-For).
    pub ip: String,
}

impl Claktory {
    pub fn new(ip: &str) -> Claktory {
        Claktory { id: b64(&random_32()[..18]), key: new_signing_key(), token: String::new(), epoch: 0, ip: ip.to_owned() }
    }
}

/// A person's device (the app on a phone, say), by its account key.
pub struct Device {
    pub key: SigningKey,
    /// Where its requests come from (X-Forwarded-For).
    pub ip: String,
}

impl Device {
    pub fn new(ip: &str) -> Device {
        Device { key: new_signing_key(), ip: ip.to_owned() }
    }

    pub fn account_key(&self) -> String {
        b64(self.key.verifying_key().to_bytes())
    }
}

impl Instance {
    pub fn advance(&self, seconds: i64) {
        self.time.fetch_add(seconds, Ordering::SeqCst);
    }

    pub fn now(&self) -> i64 {
        self.time.load(Ordering::SeqCst)
    }

    /// Sends a request and checks the answer is signed for it.
    pub async fn send(&self, method: &str, path: &str, body: Vec<u8>, headers: &[(&str, String)]) -> Answer {
        let mut request = self.client.request(method.parse().expect("a method"), format!("{}{path}", self.url)).body(body.clone());
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        if !body.is_empty() && !headers.iter().any(|(name, _)| name.eq_ignore_ascii_case("content-type")) {
            request = request.header("content-type", "application/json");
        }
        let response = request.send().await.expect("an answer");
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let answer = response.bytes().await.expect("a body").to_vec();
        let signature = headers.get(SIGNATURE_HEADER).and_then(|value| value.to_str().ok()).expect("a signed answer");
        assert!(verify_response(&self.service_key, status, method, path, &body, &answer, signature), "{method} {path}: the signature");
        assert!(!verify_response(&self.service_key, status + 1, method, path, &body, &answer, signature), "bound to its status");
        Answer { status, body: answer, headers }
    }

    pub async fn post(&self, path: &str, body: &impl Serialize, ip: &str) -> Answer {
        self.send("POST", path, serde_json::to_vec(body).expect("json"), &[("x-forwarded-for", ip.to_owned())]).await
    }

    /// A signed request from `claktory`, at `time` (now if None).
    pub async fn signed(&self, claktory: &Claktory, method: &str, path: &str, body: &impl Serialize, time: Option<i64>) -> Answer {
        let body = serde_json::to_vec(body).expect("json");
        let time = time.unwrap_or_else(|| self.now());
        let signature = sign_request(&claktory.key, method, path, time, &body);
        let headers = [
            ("authorization", format!("Bearer {}", claktory.token)),
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, signature),
            ("x-forwarded-for", claktory.ip.clone()),
        ];
        self.send(method, path, body, &headers).await
    }

    /// A device-signed request from `device`, at `time` (now if None); a
    /// GET has no body.
    pub async fn device_signed(&self, device: &Device, method: &str, path: &str, body: Option<&Value>, time: Option<i64>) -> Answer {
        let body = body.map(|body| serde_json::to_vec(body).expect("json")).unwrap_or_default();
        let time = time.unwrap_or_else(|| self.now());
        let signature = sign_request(&device.key, method, path, time, &body);
        let headers = [
            ("authorization", format!("{DEVICE_AUTH} {}", device.account_key())),
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, signature),
            ("x-forwarded-for", device.ip.clone()),
        ];
        self.send(method, path, body, &headers).await
    }

    pub async fn start_device_sign_in(&self, device: &Device, email: &str, name: &str) -> Answer {
        let start = DeviceSignInStart {
            email: email.to_owned(),
            account_key: device.account_key(),
            name: name.to_owned(),
            platform: "ios".to_owned(),
            version: "v0.1.13".to_owned(),
        };
        self.post("/v1/devices/sign-in/start", &start, &device.ip).await
    }

    /// Signs `device` in to `email`'s account.
    pub async fn sign_in_device(&self, device: &Device, email: &str, name: &str) -> DeviceAccount {
        let answer = self.start_device_sign_in(device, email, name).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        let started: SignInStarted = answer.json();
        let verify = serde_json::json!({"request_id": started.request_id, "code": self.last_code(email)});
        let answer = self.device_signed(device, "POST", "/v1/devices/sign-in/verify", Some(&verify), None).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        answer.json()
    }

    /// The code in the last email to `email`.
    pub fn last_code(&self, email: &str) -> String {
        let mail = self.mail.sent().into_iter().rev().find(|mail| mail.to == email).expect("an email");
        mail.subject.rsplit(' ').next().expect("a code").to_owned()
    }

    pub async fn start_sign_in(&self, claktory: &Claktory, email: &str, name: &str) -> Answer {
        let start = SignInStart {
            email: email.to_owned(),
            claktory_id: claktory.id.clone(),
            name: name.to_owned(),
            backend_key: b64(claktory.key.verifying_key().to_bytes()),
            version: "v0.1.9".to_owned(),
            platform: "macos-arm64".to_owned(),
        };
        self.post("/v1/sign-in/start", &start, &claktory.ip).await
    }

    /// Signs `claktory` in to `email`'s account, and checks its share. Its
    /// verify is signed with its backend key, as factoryd signs it: proof
    /// of the key it has there when it signs in again.
    pub async fn sign_in(&self, claktory: &mut Claktory, email: &str, name: &str) -> SignedIn {
        let answer = self.start_sign_in(claktory, email, name).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        let started: SignInStarted = answer.json();
        let ephemeral = Ephemeral::new();
        let verify = SignInVerify {
            request_id: started.request_id,
            code: self.last_code(&normalize_email(email).expect("an email")),
            eph_pub: ephemeral.public_b64(),
        };
        let body = serde_json::to_vec(&verify).expect("json");
        let time = self.now();
        let headers = [
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, sign_request(&claktory.key, "POST", "/v1/sign-in/verify", time, &body)),
            ("x-forwarded-for", claktory.ip.clone()),
        ];
        let answer = self.send("POST", "/v1/sign-in/verify", body, &headers).await;
        assert_eq!(answer.status, 200, "{}", answer.text());
        let signed_in: SignedIn = answer.json();
        let share = ephemeral.open(&signed_in.sealed_share, &claktory.id, signed_in.epoch).expect("the share opens");
        assert_eq!(*share, *derive_share(&self.k_share, &claktory.id, signed_in.epoch));
        claktory.token = signed_in.machine_token.clone();
        claktory.epoch = signed_in.epoch;
        signed_in
    }

    /// Asks for `claktory`'s share and opens it.
    pub async fn share(&self, claktory: &Claktory, epoch: Option<u64>) -> Result<(u64, [u8; 32]), Answer> {
        let ephemeral = Ephemeral::new();
        let answer = self.signed(claktory, "POST", "/v1/share", &ShareRequest { eph_pub: ephemeral.public_b64(), epoch }, None).await;
        if answer.status != 200 {
            return Err(answer);
        }
        let share: Share = answer.json();
        let opened = ephemeral.open(&share.sealed_share, &claktory.id, share.epoch).expect("the share opens");
        Ok((share.epoch, *opened))
    }

    pub fn expected_share(&self, claktory: &Claktory, epoch: u64) -> [u8; 32] {
        *derive_share(&self.k_share, &claktory.id, epoch)
    }

    pub async fn relay_allowlist(&self, since: Option<i64>, bearer: &str) -> Answer {
        let path = match since {
            Some(since) => format!("/v1/relay/allowlist?since={since}"),
            None => "/v1/relay/allowlist".to_owned(),
        };
        self.send("GET", &path, Vec::new(), &[("authorization", format!("Bearer {bearer}"))]).await
    }
}

pub use protocol::Ephemeral;
