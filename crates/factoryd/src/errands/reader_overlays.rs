//! What covers the page (#435, Fugo 40527): the reader lists it first and
//! names it when it covers a target; dismiss clears it with the most
//! private choice, never a link, never what's the person's, and never twice.

use super::tests::{TestHand, element, page_with, seen};
use super::*;

/// A long page, many links before its end, and a cookie notice there that
/// covers a button.
fn cookies(buttons: &str) -> String {
    let links: String = (0..250).map(|n| format!("<a href=\"/p{n}\">Product {n}</a> ")).collect();
    format!(
        r#"<!doctype html><title>Kettles</title><style>body{{margin:0}} #cn{{position:fixed;bottom:0;left:0;right:0;height:140px;background:#fff;z-index:9}}
        #under{{position:fixed;bottom:40px;left:20px}}</style>
        <h1>Kettles</h1><button id=under onclick="window.bought = true">Add to basket</button>
        <div style="height:3000px">{links}</div>
        <div id=cn role=dialog aria-label="Cookie consent"><p>We use cookies to improve your experience.</p>{buttons}</div>
        <script>function pick(choice) {{ window.picked = choice; document.getElementById('cn').remove(); }}</script>"#
    )
}

fn dismissed(answer: Answer) -> schema::Dismissed {
    match answer {
        Answer::Dismissed(dismissed) => dismissed,
        other => panic!("{other:?}"),
    }
}

#[tokio::test]
async fn a_cookie_notice_is_read_first_and_refused_with_its_most_private_choice() {
    let html = cookies(r#"<button onclick="pick('accept')">Accept all</button><button onclick="pick('reject')">Reject all</button>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(read.overlays, ["cookie notice"]);
    // Its buttons first, though the page lists 250 links before them.
    assert_eq!(read.elements[0].overlay.as_deref(), Some("cookie notice"), "{:?}", read.elements[0]);
    assert!(read.text.trim_start().starts_with("⟦cookie notice:⟧ We use cookies"), "{}", read.text);
    // What it covers says so.
    let under = element(&read, "Add to basket").reference.clone();
    let covered = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &under, None).unwrap()).await.unwrap_err();
    assert!(covered.1.starts_with("covered by a cookie notice: clear it with browser_act op dismiss"), "{covered:?}");
    // Its Accept all isn't the agent's to press while it offers a refusal.
    let accept = element(&read, "Accept all").reference.clone();
    let refused = page.act(&mut hands.hand(), &Op::on_element(&read.document, 0, &accept, None).unwrap()).await.unwrap_err();
    assert_eq!(refused.0, "sensitive_action", "{refused:?}");
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert_eq!((done.kind.as_str(), done.pressed.as_slice(), done.accepted_cookies, done.gone), ("cookie notice", &["Reject all".to_owned()][..], false, true));
    assert_eq!(seen(&page, "window.picked").await, "reject");
    assert_eq!(seen(&page, "window.bought === undefined").await, true);
    let none = page.dismiss(&mut hands.hand(), &read.document).await.unwrap_err();
    assert_eq!(none.0, "unsupported_target", "{none:?}");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_notice_that_only_offers_accept_is_accepted_and_says_so() {
    let html = cookies(r#"<button onclick="pick('accept')">Got it</button><a href="/privacy">Privacy policy</a>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert!(done.accepted_cookies && done.gone, "{done:?}");
    assert_eq!(done.pressed, ["Got it"]);
    assert_eq!(seen(&page, "window.picked").await, "accept");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_notices_settings_are_opened_for_a_more_private_choice_than_accept() {
    let html = cookies(
        r#"<button onclick="pick('accept')">Accept all cookies</button>
        <button onclick="this.parentNode.style.display = 'none'; document.getElementById('pc').style.display = 'block'">Cookie settings</button>
        </div><div id=pc role=dialog style="display:none;position:fixed;inset:10%;background:#fff;z-index:10"><p>Privacy preference centre: which cookies we may use.</p>
        <label><input type=checkbox> Marketing cookies</label>
        <button onclick="window.picked = 'all'">Allow all</button><button onclick="window.picked = 'confirmed'; this.parentNode.remove()">Confirm my choices</button>"#,
    );
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert_eq!(done.pressed, ["Cookie settings", "Confirm my choices"]);
    assert!(!done.accepted_cookies, "{done:?}");
    assert_eq!(seen(&page, "window.picked").await, "confirmed");
    let _ = std::fs::remove_dir_all(&profile);
}

/// A notice whose settings panel holds `purposes` (its switches) and
/// records Escape.
fn panel(purposes: &str) -> String {
    cookies(&format!(
        r#"<button onclick="pick('accept')">Accept all cookies</button>
        <button onclick="this.parentNode.style.display = 'none'; document.getElementById('pc').style.display = 'block'">Cookie settings</button>
        </div><div id=pc role=dialog style="display:none;position:fixed;inset:10%;background:#fff;z-index:10"><p>Privacy preference centre: which cookies we may use.</p>
        <label><input type=checkbox checked disabled> Strictly necessary</label>{purposes}
        <button onclick="window.picked = 'all'">Allow all</button><button onclick="window.picked = 'confirmed'; window.saved = [...document.querySelectorAll('#pc input')].map(i => i.checked); this.parentNode.remove()">Confirm my choices</button>
        <script>document.addEventListener('keydown', (e) => {{ if (e.key === 'Escape') window.escaped = true; }});</script>"#
    ))
}

#[tokio::test]
async fn a_panels_purposes_switched_on_are_turned_off_before_it_is_saved() {
    let html = panel(r#"<label><input type=checkbox id=ads checked> Marketing cookies</label><span role=switch aria-checked=false tabindex=0>Analytics</span>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert_eq!(done.pressed, ["Cookie settings", "Confirm my choices"]);
    assert_eq!(done.turned_off, 1, "{done:?}");
    assert_eq!(seen(&page, "window.picked").await, "confirmed");
    assert_eq!(seen(&page, "JSON.stringify(window.saved)").await, "[true,false]", "only what's needed, on");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_panel_with_a_purpose_on_that_cant_be_turned_off_is_never_saved() {
    let html = panel(r#"<input type=checkbox checked style="display:none"><span>Personalised ads</span>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert!(done.escape && !done.accepted_cookies, "{done:?}");
    assert_eq!(done.pressed, ["Cookie settings"]);
    assert_eq!(seen(&page, "window.picked === undefined && window.escaped === true").await, true);
    let _ = std::fs::remove_dir_all(&profile);
}

/// A pop-up over half the page.
fn popup(inside: &str) -> String {
    format!(
        r#"<!doctype html><title>Kettles</title><h1>Kettles</h1><p>Every kettle we sell.</p>
        <div id=pop role=dialog aria-modal=true style="position:fixed;inset:20%;background:#fff;z-index:9"><p>Sign up for our newsletter and save 10%.</p>{inside}</div>
        <script>document.addEventListener('keydown', (e) => {{ if (e.key === 'Escape') {{ window.escaped = true; document.getElementById('pop').remove(); }} }});</script>"#
    )
}

#[tokio::test]
async fn a_pop_ups_link_is_never_pressed_to_dismiss_it() {
    let html = popup(r#"<label>Email <input type=email></label><a href="https://elsewhere.example/">No thanks</a>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(read.overlays, ["pop-up"]);
    let url = page.frame().await.unwrap().url;
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert!(done.escape && done.pressed.is_empty() && done.gone, "{done:?}");
    assert_eq!(seen(&page, "window.escaped").await, true);
    assert_eq!(page.frame().await.unwrap().url, url, "the page stayed");
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_pop_up_that_comes_back_is_not_dismissed_again() {
    let html = popup(
        r#"<button onclick="const pop = document.getElementById('pop'); pop.style.display = 'none'; window.shut = (window.shut || 0) + 1; setTimeout(() => pop.style.display = 'block', 50)">Close</button>"#,
    );
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert_eq!(done.pressed, ["Close"]);
    let again = page.dismiss(&mut hands.hand(), &read.document).await.unwrap_err();
    assert!(again.0 == "unsupported_target" && again.1.contains("came back"), "{again:?}");
    assert_eq!(seen(&page, "window.shut").await, 1);
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_pop_up_that_asks_for_a_password_is_the_persons() {
    let html = popup(r#"<label>Password <input type=password></label><button onclick="window.shut = true">Close</button>"#);
    let Some((_local, page, profile)) = page_with(&html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    let refused = page.dismiss(&mut hands.hand(), &read.document).await.unwrap_err();
    assert_eq!(refused.0, "sensitive_action", "{refused:?}");
    assert_eq!(seen(&page, "window.shut === undefined").await, true);
    let _ = std::fs::remove_dir_all(&profile);
}

#[tokio::test]
async fn a_notice_in_an_open_shadow_root_is_read_and_dismissed() {
    let html = r#"<!doctype html><title>Kettles</title><h1>Kettles</h1><div id=host></div><script>
        const root = document.getElementById('host').attachShadow({mode: 'open'});
        root.innerHTML = '<div style="position:fixed;bottom:0;left:0;right:0;height:120px;background:#fff">We use cookies. <button id=a>Accept</button><button id=d>Deny</button></div>';
        for (const [id, choice] of [['a', 'accept'], ['d', 'deny']]) root.getElementById(id).onclick = () => { window.picked = choice; document.getElementById('host').remove(); };
        </script>"#;
    let Some((_local, page, profile)) = page_with(html).await else { return };
    let mut hands = TestHand::new();
    let Ok(Answer::Page(read)) = page.read(4096).await else { panic!("a page") };
    assert_eq!(element(&read, "Deny").overlay.as_deref(), Some("cookie notice"));
    assert!(read.text.contains("We use cookies"), "{}", read.text);
    let done = dismissed(page.dismiss(&mut hands.hand(), &read.document).await.unwrap());
    assert_eq!(done.pressed, ["Deny"]);
    assert_eq!(seen(&page, "window.picked").await, "deny");
    let _ = std::fs::remove_dir_all(&profile);
}

#[test]
fn sign_in_payment_and_check_services_are_never_touched() {
    for url in [
        "https://accounts.google.com/v3/signin",
        "https://js.stripe.com/v3/elements-inner-card.html",
        "https://www.paypal.com/checkoutnow",
        "https://challenges.cloudflare.com/cdn-cgi/challenge-platform/h/b/turnstile",
        "https://www.google.com/recaptcha/api2/anchor",
        "https://newassets.hcaptcha.com/captcha/v1/",
        "https://geo.captcha-delivery.com/captcha/",
        "https://client-api.arkoselabs.com/fc/gt2/",
    ] {
        assert!(untouched(url), "{url}");
    }
    for url in ["https://www.google.com/search", "https://notstripe.com/", "https://shop.example/stripe.com", "about:blank"] {
        assert!(!untouched(url), "{url}");
    }
}

#[test]
fn a_cookie_notices_choice_is_the_most_private_it_offers() {
    let button = |name: &str, choice: Option<&'static str>, usable: bool| Button { reference: "e1".into(), name: name.into(), choice, usable };
    let notice = |buttons| Covering { kind: "cookie notice", personal: false, buttons, switches: Vec::new() };
    let pressed = |pick: Pick<'_>| match pick {
        Pick::Press(button, accepts) => format!("{}{}", button.name, if accepts { " (accepts)" } else { "" }),
        Pick::Settings(button) => format!("settings {}", button.name),
        Pick::Save(button) => format!("save {}", button.name),
        Pick::Escape => "Escape".to_owned(),
    };
    let both = notice(vec![button("Accept all", Some("accept"), true), button("Reject all", Some("private"), true)]);
    assert_eq!(pressed(both.pick(false)), "Reject all");
    let settings = notice(vec![button("Accept all", Some("accept"), true), button("Manage", Some("settings"), true)]);
    assert_eq!(pressed(settings.pick(false)), "settings Manage");
    assert_eq!(pressed(settings.pick(true)), "Escape", "settings opened offered more than Accept");
    let panel = notice(vec![button("Allow all", Some("accept"), true), button("Confirm my choices", Some("save"), true)]);
    assert_eq!(pressed(panel.pick(true)), "save Confirm my choices");
    assert_eq!(pressed(panel.pick(false)), "Escape", "saving is only for settings Claktory opened");
    let only = notice(vec![button("OK", Some("accept"), true)]);
    assert_eq!(pressed(only.pick(false)), "OK (accepts)");
    let link = notice(vec![button("Reject all", Some("private"), false), button("Accept all", Some("accept"), true)]);
    assert_eq!(pressed(link.pick(false)), "Escape", "a refusal that's a link is never pressed, nor Accept all beside it");
    let pop = Covering { kind: "pop-up", personal: false, buttons: vec![button("Accept", Some("accept"), true)], switches: Vec::new() };
    assert_eq!(pressed(pop.pick(false)), "Escape", "a pop-up's agreeing is never Claktory's");
}
