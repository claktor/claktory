//! A Claktory's whole round with a running service, as a backend would make
//! it: only when CLAKTORY_ACCOUNT_LIVE names one (scripts/test-deploy-account.sh
//! does, for a service it deployed in a container). The rest of these tests
//! run their own instance instead.
//!
//!   CLAKTORY_ACCOUNT_LIVE      the service, e.g. https://account.test:8443
//!   CLAKTORY_ACCOUNT_RESOLVE   host:port=ip:port, to reach it without DNS
//!   CLAKTORY_ACCOUNT_CA        a PEM root certificate to trust (Caddy's own)
//!   CLAKTORY_ACCOUNT_MAIL      a shell command that prints the newest email

use claktory_account::protocol::*;

struct Live {
    url: String,
    client: reqwest::Client,
    service_key: Option<ed25519_dalek::VerifyingKey>,
}

impl Live {
    async fn send(&self, method: &str, path: &str, body: Vec<u8>, headers: &[(&str, String)]) -> (u16, Vec<u8>) {
        let mut request = self.client.request(method.parse().unwrap(), format!("{}{path}", self.url)).body(body.clone());
        for (name, value) in headers {
            request = request.header(*name, value);
        }
        let response = request.header("content-type", "application/json").send().await.expect("an answer");
        let status = response.status().as_u16();
        let signature = response.headers().get(SIGNATURE_HEADER).and_then(|value| value.to_str().ok()).map(str::to_owned);
        let answer = response.bytes().await.unwrap().to_vec();
        if let Some(key) = &self.service_key {
            let signature = signature.expect("a signed answer");
            assert!(verify_response(key, status, method, path, &body, &answer, &signature), "{method} {path}: signed by the service");
        }
        (status, answer)
    }

    async fn signed(
        &self,
        key: &ed25519_dalek::SigningKey,
        token: &str,
        method: &str,
        path: &str,
        body: &impl serde::Serialize,
    ) -> (u16, Vec<u8>) {
        let body = serde_json::to_vec(body).unwrap();
        let time = jiff::Timestamp::now().as_second();
        let headers = [
            ("authorization", format!("Bearer {token}")),
            (TIME_HEADER, time.to_string()),
            (SIGNATURE_HEADER, sign_request(key, method, path, time, &body)),
        ];
        self.send(method, path, body, &headers).await
    }
}

fn newest_code() -> String {
    let command = std::env::var("CLAKTORY_ACCOUNT_MAIL").expect("CLAKTORY_ACCOUNT_MAIL");
    let output = std::process::Command::new("sh").arg("-c").arg(&command).output().expect("the mail command runs");
    let text = String::from_utf8_lossy(&output.stdout).to_string();
    let subject =
        text.lines().find(|line| line.starts_with("Subject: Your Claktory code: ")).unwrap_or_else(|| panic!("no code in {text}"));
    subject.rsplit(' ').next().unwrap().to_owned()
}

#[tokio::test]
async fn a_claktory_signs_in_unlocks_and_checks_in_with_a_running_service() {
    let Ok(url) = std::env::var("CLAKTORY_ACCOUNT_LIVE") else { return };
    let mut client = reqwest::Client::builder();
    if let Ok(resolve) = std::env::var("CLAKTORY_ACCOUNT_RESOLVE") {
        let (host, address) = resolve.split_once('=').expect("host:port=ip:port");
        client = client.resolve(host.split(':').next().unwrap(), address.parse().expect("ip:port"));
    }
    if let Ok(path) = std::env::var("CLAKTORY_ACCOUNT_CA") {
        let pem = std::fs::read(path).expect("the CA file");
        client = client.add_root_certificate(reqwest::Certificate::from_pem(&pem).expect("a certificate"));
    }
    let mut live = Live { url: url.trim_end_matches('/').to_owned(), client: client.build().unwrap(), service_key: None };

    let (status, body) = live.send("GET", WELL_KNOWN, Vec::new(), &[]).await;
    assert_eq!(status, 200);
    let known: WellKnown = serde_json::from_slice(&body).unwrap();
    live.service_key = Some(verifying_key(&known.service_key).expect("a key"));
    eprintln!("service key {}", known.service_key);

    let claktory_id = b64(&random_32()[..18]);
    let key = new_signing_key();
    let start = SignInStart {
        email: "live-test@example.com".into(),
        claktory_id: claktory_id.clone(),
        name: "Container test".into(),
        backend_key: b64(key.verifying_key().to_bytes()),
        version: "v0.1.9".into(),
        platform: "linux-aarch64".into(),
    };
    let (status, body) = live.send("POST", "/v1/sign-in/start", serde_json::to_vec(&start).unwrap(), &[]).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let started: SignInStarted = serde_json::from_slice(&body).unwrap();
    let ephemeral = Ephemeral::new();
    let verify = SignInVerify { request_id: started.request_id, code: newest_code(), eph_pub: ephemeral.public_b64() };
    let (status, body) = live.send("POST", "/v1/sign-in/verify", serde_json::to_vec(&verify).unwrap(), &[]).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let signed_in: SignedIn = serde_json::from_slice(&body).unwrap();
    let first = ephemeral.open(&signed_in.sealed_share, &claktory_id, signed_in.epoch).expect("the share opens");
    eprintln!("signed in: account {}, epoch {}", signed_in.account_id, signed_in.epoch);

    let ephemeral = Ephemeral::new();
    let (status, body) = live
        .signed(&key, &signed_in.machine_token, "POST", "/v1/share", &ShareRequest { eph_pub: ephemeral.public_b64(), epoch: None })
        .await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let share: Share = serde_json::from_slice(&body).unwrap();
    assert_eq!(*ephemeral.open(&share.sealed_share, &claktory_id, share.epoch).expect("opens"), *first, "the same share after a restart");

    let now = jiff::Timestamp::now().as_second();
    let hour = now - now.rem_euclid(3600);
    let check_in = CheckIn {
        version: "v0.1.9".into(),
        platform: "linux-aarch64".into(),
        counts: Some(Counts { agents: 3, projects: 1, devices: 1 }),
        periods: vec![Period {
            id: b64(&random_32()[..18]),
            from: hour,
            to: now,
            usage: vec![Usage {
                provider: "anthropic".into(),
                model: "claude-opus-4-8".into(),
                model_name: Some("Claude Opus 4.8".into()),
                requests: 2,
                input_tokens: 1000,
                output_tokens: 100,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
            }],
            activity: vec![Activity { hour, agent_minutes: 1, agents_working: 1, runs: 1 }],
        }],
    };
    let (status, body) = live.signed(&key, &signed_in.machine_token, "POST", "/v1/check-in", &check_in).await;
    assert_eq!(status, 200, "{}", String::from_utf8_lossy(&body));
    let checked: CheckedIn = serde_json::from_slice(&body).unwrap();
    assert_eq!((checked.periods_recorded, checked.epoch), (1, signed_in.epoch));

    let endpoints = Endpoints { backend: "a".repeat(64), devices: vec!["b".repeat(64)] };
    let (status, _) = live.signed(&key, &signed_in.machine_token, "PUT", "/v1/endpoints", &endpoints).await;
    assert_eq!(status, 200);

    let (status, body) = live.send("GET", "/v1/stats/public", Vec::new(), &[]).await;
    assert_eq!(status, 200);
    let public: PublicStats = serde_json::from_slice(&body).unwrap();
    eprintln!("public stats: {public:?}");

    let (status, page) = live.send("GET", "/account", Vec::new(), &[]).await;
    assert_eq!(status, 200);
    assert!(String::from_utf8_lossy(&page).contains("Sign in to Claktory"));
}
