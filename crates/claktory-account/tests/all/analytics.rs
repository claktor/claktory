use std::net::IpAddr;
use std::sync::Arc;

use claktory_account::protocol::*;
use claktory_account::server::geo::{Location, Locator};
use claktory_account::server::{Config, db, stats};

use crate::support::*;

/// Places 203.0.113.x in Amsterdam and 198.51.100.x in Lyon.
struct Places;

impl Locator for Places {
    fn locate(&self, ip: IpAddr) -> Option<Location> {
        let place = |city: &str, region: &str, country: &str| Location {
            country: Some(country.to_owned()),
            region: Some(region.to_owned()),
            city: Some(city.to_owned()),
        };
        match ip.to_string() {
            ip if ip.starts_with("203.0.113.") => Some(place("Amsterdam", "North Holland", "Netherlands")),
            ip if ip.starts_with("198.51.100.") => Some(place("Lyon", "Auvergne-Rhône-Alpes", "France")),
            _ => None,
        }
    }
}

async fn start_located() -> Instance {
    start_with(Config { trust_proxy: true, locator: Some(Arc::new(Places)), admins: vec!["ops@example.com".into()], ..Config::default() })
        .await
}

fn usage(model: &str, input: u64, output: u64) -> Usage {
    Usage {
        provider: "anthropic".into(),
        model: model.into(),
        model_name: None,
        requests: 10,
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: 1000,
        cache_write_tokens: 100,
    }
}

/// A period of the hour before `now`.
fn period(id: &str, now: i64, usage: Vec<Usage>) -> Period {
    let hour = now - now.rem_euclid(3600) - 3600;
    Period {
        id: id.into(),
        from: hour,
        to: hour + 3600,
        usage,
        activity: vec![Activity { hour, agent_minutes: 90, agents_working: 2, runs: 4 }],
    }
}

fn check_in(periods: Vec<Period>) -> CheckIn {
    CheckIn {
        version: "v0.1.9".into(),
        platform: "macos-arm64".into(),
        counts: Some(Counts { agents: 12, projects: 3, devices: 2 }),
        periods,
    }
}

#[tokio::test]
async fn a_check_in_records_its_periods_once() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("203.0.113.30");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let now = instance.now();
    let body = check_in(vec![period(
        "period-0000000000000001",
        now,
        vec![usage("claude-opus-4-8", 5000, 800), usage("claude-haiku-4-5", 100, 10)],
    )]);
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await.json();
    assert_eq!((checked.periods_recorded, checked.periods_ignored), (1, 0));

    // Its answer was lost, so it sends the period again, with a new one.
    instance.advance(3600);
    let mut again =
        check_in(vec![body.periods[0].clone(), period("period-0000000000000002", instance.now(), vec![usage("claude-opus-4-8", 1, 1)])]);
    again.counts = Some(Counts { agents: 13, projects: 3, devices: 2 });
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &again, None).await.json();
    assert_eq!(checked.periods_recorded, 1, "the first was recorded already");

    let public: PublicStats = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json();
    assert_eq!(public.tokens_total, (5000 + 800 + 1100) + (100 + 10 + 1100) + (1 + 1 + 1100));
    assert_eq!((public.input_tokens, public.output_tokens, public.cache_read_tokens, public.cache_write_tokens), (5101, 811, 3000, 300));
    assert_eq!(public.claktories, 1);
    assert_eq!(public.since, START);

    let conn = db::open(&instance.db).unwrap();
    let saved = db::claktory(&conn, &claktory.id).unwrap().unwrap();
    assert_eq!(saved.counts, Some(Counts { agents: 13, projects: 3, devices: 2 }));
    assert_eq!(saved.location.describe().as_deref(), Some("Amsterdam, North Holland, Netherlands"));
    let (minutes, runs): (i64, i64) = conn
        .query_row("SELECT sum(agent_minutes), sum(runs) FROM activity WHERE claktory_id = ?1", [&claktory.id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .unwrap();
    assert_eq!((minutes, runs), (180, 8));
    let ips: i64 = conn.query_row("SELECT count(*) FROM sqlite_master WHERE sql LIKE '%ip TEXT%'", [], |row| row.get(0)).unwrap();
    assert_eq!(ips, 1, "only share releases keep an IP");
}

#[tokio::test]
async fn periods_out_of_range_are_ignored_and_the_rest_recorded() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("198.51.100.31");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let now = instance.now();
    let good = period("period-0000000000000010", now, vec![usage("claude-opus-4-8", 10, 10)]);
    let mut future = period("period-0000000000000011", now + 7200, vec![]);
    future.activity.clear();
    let old = period("period-0000000000000012", now - 91 * 86_400, vec![]);
    let mut odd_hour = period("period-0000000000000013", now, vec![]);
    odd_hour.activity[0].hour += 60;
    let mut too_busy = period("period-0000000000000014", now, vec![]);
    too_busy.activity[0].agent_minutes = 121;
    let mut bad_name = usage("claude-opus-4-8", 1, 1);
    bad_name.model_name = Some("Claude <b>".into());
    let bad_model = period("period-0000000000000015", now, vec![usage("claude <b>", 1, 1), bad_name]);
    let mut short_id = good.clone();
    short_id.id = "p1".into();
    let mut backwards = period("period-0000000000000016", now, vec![]);
    backwards.to = backwards.from;
    let body = check_in(vec![good, future, old, odd_hour, too_busy, bad_model, short_id, backwards]);
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await.json();
    assert_eq!((checked.periods_recorded, checked.periods_ignored), (1, 7));

    instance.advance(1);
    let mut body = check_in(vec![]);
    body.counts = Some(Counts { agents: 2_000_000, projects: 1, devices: 1 });
    let answer = instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"));

    // A check-in from an older Claktory, without any of it, still works.
    instance.advance(1);
    let plain = serde_json::json!({"version": "v0.1.9", "platform": "macos-arm64"});
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &plain, None).await.json();
    assert_eq!(checked.periods_recorded, 0);
}

#[tokio::test]
async fn the_place_comes_from_a_public_ip_only() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("10.0.0.5");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let conn = db::open(&instance.db).unwrap();
    assert_eq!(db::claktory(&conn, &claktory.id).unwrap().unwrap().location, Location::default());
    claktory.ip = "198.51.100.32".into();
    instance.signed(&claktory, "POST", "/v1/check-in", &check_in(vec![]), None).await;
    assert_eq!(db::claktory(&conn, &claktory.id).unwrap().unwrap().location.city.as_deref(), Some("Lyon"));
}

#[tokio::test]
async fn the_account_page_and_data_show_what_was_sent() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("203.0.113.33");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let mut opus = usage("claude-opus-4-8", 1_000_000, 200_000);
    opus.model_name = Some("Claude Opus 4.8".into());
    let body = check_in(vec![period("period-0000000000000020", instance.now(), vec![opus, usage("claude-haiku-4-5", 800_000, 38_900)])]);
    instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;

    let cookie = crate::web::web_sign_in(&instance, "matt@example.com", "192.0.2.60").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    for text in [
        "Near Amsterdam, Netherlands (from its IP address)",
        "12 agents · 3 projects · 2 devices",
        "Agents worked 2 hours in the last 7 days.",
        "<p class=\"muted\">Usage in the last 30 days:</p><ul><li>Claude Opus 4.8: 1.2M tokens in 10 requests</li><li>claude-haiku-4-5: 840k tokens in 10 requests</li></ul>",
        "IP Geolocation by DB-IP",
    ] {
        assert!(page.contains(text), "{text} in {page}");
    }
    assert!(!page.contains("Usage counts"), "not an admin");
    let admin = crate::web::get(&instance, "/account/admin", &cookie).await.text();
    assert!(admin.contains("Sign in to Claktory") || admin.contains("Your Claktory account"), "not shown to others");

    let data = crate::web::get(&instance, "/account/data", &cookie).await.value();
    let sent = &data["claktories"][0];
    assert_eq!(sent["counts"]["agents"], 12);
    assert_eq!(sent["location"]["city"], "Amsterdam");
    assert_eq!(sent["usage"][1]["input_tokens"], 1_000_000, "after claude-haiku-4-5");
    assert_eq!(sent["activity"][0]["agent_minutes"], 90);
    assert_eq!(sent["days_in_touch"][0], "2026-11-20");
}

#[tokio::test]
async fn whoever_runs_it_sees_the_usage_counts() {
    let instance = start_located().await;
    for (number, ip) in ["203.0.113.40", "198.51.100.41"].into_iter().enumerate() {
        let mut claktory = Claktory::new(ip);
        instance.sign_in(&mut claktory, &format!("p{number}@example.com"), "Mac").await;
        let body = check_in(vec![period("period-0000000000000030", instance.now(), vec![usage("gpt-5.6-sol", 1000, 0)])]);
        instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;
    }
    let cookie = crate::web::web_sign_in(&instance, "ops@example.com", "192.0.2.61").await;
    assert!(crate::web::get(&instance, "/account", &cookie).await.text().contains("Usage counts"));
    let page = crate::web::get(&instance, "/account/admin", &cookie).await.text();
    for text in [
        "In touch today, in the last 7 days, in the last 30 days: Claktories 2 / 2 / 2; accounts 2 / 2 / 2",
        "Now, over all Claktories: 24 agents, 6 projects, 4 devices",
        "Tokens, all time: 4,200 (input 2,000, output 0, cache reads 2,000, cache writes 200)",
        "anthropic gpt-5.6-sol: 4,200 (20)",
        "Netherlands: 1",
        "France: 1",
        "13:00: 3 (8)",
    ] {
        assert!(page.contains(text), "{text} in {page}");
    }
    let conn = db::open(&instance.db).unwrap();
    assert!(stats::report(&conn, instance.now()).unwrap().contains("2026-11-20: 2 (2)"));
}

#[tokio::test]
async fn deleting_an_account_keeps_only_anonymous_totals() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("203.0.113.50");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let body = check_in(vec![period("period-0000000000000040", instance.now(), vec![usage("claude-opus-4-8", 7000, 0)])]);
    instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;
    let before: PublicStats = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json();

    let cookie = crate::web::web_sign_in(&instance, "matt@example.com", "192.0.2.62").await;
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    let csrf = crate::web::field(&page, "csrf");
    crate::web::post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;

    let conn = db::open(&instance.db).unwrap();
    let linked: i64 = conn
        .query_row(
            "SELECT (SELECT count(*) FROM usage WHERE claktory_id != '') + (SELECT count(*) FROM activity WHERE claktory_id != '')
             + (SELECT count(*) FROM seen_days WHERE claktory_id = ?1) + (SELECT count(*) FROM periods)",
            [&claktory.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked, 0);
    instance.advance(60);
    let after: PublicStats = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json();
    assert_eq!((after.tokens_total, after.claktories), (before.tokens_total, 0));
}

#[tokio::test]
async fn the_public_stats_are_for_any_page_and_a_minute_old_at_most() {
    let instance = start().await;
    let answer = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await;
    assert_eq!(answer.headers.get("access-control-allow-origin").unwrap(), "*");
    assert_eq!(answer.headers.get("cache-control").unwrap(), "public, max-age=300");
    assert_eq!(answer.json::<PublicStats>().tokens_total, 0);
    let mut claktory = Claktory::new("192.0.2.70");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    assert_eq!(instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json::<PublicStats>().claktories, 0, "counted a moment ago");
    instance.advance(60);
    assert_eq!(instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json::<PublicStats>().claktories, 1);
}

#[test]
fn a_database_from_the_first_version_gains_the_new_columns() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("account.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE claktories (id TEXT PRIMARY KEY, account_id TEXT NOT NULL, name TEXT NOT NULL, backend_key TEXT NOT NULL,
             token_hash TEXT NOT NULL UNIQUE, epoch INTEGER NOT NULL, created INTEGER NOT NULL, last_seen INTEGER NOT NULL,
             version TEXT NOT NULL, platform TEXT NOT NULL, revoked_at INTEGER);
             INSERT INTO claktories VALUES ('claktory-0123456789', 'a', 'Mac', 'k', 'h', 1, 0, 0, 'v0.1.0', 'macos-arm64', NULL);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let conn = db::open(&path).unwrap();
    let claktory = db::claktory(&conn, "claktory-0123456789").unwrap().expect("kept");
    assert_eq!((claktory.name.as_str(), claktory.counts, claktory.location), ("Mac", None, Location::default()));
    drop(conn);
    db::open(&path).expect("opens again");
}

/// With a real GeoIP file (CLAKTORY_GEOIP_FILE, such as DB-IP's free city
/// file the deploy kit fetches), known addresses land where they should.
/// Without one there's nothing to check.
#[test]
fn a_geoip_file_places_public_addresses() {
    let Some(path) = std::env::var_os("CLAKTORY_GEOIP_FILE") else { return };
    let geoip = claktory_account::server::geo::GeoIp::open(std::path::Path::new(&path)).expect("opens");
    let google_dns = geoip.locate("8.8.8.8".parse().unwrap()).expect("a place");
    assert_eq!(google_dns.country.as_deref(), Some("United States"), "{google_dns:?}");
    let quad9_v6 = geoip.locate("2620:fe::fe".parse().unwrap());
    assert!(quad9_v6.is_some_and(|place| place.country.is_some()), "IPv6 too");
    assert_eq!(geoip.locate("10.0.0.1".parse().unwrap()), None);
}
