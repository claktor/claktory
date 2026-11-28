//! Frames (#435, Fugo 40527): a frame of the page's own site is read and
//! acted on as the page is; another site's shows only as its place, but
//! for one that covers the page, which dismiss may clear; sign-in,
//! payment and check services never.

use std::sync::Arc;

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};

use super::super::cdp::{Cdp, Local, Transport, find_chrome};
use super::tests::TestHand;
use super::*;

/// Sites on this machine, over https with a certificate of their own:
/// `pages` is (host, path, html), PORT in it the port they're on. Its port.
pub(crate) async fn served(pages: Vec<(&'static str, &'static str, String)>) -> u16 {
    let mut names: Vec<String> = pages.iter().map(|(host, _, _)| (*host).to_owned()).collect();
    names.dedup();
    let made = rcgen::generate_simple_self_signed(names).unwrap();
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(made.signing_key.serialize_der()));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(vec![CertificateDer::from(made.cert.der().to_vec())], key)
        .unwrap();
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let pages: Vec<(&str, &str, String)> = pages.into_iter().map(|(host, path, html)| (host, path, html.replace("PORT", &port.to_string()))).collect();
    let pages = Arc::new(pages);
    tokio::spawn(async move {
        while let Ok((socket, _)) = listener.accept().await {
            let (acceptor, pages) = (acceptor.clone(), pages.clone());
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(socket).await else { return };
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n") && head.len() < 16384 {
                    if tls.read(&mut byte).await.unwrap_or(0) == 0 {
                        return;
                    }
                    head.push(byte[0]);
                }
                let head = String::from_utf8_lossy(&head);
                let path = head.split_whitespace().nth(1).unwrap_or("/").to_owned();
                let host = head
                    .lines()
                    .find_map(|line| line.strip_prefix("Host: ").or_else(|| line.strip_prefix("host: ")))
                    .map(|host| host.split(':').next().unwrap_or_default().to_owned())
                    .unwrap_or_default();
                let page = pages.iter().find(|(h, p, _)| *h == host && *p == path).map(|(_, _, html)| html.clone());
                let (status, body) = match page {
                    Some(html) => ("200 OK", html),
                    None => ("404 Not Found", "<title>Not found</title>".to_owned()),
                };
                let kind = if path.ends_with(".pdf") { "application/pdf" } else { "text/html; charset=utf-8" };
                let answer = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = tls.write_all(answer.as_bytes()).await;
                let _ = tls.shutdown().await;
            });
        }
    });
    port
}

/// A real headless Chrome that finds every name on this machine and takes
/// its certificates, on `url`: None where there's no Chrome.
pub(crate) async fn page_at(url: &str) -> Option<(Local, Arc<Page>, std::path::PathBuf)> {
    let chrome = find_chrome()?;
    super::tests::room_for_chromes();
    let profile = std::env::temp_dir().join(format!("factoryd-frames-{}-{}", std::process::id(), rand_suffix()));
    std::fs::create_dir_all(&profile).unwrap();
    let more = ["--host-resolver-rules=MAP * 127.0.0.1".to_owned(), "--ignore-certificate-errors".to_owned()];
    let local = Local::start_with(&chrome, &profile, &more).unwrap();
    let cdp = Arc::new(Cdp::new(local.connect().unwrap()));
    let targets = cdp.call("Target.getTargets", json!({}), None).await.unwrap();
    let target = targets["targetInfos"].as_array().unwrap().iter().find(|t| t["type"] == "page").unwrap()["targetId"].clone();
    let attached = cdp.call("Target.attachToTarget", json!({"targetId": target, "flatten": true}), None).await.unwrap();
    let page = Arc::new(Page::new(cdp, attached["sessionId"].as_str().unwrap().to_owned()));
    page.call("Page.enable", json!({})).await.unwrap();
    page.call("Page.navigate", json!({"url": url})).await.unwrap();
    // Loaded, frames and all.
    for _ in 0..100 {
        let state = page.call("Runtime.evaluate", json!({"expression": "document.readyState", "returnByValue": true})).await;
        if state.is_ok_and(|state| state["result"]["value"] == "complete") {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    Some((local, page, profile))
}

fn rand_suffix() -> u64 {
    let mut bytes = [0u8; 8];
    getrandom::fill(&mut bytes).unwrap();
    u64::from_le_bytes(bytes)
}

#[test]
fn a_frames_site_is_its_registrable_domain_over_https() {
    assert!(frames::same_site("https://www.kettles.test/", "https://pay.kettles.test:8443/form"));
    assert!(frames::same_site("https://shop.example.co.uk/", "https://checkout.example.co.uk/"));
    assert!(!frames::same_site("https://shop.example.co.uk/", "https://other.co.uk/"));
    assert!(!frames::same_site("https://www.kettles.test/", "http://pay.kettles.test/"), "https only");
    assert!(!frames::same_site("https://a.github.io/", "https://b.github.io/"), "the list's private section");
    assert!(!frames::same_site("about:blank", "about:blank"));
}

fn frame<'a>(page: &'a schema::Page, origin: &str) -> &'a schema::Frame {
    page.frames.iter().find(|frame| frame.origin.starts_with(origin)).unwrap_or_else(|| panic!("{origin} in {:?}", page.frames))
}

#[tokio::test]
async fn the_sites_own_frame_is_read_and_filled_and_anothers_shows_only_where_it_is() {
    let port = served(vec![
        (
            "www.kettles.test",
            "/",
            r#"<!doctype html><title>Kettles</title><h1>Kettles</h1>
            <iframe src="https://pay.kettles.test:PORT/form" width=400 height=160></iframe>
            <iframe src="https://ads.other.test:PORT/ad" width=300 height=60></iframe>
            <iframe src="https://js.stripe.com:PORT/card" width=300 height=40></iframe>
            <iframe src="https://accounts.google.com:PORT/signin" width=300 height=40></iframe>"#
                .to_owned(),
        ),
        (
            "pay.kettles.test",
            "/form",
            r#"<title>Delivery</title><p>Where to?</p><label>Name <input id=name></label>
            <button onclick="document.title = 'sent ' + document.getElementById('name').value + ' ' + event.isTrusted">Send</button>"#
                .to_owned(),
        ),
        ("ads.other.test", "/ad", "<title>Ad</title><p>Buy a toaster</p><a href=\"/buy\">Buy now</a>".to_owned()),
        ("js.stripe.com", "/card", "<title>Card</title><label>Card number <input></label>".to_owned()),
        ("accounts.google.com", "/signin", "<title>Sign in</title><label>Email <input></label>".to_owned()),
    ])
    .await;
    let Some((_local, page, profile)) = page_at(&format!("https://www.kettles.test:{port}/")).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(read.frames.len(), 4, "{:?}", read.frames);
    let own = frame(&read, "https://pay.kettles.test");
    assert!(own.same_site && own.text.as_deref().is_some_and(|text| text.contains("Where to?")), "{own:?}");
    assert_eq!(own.title.as_deref(), Some("Delivery"));
    let ad = frame(&read, "https://ads.other.test");
    assert!(!ad.same_site && ad.text.is_none() && ad.elements.is_empty() && ad.overlay.is_none(), "{ad:?}");
    assert!(ad.width == 300 && ad.height == 60, "{ad:?}");
    for theirs in ["https://js.stripe.com", "https://accounts.google.com"] {
        let frame = frame(&read, theirs);
        assert_eq!(frame.not_read.as_deref(), Some(UNTOUCHED_WHY), "{frame:?}");
        assert!(frame.text.is_none() && frame.elements.is_empty());
    }
    let json = serde_json::to_string(&read).unwrap();
    assert!(!json.contains("toaster") && !json.contains("Card number") && !json.contains("Email"), "{json}");

    // Its field is filled and its button pressed as the page's are.
    let name = own.elements.iter().find(|element| element.name.contains("Name")).unwrap().reference.clone();
    let send = own.elements.iter().find(|element| element.name == "Send").unwrap().reference.clone();
    let filled = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &name, Some("Ada")).unwrap()).await.unwrap();
    assert!(matches!(filled, Answer::Filled(schema::Filled { took: true, .. })), "{filled:?}");
    page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &send, None).unwrap()).await.unwrap();
    let Ok(Answer::Page(again)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(frame(&again, "https://pay.kettles.test").title.as_deref(), Some("sent Ada true"));
    // A ref of the frame's earlier read is stale once read again.
    let stale = page.act(&mut hands.hand(), &Op::on_element(&again.document, 0, &send, None).unwrap()).await.unwrap_err();
    assert_eq!(stale.0, "stale_target", "{stale:?}");
    let _ = std::fs::remove_dir_all(&profile);
}

/// A page another site's frame covers whole: `inside` is the frame's.
async fn covered_by(host: &'static str, inside: &str) -> Option<(Local, Arc<Page>, std::path::PathBuf)> {
    let port = served(vec![
        (
            "www.shop.test",
            "/",
            r#"<!doctype html><title>Shop</title><h1>Shop</h1><p>Kettles and toasters.</p>
            <iframe id=cmp src="https://HOST:PORT/notice" style="position:fixed;inset:0;width:100%;height:100%;border:0"></iframe>
            <script>addEventListener('message', (e) => { window.picked = e.data; document.getElementById('cmp').remove(); });</script>"#
                .replace("HOST", host),
        ),
        (host, "/notice", inside.to_owned()),
    ])
    .await;
    page_at(&format!("https://www.shop.test:{port}/")).await
}

#[tokio::test]
async fn another_sites_consent_frame_is_refused_never_accepted() {
    let notice = r#"<title>Privacy</title><p>We and our partners use cookies.</p>
        <button onclick="parent.postMessage('accept', '*')">Accept all</button>
        <button onclick="parent.postMessage('reject', '*')">Reject all</button>"#;
    let Some((_local, page, profile)) = covered_by("consent.cmp.test", notice).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(read.overlays, ["cookie notice"]);
    let cmp = frame(&read, "https://consent.cmp.test");
    assert_eq!(cmp.overlay.as_deref(), Some("cookie notice"));
    assert!(cmp.text.is_none() && cmp.elements.is_empty(), "only its place: {cmp:?}");
    let Answer::Dismissed(done) = page.dismiss(&mut hands.hand(), &read.document).await.unwrap() else { panic!("dismissed") };
    assert_eq!(done.pressed, ["Reject all"]);
    assert!(!done.accepted_cookies && done.gone, "{done:?}");
    assert_eq!(super::tests::seen(&page, "window.picked").await, "reject");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_sign_in_frame_over_the_page_is_never_touched() {
    let sign_in = r#"<title>Sign in</title><p>Use your account.</p><button onclick="parent.postMessage('closed', '*')">Close</button>"#;
    let Some((_local, page, profile)) = covered_by("accounts.google.com", sign_in).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(frame(&read, "https://accounts.google.com").not_read.as_deref(), Some(UNTOUCHED_WHY));
    let refused = page.dismiss(&mut hands.hand(), &read.document).await.unwrap_err();
    assert_eq!(refused.0, "sensitive_action", "{refused:?}");
    assert_eq!(super::tests::seen(&page, "window.picked === undefined").await, true);
    let _ = std::fs::remove_dir_all(&profile);
}

/// A PDF the agent asks for comes as a file, not into Chrome's viewer
/// (Ghiaccio 40395): its response goes on as an attachment.
#[tokio::test]
async fn a_pdf_asked_for_is_saved_not_shown() {
    let pdf = "%PDF-1.4\n1 0 obj<<>>endobj\ntrailer<<>>\n%%EOF\n";
    let port = served(vec![
        ("www.kettles.test", "/", "<title>Manuals</title><p>Kettle manuals.</p>".to_owned()),
        ("www.kettles.test", "/manual.pdf", pdf.to_owned()),
    ])
    .await;
    let Some((_local, page, profile)) = page_at(&format!("https://www.kettles.test:{port}/")).await else { return };
    let folder = profile.with_extension("downloads");
    std::fs::create_dir_all(&folder).unwrap();
    let behavior = json!({"behavior": "allowAndName", "downloadPath": folder.display().to_string(), "eventsEnabled": true});
    page.cdp().call("Browser.setDownloadBehavior", behavior, None).await.unwrap();
    let patterns = json!({"patterns": [{"urlPattern": "*", "resourceType": "Document", "requestStage": "Response"}]});
    page.call("Fetch.enable", patterns).await.unwrap();
    let mut events = page.cdp().events();
    let paused = {
        let (page, mut events) = (page.clone(), page.cdp().events());
        tokio::spawn(async move {
            while let Ok(event) = events.recv().await {
                if event.method == "Fetch.requestPaused" && event.session.as_deref() == Some(page.session()) {
                    let request = event.params["requestId"].as_str().unwrap().to_owned();
                    crate::errands::fence::let_through(&page, &request, &event.params, true).await;
                }
            }
        })
    };
    page.call("Page.navigate", json!({"url": format!("https://www.kettles.test:{port}/manual.pdf")})).await.unwrap();
    let guid = tokio::time::timeout(std::time::Duration::from_secs(15), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.method == "Browser.downloadProgress" && event.params["state"] == "completed" {
                return event.params["guid"].as_str().unwrap().to_owned();
            }
        }
    })
    .await
    .expect("a download");
    assert_eq!(std::fs::read_to_string(folder.join(guid)).unwrap(), pdf);
    assert!(!page.frame().await.unwrap().url.ends_with(".pdf"), "not shown in the viewer");
    paused.abort();
    let _ = std::fs::remove_dir_all(&folder);
    let _ = std::fs::remove_dir_all(&profile);
}
