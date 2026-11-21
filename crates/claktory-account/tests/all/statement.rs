//! The account root's word for the service key: served at well-known, and
//! checked as a Claktory checks it (`claktory-account check-statement`, for
//! deploys). With a root of the test's own: the pinned ones sign offline.

use std::sync::Arc;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use claktory_account::protocol::{KeyStatement, SignedStatement, b64};
use claktory_account::server::{self, Config, Secrets, Service, mail, statement};
use ed25519_dalek::{Signer, SigningKey};

fn root() -> (SigningKey, Vec<(&'static str, String)>) {
    let root = SigningKey::from_bytes(&[3; 32]);
    let roots = vec![("test-root", STANDARD.encode(root.verifying_key().to_bytes()))];
    (root, roots)
}

fn sign(root: &SigningKey, statement: &KeyStatement) -> SignedStatement {
    let text = statement.text();
    SignedStatement { signature: format!("test-root:{}", STANDARD.encode(root.sign(text.as_bytes()).to_bytes())), text }
}

/// A service at a port of its own, serving what `statement` makes of its
/// address and key.
async fn serving(statement: impl FnOnce(&str, &str) -> Option<SignedStatement>) -> String {
    let dir = tempfile::tempdir().expect("a folder");
    let secrets = Secrets::generate();
    let key = b64(secrets.signing_key.verifying_key().to_bytes());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let url = format!("http://{}", listener.local_addr().expect("an address"));
    let config = Config { service_key_statement: statement(&url, &key), ..Config::default() };
    let service = Service::new(&dir.path().join("account.db"), secrets, config, Arc::new(mail::Memory::default()), server::system_clock())
        .expect("the service");
    tokio::spawn(async move {
        let _dir = dir;
        server::serve(listener, service).await
    });
    url
}

#[tokio::test]
async fn the_deploy_check_holds_only_for_a_statement_a_root_signed_for_this_service_and_its_key() {
    let (root, owned) = root();
    let roots: Vec<(&str, &str)> = owned.iter().map(|(id, key)| (*id, key.as_str())).collect();
    let good = |url: &str, key: &str| KeyStatement { service: url.to_owned(), key: key.to_owned(), from: 1_795_000_000 };

    let url = serving(|url, key| Some(sign(&root, &good(url, key)))).await;
    let held = statement::check(&format!("{url}/"), &roots).await.expect("it holds");
    assert_eq!((held.service, held.from), (url.clone(), 1_795_000_000));

    // None served, another service's, another key's, another root's.
    let none = serving(|_, _| None).await;
    assert!(statement::check(&none, &roots).await.expect_err("none").contains("serves no service-key statement"));
    let elsewhere = serving(|_, key| Some(sign(&root, &good("https://account.claktory.com", key)))).await;
    assert!(statement::check(&elsewhere, &roots).await.expect_err("elsewhere").contains("is for https://account.claktory.com"));
    let other_key = b64(SigningKey::from_bytes(&[4; 32]).verifying_key().to_bytes());
    let wrong_key = serving(|url, _| Some(sign(&root, &good(url, &other_key)))).await;
    assert!(statement::check(&wrong_key, &roots).await.expect_err("key").contains("but it serves"));
    let stranger = SigningKey::from_bytes(&[5; 32]);
    let unsigned = serving(|url, key| Some(sign(&stranger, &good(url, key)))).await;
    assert!(statement::check(&unsigned, &roots).await.expect_err("root").contains("doesn't match"));
}

#[test]
fn the_service_reads_only_a_statement_for_its_own_key() {
    let (root, owned) = root();
    let roots: Vec<(&str, &str)> = owned.iter().map(|(id, key)| (*id, key.as_str())).collect();
    let dir = tempfile::tempdir().expect("a folder");
    let path = dir.path().join("statement.json");
    let statement = KeyStatement { service: "https://account.claktory.com".into(), key: b64([8u8; 32]), from: 1_795_000_000 };
    std::fs::write(&path, serde_json::to_string(&sign(&root, &statement)).unwrap()).unwrap();
    assert!(statement::read(&path, &statement.key, &roots).is_ok());
    assert!(statement::read(&path, &b64([9u8; 32]), &roots).is_err(), "another key's");
    assert!(statement::read(&path, &statement.key, &[]).is_err(), "no root signed it");
}
