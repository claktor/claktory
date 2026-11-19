//! Factory's model connections get through a TLS handshake in factoryd itself,
//! which links two rustls crypto providers (ring for the model providers,
//! aws-lc-rs for remote access): code asking rustls for the process default
//! panics there, as the Codex WebSocket once did, killing every request.

use std::sync::Arc;
use std::time::Duration;

use factory_agent::providers::openai_codex::stream_simple_with;
use factory_agent::stream::{LlmContext, StreamOptions, Transport};
use factory_agent::types::{StopReason, StreamEvent};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

/// A TLS server whose certificate no client trusts: every handshake gets as far
/// as checking it, and fails there.
async fn untrusted_server() -> u16 {
    let made = rcgen::generate_simple_self_signed(vec!["localhost".to_owned()]).expect("certificate");
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.signing_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(made.cert.der().to_vec())], key)
        .expect("server config");
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().expect("address").port();
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let _ = acceptor.accept(socket).await;
            });
        }
    });
    port
}

/// A ChatGPT token naming an account, as Codex requests need; unsigned.
fn token() -> String {
    let part = |json: &str| {
        const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
        let bits: Vec<bool> = json.bytes().flat_map(|byte| (0..8).rev().map(move |bit| byte >> bit & 1 == 1)).collect();
        bits.chunks(6)
            .map(|six| ALPHABET[six.iter().chain(std::iter::repeat(&false)).take(6).fold(0, |n, &bit| n << 1 | usize::from(bit))] as char)
            .collect::<String>()
    };
    format!("{}.{}.unsigned", part(r#"{"alg":"none"}"#), part(r#"{"https://api.openai.com/auth":{"chatgpt_account_id":"acct_tls"}}"#))
}

#[tokio::test]
async fn codex_over_websocket_and_sse_reaches_the_tls_handshake() {
    let port = untrusted_server().await;
    let mut model = factory_agent::providers::catalog::models("openai-codex").into_iter().next().expect("a Codex model");
    model.base_url = format!("https://localhost:{port}/backend-api");
    // Auto: the WebSocket first, then SSE when it can't connect.
    let options = StreamOptions { api_key: Some(token()), session_id: Some("tls".into()), ..StreamOptions::default() };
    let mut stream = stream_simple_with(&model, LlmContext::default(), options, Transport::Auto).expect("stream");
    let mut last = None;
    while let Some(event) = tokio::time::timeout(Duration::from_secs(60), stream.next()).await.expect("the request ends") {
        last = Some(event);
    }
    // A provider task that panicked ends the stream with no terminal event.
    let Some(StreamEvent::Error { reason, error }) = last else { panic!("no terminal event: {last:?}") };
    assert_eq!(reason, StopReason::Error);
    // The WebSocket failed checking the certificate, so it got that far, and SSE then tried too.
    let tried = &error.extra["diagnostics"][0];
    assert_eq!(tried["details"]["fallbackTransport"], "sse", "{tried}");
    let failure = tried["error"]["message"].as_str().unwrap_or_default().to_lowercase();
    assert!(failure.contains("certificate"), "{failure}");
    assert!(error.error_message.as_deref().is_some_and(|message| message.starts_with("error sending request")), "{error:?}");
}
