use crate::support::*;

const FORM: &str = "application/x-www-form-urlencoded";

/// Signs in on the account page; the session cookie.
pub async fn web_sign_in(instance: &Instance, email: &str, ip: &str) -> String {
    let body = format!("email={}", email.replace('@', "%40")).into_bytes();
    let answer = instance.send("POST", "/account/sign-in", body, &[("content-type", FORM.into()), ("x-forwarded-for", ip.into())]).await;
    assert_eq!(answer.status, 200);
    let page = answer.text();
    assert!(page.contains("Enter the 6-digit code sent to"), "{page}");
    let request = field(&page, "request");
    let body = format!("request={request}&email={}&code={}", email.replace('@', "%40"), instance.last_code(email)).into_bytes();
    let answer = instance.send("POST", "/account/verify", body, &[("content-type", FORM.into()), ("x-forwarded-for", ip.into())]).await;
    assert_eq!(answer.status, 303, "{}", answer.text());
    let cookie = answer.headers.get("set-cookie").and_then(|value| value.to_str().ok()).expect("a cookie").to_owned();
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Strict") && cookie.contains("Secure"), "{cookie}");
    cookie.split(';').next().expect("the cookie").to_owned()
}

/// A hidden field's value on a page.
pub fn field(page: &str, name: &str) -> String {
    let marker = format!("name=\"{name}\" value=\"");
    let start = page.find(&marker).unwrap_or_else(|| panic!("no {name} in {page}")) + marker.len();
    page[start..].split('"').next().expect("a value").to_owned()
}

pub async fn get(instance: &Instance, path: &str, cookie: &str) -> Answer {
    instance.send("GET", path, Vec::new(), &[("cookie", cookie.into()), ("x-forwarded-for", "192.0.2.50".into())]).await
}

pub async fn post_form(instance: &Instance, path: &str, cookie: &str, body: &str) -> Answer {
    let headers = [("cookie", cookie.into()), ("content-type", FORM.into()), ("x-forwarded-for", "192.0.2.50".into())];
    instance.send("POST", path, body.as_bytes().to_vec(), &headers).await
}

#[tokio::test]
async fn the_account_page_lists_claktories_and_removes_one() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.7");
    instance.sign_in(&mut claktory, "matt@example.com", "Ana's Mac & co").await;
    instance.share(&claktory, None).await.expect("a share");

    let page = instance.send("GET", "/account", Vec::new(), &[]).await;
    assert!(page.text().contains("Sign in to Claktory"));
    assert!(page.headers.get("content-security-policy").is_some());

    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.50").await;
    let page = get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("<h2>Ana&#39;s Mac &amp; co</h2>"), "{page}");
    assert!(page.contains("v0.1.9 on macOS (Apple silicon) · last contact"));
    assert!(page.contains("20 Nov 2026, 14:02 UTC from 203.0.113.7"), "{page}");
    let csrf = field(&page, "csrf");

    // Without its CSRF token, nothing happens.
    post_form(&instance, "/account/remove", &cookie, &format!("claktory={}&confirm=yes", claktory.id)).await;
    instance.advance(60);
    assert!(instance.share(&claktory, None).await.is_ok(), "not removed");

    let confirm = post_form(&instance, "/account/remove", &cookie, &format!("csrf={csrf}&claktory={}", claktory.id)).await.text();
    assert!(confirm.contains("Remove Ana&#39;s Mac &amp; co?"), "{confirm}");
    assert!(confirm.contains("It won't be able to unlock its secrets after a restart until you sign in on it again."));
    let done = post_form(&instance, "/account/remove", &cookie, &format!("csrf={csrf}&claktory={}&confirm=yes", claktory.id)).await;
    assert_eq!(done.status, 303);
    instance.advance(60);
    let answer = instance.share(&claktory, None).await.expect_err("removed");
    assert_eq!((answer.status, answer.error().as_str()), (403, "revoked"));
    assert!(get(&instance, "/account", &cookie).await.text().contains("No Claktory uses this account yet."));

    // Signing in on it again undoes the removal.
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.advance(60);
    assert_eq!(instance.share(&claktory, None).await.ok().map(|(epoch, _)| epoch), Some(2));
}

#[tokio::test]
async fn download_my_data_and_delete_account() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.8");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.share(&claktory, None).await.expect("a share");
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.51").await;

    let data = get(&instance, "/account/data", &cookie).await;
    assert!(
        data.headers.get("content-disposition").and_then(|value| value.to_str().ok()).is_some_and(|value| value.starts_with("attachment"))
    );
    let data = data.value();
    assert_eq!(data["account"]["email"], "matt@example.com");
    assert_eq!(data["claktories"][0]["id"], claktory.id.as_str());
    assert_eq!(data["claktories"][0]["unlocks"][0]["ip"], "203.0.113.8");
    assert!(!data.to_string().contains("token"), "no token hashes");

    let page = get(&instance, "/account", &cookie).await.text();
    let csrf = field(&page, "csrf");
    let confirm = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}")).await.text();
    assert!(confirm.contains("Delete your account?"));
    let done = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(done.text().contains("Your account was deleted."));

    let conn = rusqlite::Connection::open(&instance.db).unwrap();
    for table in ["accounts", "claktories", "share_releases", "sessions"] {
        let count: i64 = conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0)).unwrap();
        assert_eq!(count, 0, "{table}");
    }
    instance.advance(60);
    assert_eq!(instance.share(&claktory, None).await.expect_err("gone").error(), "unauthorized");
    assert!(get(&instance, "/account", &cookie).await.text().contains("Sign in to Claktory"), "signed out");
}

#[tokio::test]
async fn a_web_code_signs_in_only_on_the_web() {
    let instance = start().await;
    let body = b"email=matt%40example.com".to_vec();
    let page = instance.send("POST", "/account/sign-in", body, &[("content-type", FORM.into())]).await.text();
    let request = field(&page, "request");
    let verify = claktory_account::protocol::SignInVerify {
        request_id: request,
        code: instance.last_code("matt@example.com"),
        eph_pub: Ephemeral::new().public_b64(),
    };
    assert_eq!(instance.post("/v1/sign-in/verify", &verify, "192.0.2.52").await.error(), "code_expired");

    let page =
        instance.send("POST", "/account/verify", b"request=x&code=1&email=a".to_vec(), &[("content-type", FORM.into())]).await.text();
    assert!(page.contains("That code has expired. Get a new one."), "{page}");
}

#[tokio::test]
async fn sign_out_ends_the_session() {
    let instance = start().await;
    let cookie = web_sign_in(&instance, "new@example.com", "192.0.2.53").await;
    let page = get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("No Claktory uses this account yet.") && !page.contains("Delete account"), "no account yet");
    let answer = post_form(&instance, "/account/sign-out", &cookie, "").await;
    assert_eq!(answer.status, 303);
    assert!(get(&instance, "/account", &cookie).await.text().contains("Sign in to Claktory"));
}

#[tokio::test]
async fn out_of_tries_offers_a_new_code() {
    let instance = start().await;
    let headers = [("content-type", FORM.to_owned()), ("x-forwarded-for", "192.0.2.54".to_owned())];
    let page = instance.send("POST", "/account/sign-in", b"email=matt%40example.com".to_vec(), &headers).await.text();
    assert!(page.contains("Check your email") && page.contains("Resend code"), "{page}");
    let request = field(&page, "request");
    let code = instance.last_code("matt@example.com");
    let wrong = if code == "000000" { "000001" } else { "000000" };
    let mut page = String::new();
    for _ in 0..5 {
        let body = format!("request={request}&email=matt%40example.com&code={wrong}").into_bytes();
        page = instance.send("POST", "/account/verify", body, &headers).await.text();
    }
    assert!(page.contains("Too many tries. Get a new code.") && page.contains("Send new code"), "{page}");
    assert_eq!(field(&page, "email"), "matt@example.com");
}

#[tokio::test]
async fn the_bare_root_leads_to_the_account_page() {
    let instance = start().await;
    for method in ["GET", "HEAD"] {
        let answer = instance.send(method, "/", Vec::new(), &[]).await;
        assert_eq!(answer.status, 302, "{method}");
        assert_eq!(answer.headers.get("location").and_then(|value| value.to_str().ok()), Some("/account"), "{method}");
    }
    assert_eq!(instance.send("POST", "/", Vec::new(), &[]).await.status, 404, "only GET and HEAD");
    assert_eq!(instance.send("GET", "/elsewhere", Vec::new(), &[]).await.status, 404);
}

/// The app's Delete account… opens /account/delete: signed in, the confirm
/// step; signed out, the sign-in, which leads there instead of /account.
#[tokio::test]
async fn delete_account_has_a_page_of_its_own_that_signs_in_first() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.9");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;

    let headers = [("content-type", FORM.to_owned()), ("x-forwarded-for", "192.0.2.55".to_owned())];
    let page = instance.send("GET", "/account/delete", Vec::new(), &[]).await.text();
    assert!(page.contains("To delete your account, sign in first.") && page.contains("name=\"next\" value=\"delete\""), "{page}");
    let body = b"next=delete&email=matt%40example.com".to_vec();
    let page = instance.send("POST", "/account/sign-in", body, &headers).await.text();
    assert_eq!(field(&page, "next"), "delete", "the code step carries it on");
    let request = field(&page, "request");
    let code = instance.last_code("matt@example.com");
    let body = format!("next=delete&request={request}&email=matt%40example.com&code={code}").into_bytes();
    let answer = instance.send("POST", "/account/verify", body, &headers).await;
    assert_eq!(answer.status, 303);
    assert_eq!(answer.headers.get("location").and_then(|value| value.to_str().ok()), Some("/account/delete"));
    let cookie = answer.headers.get("set-cookie").and_then(|value| value.to_str().ok()).expect("a cookie");
    let cookie = cookie.split(';').next().expect("the cookie").to_owned();

    let page = get(&instance, "/account/delete", &cookie).await.text();
    assert!(page.contains("Delete your account?") && page.contains("Cancel"), "{page}");
    let csrf = field(&page, "csrf");
    let done = post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    assert!(done.text().contains("Your account was deleted."));

    // Signed in with an email no account has.
    let cookie = web_sign_in(&instance, "new@example.com", "192.0.2.56").await;
    let page = get(&instance, "/account/delete", &cookie).await.text();
    assert!(page.contains("No account to delete"), "{page}");
    // Signing in from the account page still leads to it.
    let page = instance.send("GET", "/account", Vec::new(), &[]).await.text();
    assert!(!page.contains("name=\"next\""), "{page}");
}

#[tokio::test]
async fn the_mark_sign_in_emails_show_is_served_to_any_number_of_image_proxies() {
    let instance = start().await;
    let mut claktory = Claktory::new("203.0.113.7");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mail = instance.mail.sent().into_iter().last().expect("the code's email");
    assert!(mail.html.contains("<img src=\"https://account.claktory.com/mark.png\""), "{}", mail.html);
    assert!(mail.html.contains(&format!(">{}</div>", instance.last_code("matt@example.com"))), "the code, large");

    // More than an IP's requests a minute: mail providers fetch it through few proxies.
    for _ in 0..40 {
        let mark = instance.send("GET", "/mark.png", Vec::new(), &[("x-forwarded-for", "192.0.2.80".into())]).await;
        assert_eq!(mark.status, 200);
        assert_eq!(mark.headers.get("content-type").and_then(|value| value.to_str().ok()), Some("image/png"));
        assert!(mark.body.starts_with(b"\x89PNG\r\n\x1a\n"));
    }
}
