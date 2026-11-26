use std::{net::Ipv4Addr, time::Duration};

use iroh_base::{RelayUrl, SecretKey};
use iroh_relay::{
    client::ClientBuilder,
    http::ProtocolVersion,
    protos::relay::{ClientToRelayMsg, Datagrams, RelayToClientMsg},
    server::{RelayConfig, Server, ServerConfig},
};
use n0_future::{SinkExt, StreamExt};

use super::*;

/// The relay's clock in these tests.
const NOW: u64 = 1_800_000_000;
const DAY: u64 = 24 * 3600;
const ACCOUNT: &str = "00112233445566778899aabbccddeeff";

fn signer() -> SecretKey {
    SecretKey::from_bytes(&[7; 32])
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn endpoint(seed: u8) -> SecretKey {
    SecretKey::from_bytes(&[seed; 32])
}

/// A token as the account service makes it: `payload`, signed by `key`
/// after the domain.
fn signed(key: &SecretKey, payload: &str) -> String {
    let signature = key.sign(&[DOMAIN, payload.as_bytes()].concat());
    format!("{}.{}", URL_SAFE_NO_PAD.encode(payload), URL_SAFE_NO_PAD.encode(signature.to_bytes()))
}

fn payload(endpoint: EndpointId, account: &str, iat: u64, exp: u64) -> String {
    format!(r#"{{"v":1,"e":"{}","a":"{account}","iat":{iat},"exp":{exp}}}"#, hex_of(endpoint.as_bytes()))
}

/// A good token for `endpoint`, from `NOW` for 7 days.
fn token_for(endpoint: EndpointId) -> String {
    signed(&signer(), &payload(endpoint, ACCOUNT, NOW, NOW + 7 * DAY))
}

fn keys() -> Vec<PublicKey> {
    vec![signer().public()]
}

fn check(token: &str, endpoint: EndpointId) -> Result<AccountTag, Rejected> {
    verify(token, endpoint, &keys(), &HashSet::new(), NOW)
}

#[test]
fn a_good_token_names_its_account() {
    let me = endpoint(1).public();
    assert_eq!(check(&token_for(me), me), Ok(hex::<16>(ACCOUNT).unwrap()));
    // Signed by the second key of two.
    let other = SecretKey::from_bytes(&[9; 32]);
    let token = signed(&other, &payload(me, ACCOUNT, NOW, NOW + DAY));
    assert!(verify(&token, me, &[signer().public(), other.public()], &HashSet::new(), NOW).is_ok());
    // Key order in the payload doesn't matter.
    let reordered = format!(r#"{{"exp":{},"iat":{NOW},"a":"{ACCOUNT}","e":"{}","v":1}}"#, NOW + DAY, hex_of(me.as_bytes()));
    assert!(check(&signed(&signer(), &reordered), me).is_ok());
}

#[test]
fn a_token_that_isnt_ours_gives_nothing() {
    let me = endpoint(1).public();
    let good = payload(me, ACCOUNT, NOW, NOW + DAY);
    let forged = signed(&SecretKey::from_bytes(&[8; 32]), &good);
    assert_eq!(check(&forged, me), Err(Rejected::Bad), "another key");
    let undomained = {
        let signature = signer().sign(good.as_bytes());
        format!("{}.{}", URL_SAFE_NO_PAD.encode(&good), URL_SAFE_NO_PAD.encode(signature.to_bytes()))
    };
    assert_eq!(check(&undomained, me), Err(Rejected::Bad), "signed without the domain");
    let (head, tail) = token_for(me).split_once('.').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap();
    let tampered = URL_SAFE_NO_PAD.encode(good.replace(ACCOUNT, "ffffffffffffffffffffffffffffffff"));
    assert_eq!(check(&format!("{tampered}.{tail}"), me), Err(Rejected::Bad), "payload changed");
    assert_eq!(check(&format!("{head}={}.{tail}", ""), me), Err(Rejected::Bad), "padding");
    assert_eq!(check(&head, me), Err(Rejected::Bad), "no signature");
    assert_eq!(check(&format!("{head}.{tail}.x"), me), Err(Rejected::Bad));
    assert_eq!(check("", me), Err(Rejected::Bad));
    assert_eq!(verify(&token_for(me), me, &[], &HashSet::new(), NOW), Err(Rejected::Bad), "no keys loaded");
    let long = signed(
        &signer(),
        &format!(r#"{{"v":1,"e":"{}","a":"{ACCOUNT}","iat":{NOW},"exp":{}{}}}"#, hex_of(me.as_bytes()), NOW + DAY, " ".repeat(400)),
    );
    assert!(long.len() > MAX_TOKEN);
    assert_eq!(check(&long, me), Err(Rejected::Bad), "over 512 bytes");
}

#[test]
fn a_signed_payload_is_still_read_strictly() {
    let me = endpoint(1).public();
    let e = hex_of(me.as_bytes());
    let exp = NOW + DAY;
    for (why, payload) in [
        ("unknown field", format!(r#"{{"v":1,"e":"{e}","a":"{ACCOUNT}","iat":{NOW},"exp":{exp},"x":1}}"#)),
        ("duplicate field", format!(r#"{{"v":1,"e":"{e}","a":"{ACCOUNT}","a":"{ACCOUNT}","iat":{NOW},"exp":{exp}}}"#)),
        ("missing field", format!(r#"{{"v":1,"e":"{e}","iat":{NOW},"exp":{exp}}}"#)),
        ("version", format!(r#"{{"v":2,"e":"{e}","a":"{ACCOUNT}","iat":{NOW},"exp":{exp}}}"#)),
        ("upper-case hex", format!(r#"{{"v":1,"e":"{}","a":"{ACCOUNT}","iat":{NOW},"exp":{exp}}}"#, e.to_uppercase())),
        ("short account", format!(r#"{{"v":1,"e":"{e}","a":"0011","iat":{NOW},"exp":{exp}}}"#)),
        ("lives over 8 days", payload(me, ACCOUNT, NOW, NOW + 9 * DAY)),
        ("ends before it starts", payload(me, ACCOUNT, NOW, NOW)),
        ("issued in the future", payload(me, ACCOUNT, NOW + 61, NOW + DAY)),
        ("not JSON", "v=1".to_owned()),
    ] {
        assert_eq!(check(&signed(&signer(), &payload), me), Err(Rejected::Bad), "{why}");
    }
    assert!(check(&signed(&signer(), &payload(me, ACCOUNT, NOW + 60, NOW + DAY)), me).is_ok(), "60 s of skew");
}

#[test]
fn expired_bound_elsewhere_or_denied_gives_nothing() {
    let me = endpoint(1).public();
    assert_eq!(check(&signed(&signer(), &payload(me, ACCOUNT, NOW - 7 * DAY, NOW)), me), Err(Rejected::Expired));
    assert_eq!(check(&token_for(endpoint(2).public()), me), Err(Rejected::Endpoint));
    let denied = HashSet::from([hex::<16>(ACCOUNT).unwrap()]);
    assert_eq!(verify(&token_for(me), me, &keys(), &denied, NOW), Err(Rejected::Denied));
}

#[test]
fn key_files_and_denylists_are_read_strictly() {
    let key = hex_of(signer().public().as_bytes());
    let other = hex_of(SecretKey::from_bytes(&[9; 32]).public().as_bytes());
    assert_eq!(parse_keys(format!(r#"{{"seq":3,"keys":["{key}","{other}"]}}"#).as_bytes()).unwrap().keys.len(), 2);
    for (why, file) in [
        ("no keys", r#"{"seq":1,"keys":[]}"#.to_owned()),
        ("three keys", format!(r#"{{"seq":1,"keys":["{key}","{other}","{key}"]}}"#)),
        ("not hex", r#"{"seq":1,"keys":["nope"]}"#.to_owned()),
        ("unknown field", format!(r#"{{"seq":1,"keys":["{key}"],"x":0}}"#)),
        ("no seq", format!(r#"{{"keys":["{key}"]}}"#)),
        ("negative seq", format!(r#"{{"seq":-1,"keys":["{key}"]}}"#)),
        ("over 4 KB", format!(r#"{{"seq":1,"keys":["{key}"]}}{}"#, " ".repeat(4096))),
    ] {
        assert!(parse_keys(file.as_bytes()).is_err(), "{why}");
    }

    let empty = parse_denylist(br#"{"seq":4,"a":[]}"#).unwrap();
    assert_eq!((empty.seq, empty.accounts.len()), (4, 0));
    assert_eq!(parse_denylist(format!(r#"{{"seq":1,"a":["{ACCOUNT}"]}}"#).as_bytes()).unwrap().accounts.len(), 1);
    assert!(parse_denylist(br#"{"seq":1,"a":["00"]}"#).is_err());
    assert!(parse_denylist(br#"{"seq":1,"a":[],"keys":[]}"#).is_err());
    let too_many = format!(r#"{{"seq":1,"a":[{}]}}"#, vec![format!("\"{ACCOUNT}\""); MAX_DENIED + 1].join(","));
    assert!(parse_denylist(too_many.as_bytes()).is_err());
}

/// Files in a folder of their own, as the deploy writes them.
struct Files {
    _dir: tempfile::TempDir,
    config: Config,
}

impl Files {
    fn new(mode: Mode) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = Config { mode, keys: dir.path().join("keys.json"), denylist: dir.path().join("denylist.json") };
        Self { _dir: dir, config }
    }

    fn with_keys(self, seq: u64) -> Self {
        self.write_keys(seq, &signer());
        self
    }

    fn with_denylist(self, seq: u64, accounts: &[&str]) -> Self {
        self.write_denylist(seq, accounts);
        self
    }

    /// Replaced with a rename, as the deploy's wrapper does.
    fn replace(&self, path: &Path, contents: String) {
        let temporary = path.with_extension("tmp");
        std::fs::write(&temporary, contents).unwrap();
        std::fs::rename(temporary, path).unwrap();
    }

    fn write_keys(&self, seq: u64, key: &SecretKey) {
        self.replace(&self.config.keys, format!(r#"{{"seq":{seq},"keys":["{}"]}}"#, hex_of(key.public().as_bytes())));
    }

    fn write_denylist(&self, seq: u64, accounts: &[&str]) {
        let list = accounts.iter().map(|a| format!("\"{a}\"")).collect::<Vec<_>>().join(",");
        self.replace(&self.config.denylist, format!(r#"{{"seq":{seq},"a":[{list}]}}"#));
    }
}

fn access_at_now(files: &Files) -> Arc<ClaktoryAccess> {
    let access = Arc::new(ClaktoryAccess::with_clock(Arc::default(), || NOW));
    access.load_keys(&files.config.keys).ok();
    access.load_denylist(&files.config.denylist).ok();
    access.set_mode(files.config.mode).unwrap();
    access
}

/// A connection request from `key`, with `token` if any.
fn request(key: &SecretKey, token: Option<&str>) -> ClientRequest {
    let mut builder = http::Request::builder().uri("/relay");
    if let Some(token) = token {
        builder = builder.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
    }
    ClientRequest::new(key.public(), ProtocolVersion::default(), builder.body(()).unwrap().into_parts().0)
}

#[tokio::test]
async fn authorization_lasts_until_the_last_authorized_connection_ends() {
    let files = Files::new(Mode::Enforce).with_keys(1).with_denylist(1, &[]);
    let access = access_at_now(&files);
    let me = endpoint(1);
    let first = request(&me, Some(&token_for(me.public())));
    let second = request(&me, Some(&token_for(me.public())));
    let plain = request(&me, None);
    for request in [&first, &second, &plain] {
        assert!(matches!(access.on_connect(request).await, Access::Allow), "everyone gets in");
    }
    assert!(access.is_authorized(me.public()));
    assert_eq!((access.metrics.clients_authorized.get(), access.metrics.clients_unauthorized.get()), (1, 0));
    access.on_disconnect(me.public(), first.connection_id());
    assert!(access.is_authorized(me.public()), "one authorized connection is left");
    access.on_disconnect(me.public(), second.connection_id());
    assert!(!access.is_authorized(me.public()), "only the one without a token is left");
    assert_eq!((access.metrics.clients_authorized.get(), access.metrics.clients_unauthorized.get()), (0, 1));
    access.on_disconnect(me.public(), plain.connection_id());
    assert_eq!((access.metrics.clients_authorized.get(), access.metrics.clients_unauthorized.get()), (0, 0));

    // A bad token gets in too, without authorization, and is counted.
    let stranger = endpoint(2);
    let borrowed = request(&stranger, Some(&token_for(me.public())));
    assert!(matches!(access.on_connect(&borrowed).await, Access::Allow));
    assert!(!access.is_authorized(stranger.public()));
    assert_eq!(access.metrics.tokens_rejected_endpoint.get(), 1);
    assert_eq!(access.metrics.tokens_accepted.get(), 2);
}

#[tokio::test]
async fn forwarding_needs_a_signed_in_end() {
    let files = Files::new(Mode::Enforce).with_keys(1).with_denylist(1, &[]);
    let access = access_at_now(&files);
    let (signed_in, a, b) = (endpoint(1), endpoint(2), endpoint(3));
    access.on_connect(&request(&signed_in, Some(&token_for(signed_in.public())))).await;
    access.on_connect(&request(&a, None)).await;
    access.on_connect(&request(&b, None)).await;
    assert!(access.allow_forward(a.public(), signed_in.public(), 100), "to a signed-in Claktory");
    assert!(access.allow_forward(signed_in.public(), a.public(), 100), "from one");
    assert!(!access.allow_forward(a.public(), b.public(), 100), "between two others");
    assert_eq!((access.metrics.forward_denied_packets.get(), access.metrics.forward_denied_bytes.get()), (1, 100));

    access.set_mode(Mode::Observe).unwrap();
    assert!(access.allow_forward(a.public(), b.public(), 50), "observing forwards it");
    assert_eq!((access.metrics.forward_denied_packets.get(), access.metrics.forward_denied_bytes.get()), (2, 150), "and counts it");
}

#[tokio::test]
async fn a_denied_account_loses_its_live_connections_at_once() {
    let files = Files::new(Mode::Enforce).with_keys(1).with_denylist(1, &[]);
    let access = access_at_now(&files);
    let (me, other) = (endpoint(1), endpoint(2));
    access.on_connect(&request(&me, Some(&token_for(me.public())))).await;
    access.on_connect(&request(&other, None)).await;
    assert!(access.allow_forward(other.public(), me.public(), 1));

    files.write_denylist(2, &[ACCOUNT]);
    access.load_denylist(&files.config.denylist).unwrap();
    assert!(!access.is_authorized(me.public()));
    assert!(!access.allow_forward(other.public(), me.public(), 1));
    assert_eq!((access.metrics.clients_authorized.get(), access.metrics.clients_unauthorized.get()), (0, 2));
    // And it can't come back with the same token.
    access.on_connect(&request(&me, Some(&token_for(me.public())))).await;
    assert!(!access.is_authorized(me.public()));
    assert_eq!(access.metrics.tokens_rejected_denied.get(), 1);
}

#[test]
fn an_older_or_broken_file_keeps_the_last_good_one() {
    let files = Files::new(Mode::Enforce).with_keys(5).with_denylist(5, &[ACCOUNT]);
    let access = access_at_now(&files);
    let me = endpoint(1).public();

    files.write_keys(4, &SecretKey::from_bytes(&[8; 32]));
    assert!(access.load_keys(&files.config.keys).unwrap_err().contains("older"));
    std::fs::write(&files.config.keys, "{").unwrap();
    assert!(access.load_keys(&files.config.keys).is_err());
    assert!(
        access.check(&signed(&signer(), &payload(me, "ffffffffffffffffffffffffffffffff", NOW, NOW + DAY)), me).is_some(),
        "the loaded key still counts"
    );

    files.write_denylist(4, &[]);
    assert!(access.load_denylist(&files.config.denylist).is_err(), "a replayed older denylist would lift a denial");
    assert!(access.check(&token_for(me), me).is_none(), "still denied");
    files.write_denylist(5, &[]);
    access.load_denylist(&files.config.denylist).unwrap();
    assert!(access.check(&token_for(me), me).is_some(), "the same seq again is read");
    assert_eq!(access.metrics.reload_failures.get(), 3);
}

#[test]
fn enforcing_needs_a_key() {
    let metrics = Arc::new(Metrics::default());
    let missing = Files::new(Mode::Enforce);
    assert!(ClaktoryAccess::start(&missing.config, metrics.clone()).is_err(), "no key file: no start");
    let no_denylist = Files::new(Mode::Enforce).with_keys(1);
    assert!(ClaktoryAccess::start(&no_denylist.config, metrics.clone()).is_err(), "no denylist: no start");
    let observing = Files::new(Mode::Observe);
    let access = ClaktoryAccess::start(&observing.config, metrics.clone()).unwrap();
    assert_eq!(access.mode(), Mode::Observe);
    assert!(access.set_mode(Mode::Enforce).is_err(), "never enforce without a key");
    assert_eq!(access.mode(), Mode::Observe);
    let ready = Files::new(Mode::Enforce).with_keys(1).with_denylist(1, &[]);
    assert_eq!(ClaktoryAccess::start(&ready.config, metrics).unwrap().mode(), Mode::Enforce);
}

#[tokio::test]
async fn a_file_replaced_by_rename_is_read_again() {
    let files = Files::new(Mode::Observe).with_keys(1).with_denylist(1, &[]);
    let access = access_at_now(&files);
    let me = endpoint(1);
    access.on_connect(&request(&me, Some(&token_for(me.public())))).await;
    tokio::spawn(access.clone().watch_every(files.config.clone(), Duration::from_millis(20)));
    tokio::time::sleep(Duration::from_millis(60)).await;
    files.write_denylist(2, &[ACCOUNT]);
    let lifted = tokio::time::timeout(Duration::from_secs(5), async {
        while access.is_authorized(me.public()) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    });
    lifted.await.expect("the denylist is read again");
    // And again after a second rename.
    let rotated = SecretKey::from_bytes(&[9; 32]);
    files.write_keys(2, &rotated);
    tokio::time::timeout(Duration::from_secs(5), async {
        while access.keys.read().unwrap().as_ref().unwrap().keys != [rotated.public()] {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the key file is read again");
}

/// A relay on this computer with `access`, and its URL.
async fn relay(access: Arc<ClaktoryAccess>) -> (Server, RelayUrl) {
    let mut relay = RelayConfig::new((Ipv4Addr::LOCALHOST, 0));
    relay.access = access;
    let mut config = ServerConfig::default();
    config.relay = Some(relay);
    let server = Server::spawn(config).await.unwrap();
    let url = format!("http://{}", server.http_addr().unwrap()).parse().unwrap();
    (server, url)
}

async fn client(url: &RelayUrl, key: &SecretKey, token: Option<String>) -> iroh_relay::client::Client {
    let tls = iroh_relay::tls::CaTlsConfig::default().client_config(iroh_relay::tls::default_provider()).unwrap();
    let mut builder = ClientBuilder::new(url.clone(), key.clone(), iroh_dns::dns::DnsResolver::new()).tls_client_config(tls);
    if let Some(token) = token {
        builder = builder.auth_token(token);
    }
    builder.connect().await.unwrap()
}

/// Whether `to` gets what `from` sends it, within a second and a half.
async fn reaches(from: &mut iroh_relay::client::Client, to: &mut iroh_relay::client::Client, to_key: EndpointId) -> bool {
    let message = Datagrams::from(&b"hello"[..]);
    for _ in 0..3 {
        from.send(ClientToRelayMsg::Datagrams { dst_endpoint_id: to_key, datagrams: message.clone() }).await.unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
        while let Ok(Some(received)) = tokio::time::timeout_at(deadline, to.next()).await {
            if let Ok(RelayToClientMsg::Datagrams { datagrams, .. }) = received
                && datagrams == message
            {
                return true;
            }
        }
    }
    false
}

/// The rule on a real relay: iroh's client with and without a token.
#[tokio::test]
async fn the_relay_forwards_only_to_or_from_a_signed_in_claktory() {
    let files = Files::new(Mode::Enforce).with_keys(1).with_denylist(1, &[]);
    // A real clock here: the token is made for now.
    let access = Arc::new(ClaktoryAccess::new(Arc::default()));
    access.load_keys(&files.config.keys).unwrap();
    access.load_denylist(&files.config.denylist).unwrap();
    access.set_mode(Mode::Enforce).unwrap();
    let (server, url) = relay(access.clone()).await;

    let now = unix_now();
    let (claktory, phone, stranger) = (endpoint(1), endpoint(2), endpoint(3));
    let token = signed(&signer(), &payload(claktory.public(), ACCOUNT, now, now + 7 * DAY));
    let mut claktory_client = client(&url, &claktory, Some(token)).await;
    let mut phone_client = client(&url, &phone, None).await;
    let mut stranger_client = client(&url, &stranger, None).await;
    assert!(access.is_authorized(claktory.public()));

    assert!(reaches(&mut phone_client, &mut claktory_client, claktory.public()).await, "a phone reaches its signed-in Claktory");
    assert!(reaches(&mut claktory_client, &mut phone_client, phone.public()).await, "and hears back");
    assert!(!reaches(&mut stranger_client, &mut phone_client, phone.public()).await, "two others don't reach each other");
    assert!(access.metrics.forward_denied_packets.get() >= 3);

    access.set_mode(Mode::Observe).unwrap();
    assert!(reaches(&mut stranger_client, &mut phone_client, phone.public()).await, "observing forwards it");

    drop((claktory_client, phone_client, stranger_client));
    server.shutdown().await.unwrap();
}

/// Cioccolata's vector from the account service (the issuer), byte for byte.
#[test]
fn the_issuers_test_vector_verifies() {
    const TOKEN: &str = "eyJ2IjoxLCJlIjoiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYmFiYWJhYiIsImEiOiIwZjBmMGYwZjBmMGYwZjBmMGYwZjBmMGYwZjBmMGYwZiIsImlhdCI6MTc5NTE4MzMyMCwiZXhwIjoxNzk1Nzg4MTIwfQ.8Nxh_AlyH94f0DFSTGxUGAEBPCUhNMDl9WYu8kboyeC1kXtP5iQnLaUcsVUOAeaYWQAalSVasysxRNU_Jtt0CQ";
    let keys = parse_keys(br#"{"seq":1,"keys":["ea4a6c63e29c520abef5507b132ec5f9954776aebebe7b92421eea691446d22c"]}"#).unwrap();
    assert_eq!(keys.keys, [signer().public()], "the same seed");
    assert_eq!(TOKEN.len(), 289);
    assert_eq!(token_for_vector(), TOKEN, "made the same way here");
    let now = 1_795_183_320 + 3600;
    match EndpointId::from_bytes(&[0xab; 32]) {
        Ok(bound) => assert_eq!(verify(TOKEN, bound, &keys.keys, &HashSet::new(), now), Ok([0x0f; 16])),
        // Not a point on the curve: no endpoint can have it, so the
        // vector checks up to the binding.
        Err(_) => assert_eq!(verify(TOKEN, endpoint(1).public(), &keys.keys, &HashSet::new(), now), Err(Rejected::Endpoint)),
    }
    assert_eq!(verify(TOKEN, endpoint(1).public(), &keys.keys, &HashSet::new(), 1_795_788_120), Err(Rejected::Expired));
}

fn token_for_vector() -> String {
    signed(
        &signer(),
        r#"{"v":1,"e":"abababababababababababababababababababababababababababababababab","a":"0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f0f","iat":1795183320,"exp":1795788120}"#,
    )
}
