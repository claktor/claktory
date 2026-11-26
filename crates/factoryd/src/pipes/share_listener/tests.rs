//! The receiving side's checks, against a stand-in for the sharer: a
//! website on this computer's loopback that says what reached it.

use std::sync::atomic::{AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpStream;

use super::*;

/// The sharer's website: answers each request with what reached it, sets
/// cookies at /login, and echoes bytes after an upgrade.
async fn website() -> SocketAddr {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else { return };
            tokio::spawn(async move {
                let service = hyper::service::service_fn(|mut request: Request<Incoming>| async move {
                    let header = |name| request.headers().get(name).and_then(|value: &HeaderValue| value.to_str().ok()).unwrap_or("").to_owned();
                    let said = format!("host={}\ncookie={}\npath={}", header(header::HOST), header(header::COOKIE), request.uri().path());
                    let mut response = Response::new(Full::new(Bytes::from(said)));
                    if request.uri().path() == "/login" {
                        let headers = response.headers_mut();
                        headers.append(header::SET_COOKIE, HeaderValue::from_static("site=1; Domain=localhost; Path=/"));
                        headers.append(header::SET_COOKIE, HeaderValue::from_static("claktory_s_other=stolen; Path=/"));
                        headers.insert("clear-site-data", HeaderValue::from_static("\"cookies\", \"storage\""));
                    }
                    if request.headers().contains_key(header::UPGRADE) {
                        let upgrade = hyper::upgrade::on(&mut request);
                        tokio::spawn(async move {
                            let Ok(upgraded) = upgrade.await else { return };
                            let mut upgraded = TokioIo::new(upgraded);
                            let mut buffer = [0_u8; 64];
                            while let Ok(read) = upgraded.read(&mut buffer).await {
                                if read == 0 || upgraded.write_all(&buffer[..read]).await.is_err() {
                                    return;
                                }
                            }
                        });
                        *response.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
                        response.headers_mut().insert(header::UPGRADE, HeaderValue::from_static("echo"));
                        response.headers_mut().insert(header::CONNECTION, HeaderValue::from_static("upgrade"));
                    }
                    Ok::<_, Infallible>(response)
                });
                let _ = hyper::server::conn::http1::Builder::new().serve_connection(TokioIo::new(socket), service).with_upgrades().await;
            });
        }
    });
    address
}

struct Share {
    gate: Arc<Gate>,
    port: u16,
    cancel: CancellationToken,
    /// How many streams to the sharer were opened.
    streams: Arc<AtomicUsize>,
}

/// A share's listener whose sharer is `website`, at `s-test.localhost`.
async fn listen() -> Share {
    let site = website().await;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let gate = Arc::new(Gate {
        share: "share-1".into(),
        host: "s-test".into(),
        port,
        keys: Mutex::new(Vec::new()),
        cookies: Mutex::new(BTreeSet::new()),
        codes: Mutex::new(Vec::new()),
        save: Box::new(|_: &Gate| {}),
    });
    let streams = Arc::new(AtomicUsize::new(0));
    let counted = streams.clone();
    let connect: Connect = Arc::new(move || {
        counted.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            let socket = TcpStream::connect(site).await.map_err(|error| error.to_string())?;
            Ok(Box::new(socket) as Box<dyn Io>)
        })
    });
    let cancel = CancellationToken::new();
    tokio::spawn(serve(listener, gate.clone(), connect, cancel.clone()));
    Share { gate, port, cancel, streams }
}

/// A request as a browser makes it: (status, headers, body).
async fn get(port: u16, host: &str, path: &str, cookie: Option<&str>) -> (u16, String, String) {
    send(port, "GET", host, path, cookie, &[]).await
}

/// A request with `method` and more headers.
async fn send(port: u16, method: &str, host: &str, path: &str, cookie: Option<&str>, more: &[(&str, &str)]) -> (u16, String, String) {
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
    let cookie = cookie.map(|cookie| format!("Cookie: {cookie}\r\n")).unwrap_or_default();
    let more: String = more.iter().map(|(name, value)| format!("{name}: {value}\r\n")).collect();
    let length = if method == "POST" { "Content-Length: 0\r\n" } else { "" };
    let request = format!("{method} {path} HTTP/1.1\r\nHost: {host}\r\n{cookie}{more}{length}Connection: close\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut answer = String::new();
    socket.read_to_string(&mut answer).await.unwrap();
    let (head, body) = answer.split_once("\r\n\r\n").unwrap_or((&answer, ""));
    let status = head.split(' ').nth(1).and_then(|status| status.parse().ok()).unwrap_or(0);
    (status, head.to_owned(), body.to_owned())
}

/// Opens the share as the browser would: its key cookie, `name=value`.
async fn open(share: &Share, host: &str) -> String {
    let code = share.gate.mint();
    let (status, head, _) = get(share.port, host, &format!("/__claktory_open?k={code}"), None).await;
    assert_eq!(status, 303, "{head}");
    assert!(head.contains("Location: /\r\n") || head.contains("location: /\r\n"), "{head}");
    let set = head.lines().find(|line| line.to_ascii_lowercase().starts_with("set-cookie:")).expect("a cookie").to_owned();
    assert!(set.contains("HttpOnly") && set.contains("SameSite=Strict") && set.contains("Path=/"), "{set}");
    set.split_once(':').unwrap().1.trim().split(';').next().unwrap().to_owned()
}

#[tokio::test]
async fn only_the_shares_own_host_with_its_key_reaches_the_sharer() {
    let share = listen().await;
    let own = format!("s-test.localhost:{}", share.port);

    // Another name for this computer (DNS rebinding), or none of the
    // share's: refused before anything else.
    let refused = [
        format!("evil.example:{}", share.port),
        format!("localhost:{}", share.port),
        format!("s-testx.localhost:{}", share.port),
        format!("x.s-test.localhost:{}", share.port),
        "s-test.localhost".to_owned(),
        "s-test.localhost:0".to_owned(),
        "s-test.localhost:080".to_owned(),
        "s-test.localhost:65536".to_owned(),
        "s-test.localhost:+80".to_owned(),
        "s-test.localhost:".to_owned(),
        "127.0.0.1".to_owned(),
        format!("[::1]:{}", share.port),
    ];
    for host in refused {
        assert_eq!(get(share.port, &host, "/", None).await.0, 421, "{host}");
    }
    // W4: each refusal says which it is.
    let (_, _, body) = get(share.port, &format!("localhost:{}", share.port), "/callback", None).await;
    assert!(body.contains(&format!("This page went to localhost:{}, which isn't part of the shared website", share.port)), "{body}");
    let (_, _, body) = send(share.port, "GET", &own, "/", None, &[("Sec-Fetch-Site", "cross-site")]).await;
    assert!(body.contains("not from a link on another site"), "{body}");
    // The right host without the key: Claktory's own page, and nothing
    // reaches the sharer.
    let (status, _, body) = get(share.port, &own, "/", None).await;
    assert_eq!(status, 403);
    assert!(body.contains("This website opens only from Claktory. Choose Open on it in Claktory"), "{body}");
    assert_eq!(get(share.port, &own, "/", Some("claktory_s_test=guessed")).await.0, 403);
    assert_eq!(share.streams.load(Ordering::SeqCst), 0);

    // A code that isn't one, then a real one, which works once.
    assert_eq!(get(share.port, &own, "/__claktory_open?k=nope", None).await.0, 403);
    let code = share.gate.mint();
    assert_eq!(get(share.port, &own, &format!("/__claktory_open?k={code}"), None).await.0, 303);
    assert_eq!(get(share.port, &own, &format!("/__claktory_open?k={code}"), None).await.0, 403);
    assert_eq!(share.streams.load(Ordering::SeqCst), 0, "the open path is never passed on");

    // With the key: the sharer's website, the Host as the browser sent it,
    // and the key never passed on.
    let key = open(&share, &own).await;
    let (status, _, body) = get(share.port, &own, "/page", Some(&format!("theme=dark; {key}; claktory_s_other=x"))).await;
    assert_eq!(status, 200, "{body}");
    assert_eq!(body, format!("host={own}\ncookie=theme=dark\npath=/page"));
    assert_eq!(share.streams.load(Ordering::SeqCst), 1);

    // Its cookies reach the browser, never one named as Claktory's.
    let (_, head, _) = get(share.port, &own, "/login", Some(&key)).await;
    assert!(head.contains("site=1; Path=/"), "{head}");
    assert!(!head.to_ascii_lowercase().contains("domain"), "{head}");
    assert!(!head.to_ascii_lowercase().contains("clear-site-data"), "{head}");
    assert!(!head.contains("claktory_s_other"), "{head}");
}

#[tokio::test]
async fn on_127_0_0_1_only_the_websites_own_cookies_pass() {
    let share = listen().await;
    let fallback = format!("127.0.0.1:{}", share.port);
    let key = open(&share, &fallback).await;
    // Another local service's cookie, sent for 127.0.0.1 whatever its port:
    // kept back, until the website sets one of that name itself.
    let (_, _, body) = get(share.port, &fallback, "/", Some(&format!("{key}; site=1; session=other"))).await;
    assert_eq!(body, format!("host={fallback}\ncookie=\npath=/"));
    get(share.port, &fallback, "/login", Some(&key)).await;
    let (_, _, body) = get(share.port, &fallback, "/", Some(&format!("{key}; site=1; session=other"))).await;
    assert_eq!(body, format!("host={fallback}\ncookie=site=1\npath=/"));
}

/// An upgrade (a WebSocket, a dev server's live reload) as a browser asks
/// for it from the share's own page: the answer's status, then what comes
/// back of `say`.
async fn upgrade(port: u16, host: &str, cookie: Option<&str>, say: &[u8]) -> (u16, Vec<u8>, TcpStream) {
    upgrade_from(port, host, Some(&format!("http://{host}")), cookie, say).await
}

/// The same from a page at `origin` (None: no Origin).
async fn upgrade_from(port: u16, host: &str, origin: Option<&str>, cookie: Option<&str>, say: &[u8]) -> (u16, Vec<u8>, TcpStream) {
    let mut socket = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).await.unwrap();
    let cookie = cookie.map(|cookie| format!("Cookie: {cookie}\r\n")).unwrap_or_default();
    let origin = origin.map(|origin| format!("Origin: {origin}\r\n")).unwrap_or_default();
    let request = format!("GET /live HTTP/1.1\r\nHost: {host}\r\n{cookie}{origin}Connection: Upgrade\r\nUpgrade: echo\r\n\r\n");
    socket.write_all(request.as_bytes()).await.unwrap();
    let mut head = Vec::new();
    while !head.ends_with(b"\r\n\r\n") {
        let mut byte = [0_u8; 1];
        if socket.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        head.push(byte[0]);
    }
    let head = String::from_utf8_lossy(&head).to_string();
    let status = head.split(' ').nth(1).and_then(|status| status.parse().ok()).unwrap_or(0);
    if status != 101 {
        return (status, Vec::new(), socket);
    }
    socket.write_all(say).await.unwrap();
    let mut back = vec![0_u8; say.len()];
    socket.read_exact(&mut back).await.unwrap();
    (status, back, socket)
}

#[tokio::test]
async fn upgrades_pass_the_same_checks_and_end_with_the_share() {
    let share = listen().await;
    let own = format!("s-test.localhost:{}", share.port);
    assert_eq!(upgrade(share.port, &own, None, b"hi").await.0, 403);
    assert_eq!(upgrade(share.port, &format!("evil.example:{}", share.port), None, b"hi").await.0, 421);
    let key = open(&share, &own).await;
    let (status, back, mut socket) = upgrade(share.port, &own, Some(&key), b"hello").await;
    assert_eq!((status, back.as_slice()), (101, &b"hello"[..]));

    // The share ends: the live connection closes, and nothing listens.
    share.cancel.cancel();
    let mut rest = Vec::new();
    let ended = tokio::time::timeout(Duration::from_secs(5), socket.read_to_end(&mut rest)).await;
    assert!(ended.is_ok(), "the upgraded connection outlived the share");
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(TcpStream::connect((Ipv4Addr::LOCALHOST, share.port)).await.is_err(), "still listening");
}

/// Fugo, #154: every 127.0.0.1 port is the same site to a browser, so
/// another share's page there would send this one's key along. Only the
/// share's own pages, and the person, get through.
#[tokio::test]
async fn another_sites_pages_never_reach_the_share_even_with_its_key() {
    let share = listen().await;
    let fallback = format!("127.0.0.1:{}", share.port);
    let key = open(&share, &fallback).await;
    let elsewhere = format!("http://127.0.0.1:{}", share.port + 1);
    // A WebSocket, a POST and a fetch from another port's page, the key sent along.
    assert_eq!(upgrade_from(share.port, &fallback, Some(&elsewhere), Some(&key), b"hi").await.0, 403);
    assert_eq!(upgrade_from(share.port, &fallback, None, Some(&key), b"hi").await.0, 403);
    assert_eq!(send(share.port, "POST", &fallback, "/act", Some(&key), &[("Origin", &elsewhere)]).await.0, 403);
    assert_eq!(send(share.port, "GET", &fallback, "/", Some(&key), &[("Sec-Fetch-Site", "same-site")]).await.0, 403);
    assert_eq!(send(share.port, "GET", &fallback, "/", Some(&key), &[("Sec-Fetch-Site", "cross-site")]).await.0, 403);
    assert_eq!(send(share.port, "POST", &fallback, "/act", Some(&key), &[("Origin", "null")]).await.0, 403);
    assert_eq!(share.streams.load(Ordering::SeqCst), 0, "nothing reached the sharer");
    // Its own pages, and the person, do.
    let own = format!("http://{fallback}");
    assert_eq!(send(share.port, "POST", &fallback, "/act", Some(&key), &[("Origin", &own), ("Sec-Fetch-Site", "same-origin")]).await.0, 200);
    assert_eq!(send(share.port, "GET", &fallback, "/", Some(&key), &[("Sec-Fetch-Site", "none")]).await.0, 200);
    assert_eq!(upgrade(share.port, &fallback, Some(&key), b"hi").await.0, 101);
}

/// W1: a device showing this Claktory from another machine forwards a
/// port of its own to the share's, and its browser names that port: the
/// same share, with the same checks, its Host passed on as it came.
#[tokio::test]
async fn a_devices_forwarded_port_opens_the_same_share_with_the_same_checks() {
    let share = listen().await;
    let forwarded = 24_567_u16;
    assert_ne!(forwarded, share.port);
    let own = format!("s-test.localhost:{forwarded}");
    assert_eq!(get(share.port, &own, "/", None).await.0, 403, "the key, as ever");
    let key = open(&share, &own).await;
    let (status, _, body) = get(share.port, &own, "/page", Some(&key)).await;
    assert_eq!((status, body.as_str()), (200, format!("host={own}\ncookie=\npath=/page").as_str()));
    // Its own pages, at the forwarded port; never another port's.
    let page = format!("http://{own}");
    assert_eq!(send(share.port, "POST", &own, "/act", Some(&key), &[("Origin", &page)]).await.0, 200);
    let here = format!("http://s-test.localhost:{}", share.port);
    assert_eq!(send(share.port, "POST", &own, "/act", Some(&key), &[("Origin", &here)]).await.0, 403);
    assert_eq!(upgrade(share.port, &own, Some(&key), b"hi").await.0, 101);
    // Safari's, at the forwarded port: the website's own cookies only.
    let fallback = format!("127.0.0.1:{forwarded}");
    let key = open(&share, &fallback).await;
    let (_, _, body) = get(share.port, &fallback, "/", Some(&format!("{key}; session=the-devices-own"))).await;
    assert_eq!(body, format!("host={fallback}\ncookie=\npath=/"));
    // A phone's fallback is this address (Fugo 38931): every other port
    // of its 127.0.0.1 is another site, refused with the key, and its own
    // pages get through. The forward passes Host and Origin on untouched.
    let streams = share.streams.load(Ordering::SeqCst);
    let elsewhere = format!("http://127.0.0.1:{}", forwarded + 1);
    assert_eq!(send(share.port, "POST", &fallback, "/act", Some(&key), &[("Origin", &elsewhere)]).await.0, 403);
    assert_eq!(upgrade_from(share.port, &fallback, Some(&elsewhere), Some(&key), b"hi").await.0, 403);
    assert_eq!(upgrade_from(share.port, &fallback, None, Some(&key), b"hi").await.0, 403);
    assert_eq!(send(share.port, "GET", &fallback, "/", Some(&key), &[("Sec-Fetch-Site", "same-site")]).await.0, 403);
    assert_eq!(send(share.port, "GET", &fallback, "/", Some(&key), &[("Sec-Fetch-Site", "cross-site")]).await.0, 403);
    assert_eq!(share.streams.load(Ordering::SeqCst), streams, "nothing reached the sharer");
    let page = format!("http://{fallback}");
    assert_eq!(send(share.port, "POST", &fallback, "/act", Some(&key), &[("Origin", &page)]).await.0, 200);
    assert_eq!(upgrade(share.port, &fallback, Some(&key), b"hi").await.0, 101);
}

#[tokio::test]
async fn a_sharer_that_refuses_is_told_to_the_browser() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let gate = Arc::new(Gate {
        share: "share-2".into(),
        host: "s-two".into(),
        port,
        keys: Mutex::new(Vec::new()),
        cookies: Mutex::new(BTreeSet::new()),
        codes: Mutex::new(Vec::new()),
        save: Box::new(|_: &Gate| {}),
    });
    let connect: Connect = Arc::new(|| Box::pin(async { Err("Nothing is listening on port 3000 on Ana's Mac.".to_owned()) }));
    tokio::spawn(serve(listener, gate.clone(), connect, CancellationToken::new()));
    let own = format!("s-two.localhost:{port}");
    let code = gate.mint();
    let (_, head, _) = get(port, &own, &format!("/__claktory_open?k={code}"), None).await;
    let key = head.lines().find(|line| line.to_ascii_lowercase().starts_with("set-cookie:")).unwrap().split_once(':').unwrap().1;
    let key = key.trim().split(';').next().unwrap().to_owned();
    let (status, _, body) = get(port, &own, "/", Some(&key)).await;
    assert_eq!(status, 502);
    assert!(body.contains("Nothing is listening on port 3000 on Ana&#39;s Mac.") || body.contains("Nothing is listening on port 3000 on Ana's Mac."), "{body}");
}

#[test]
fn keys_and_codes_compare_whole() {
    assert!(same("abc", "abc"));
    assert!(!same("abc", "abd") && !same("abc", "ab") && !same("", "a"));
    let label = random_label();
    assert_eq!(label.len(), 20);
    assert!(label.bytes().all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()));
}
