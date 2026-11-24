//! When an unlock is emailed (#412, Fugo's rule in
//! coordination/security/unlock-mail-rule.md): only when it doesn't look
//! like the Claktory restarting where it runs. A network none of its
//! unlocks in the last 30 days came from, or an older copy of it, is
//! emailed at once; the first unlock just after signing in is the person's.
//! The account page lists every unlock and says which were emailed.

use crate::support::*;
use crate::web::{get, web_sign_in};

/// Unlock notices sent so far.
fn notices(instance: &Instance) -> Vec<String> {
    instance.mail.sent().into_iter().filter(|mail| mail.subject.ends_with("unlocked its secrets")).map(|mail| mail.text).collect()
}

/// A Claktory signed in a while ago, with one unlock from where it runs
/// (emailed: its network was new then).
async fn running(instance: &Instance, ip: &str) -> Claktory {
    let mut claktory = Claktory::new(ip);
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    instance.advance(16 * 60);
    instance.share(&claktory, None).await.expect("a share");
    assert_eq!(notices(instance).len(), 1, "a network it hadn't unlocked from");
    claktory
}

#[tokio::test]
async fn restarts_on_a_known_network_are_quiet_and_a_new_network_is_emailed() {
    let instance = start().await;
    let mut claktory = running(&instance, "203.0.113.7").await;
    // Restarts from the same IPv4 /24: no email.
    for ip in ["203.0.113.7", "203.0.113.200"] {
        instance.advance(3600);
        claktory.ip = ip.into();
        instance.share(&claktory, None).await.expect("a share");
    }
    assert_eq!(notices(&instance).len(), 1);
    // Another network: emailed at once, with where it came from.
    instance.advance(3600);
    claktory.ip = "198.51.100.9".into();
    instance.share(&claktory, None).await.expect("a share");
    let sent = notices(&instance);
    assert_eq!(sent.len(), 2);
    assert!(sent[1].starts_with("Mac mini unlocked its secrets from a network it hasn't used in the last 30 days.\n\n"), "{}", sent[1]);
    assert!(sent[1].contains("From: 198.51.100.9\n"), "{}", sent[1]);
    // IPv6: the same /48 is known, another isn't.
    for (ip, emailed) in [("2001:db8:1:1::5", 3), ("2001:db8:1:ffff::9", 3), ("2001:db8:2::1", 4)] {
        instance.advance(3600);
        claktory.ip = ip.into();
        instance.share(&claktory, None).await.expect("a share");
        assert_eq!(notices(&instance).len(), emailed, "{ip}");
    }

    // The account page lists every unlock (the last 5) and marks the emailed ones.
    let cookie = web_sign_in(&instance, "matt@example.com", "192.0.2.60").await;
    let page = get(&instance, "/account", &cookie).await.text();
    assert!(page.contains("from 2001:db8:2::1 <span class=\"muted\">· emailed you</span></li>"), "{page}");
    assert!(page.contains("from 2001:db8:1:ffff::9</li>"), "{page}");
    let data = get(&instance, "/account/data", &cookie).await.value();
    let unlocks = data["claktories"][0]["unlocks"].as_array().expect("unlocks").clone();
    assert_eq!(unlocks.len(), 7);
    let emailed: Vec<bool> = unlocks.iter().map(|unlock| unlock["emailed"].as_bool().expect("emailed")).collect();
    assert_eq!(emailed, [true, false, true, true, false, false, true], "newest first");
}

#[tokio::test]
async fn an_unlock_after_30_days_away_is_emailed_even_from_the_same_ip() {
    let instance = start().await;
    let claktory = running(&instance, "203.0.113.7").await;
    instance.advance(31 * 86_400);
    instance.share(&claktory, None).await.expect("a share");
    assert_eq!(notices(&instance).len(), 2);
}

#[tokio::test]
async fn an_older_copy_is_emailed_and_signing_in_again_is_not() {
    let instance = start().await;
    let mut claktory = running(&instance, "203.0.113.7").await;

    // Signed in again (epoch 2): within 15 minutes it asks for the old
    // epoch's share, then the current one, then again (a retry), all quiet.
    instance.advance(3600);
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    assert_eq!(claktory.epoch, 2);
    instance.advance(60);
    assert_eq!(instance.share(&claktory, Some(1)).await.ok().map(|(epoch, _)| epoch), Some(1));
    instance.advance(60);
    assert_eq!(instance.share(&claktory, None).await.ok().map(|(epoch, _)| epoch), Some(2));
    instance.advance(60);
    assert_eq!(instance.share(&claktory, Some(2)).await.ok().map(|(epoch, _)| epoch), Some(2));
    assert_eq!(notices(&instance).len(), 1, "signing in again emails nothing");

    // A copy of its disk from before that sign-in asks for the old epoch,
    // from the same network: emailed at once.
    instance.advance(3600);
    assert!(instance.share(&claktory, Some(1)).await.is_ok());
    let sent = notices(&instance);
    assert_eq!(sent.len(), 2);
    assert!(
        sent[1].starts_with("An older copy of Mac mini, from before it last signed in to your account, unlocked its secrets.\n\n"),
        "{}",
        sent[1]
    );
}

#[tokio::test]
async fn the_first_unlock_long_after_signing_in_from_elsewhere_is_emailed() {
    let instance = start().await;
    // Signed in, and its first unlock within 15 minutes: quiet, wherever.
    let mut quick = Claktory::new("203.0.113.7");
    instance.sign_in(&mut quick, "matt@example.com", "Mac mini").await;
    instance.advance(14 * 60);
    quick.ip = "198.51.100.30".into();
    instance.share(&quick, None).await.expect("a share");
    assert_eq!(notices(&instance).len(), 0);
    // Its first unlock 16 minutes after signing in, on a network it never
    // unlocked from: emailed.
    let mut late = Claktory::new("203.0.113.8");
    instance.sign_in(&mut late, "matt@example.com", "Mac Studio").await;
    instance.advance(16 * 60);
    instance.share(&late, None).await.expect("a share");
    assert_eq!(notices(&instance).len(), 1);
}

#[tokio::test]
async fn an_emailed_unlock_still_gets_its_share_while_mail_is_down() {
    let instance = start().await;
    let mut claktory = running(&instance, "203.0.113.7").await;
    instance.mail.fail.store(true, std::sync::atomic::Ordering::SeqCst);
    instance.advance(3600);
    claktory.ip = "198.51.100.40".into();
    assert_eq!(instance.share(&claktory, None).await.ok().map(|(epoch, _)| epoch), Some(1));
}
