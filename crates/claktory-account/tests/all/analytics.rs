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
        ..Period::default()
    }
}

fn check_in(periods: Vec<Period>) -> CheckIn {
    CheckIn {
        version: "v0.1.9".into(),
        platform: "macos-arm64".into(),
        counts: Some(Counts { agents: 12, projects: 3, devices: 2, ..Counts::default() }),
        periods,
        ..CheckIn::default()
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
    again.counts = Some(Counts { agents: 13, projects: 3, devices: 2, ..Counts::default() });
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &again, None).await.json();
    assert_eq!(checked.periods_recorded, 1, "the first was recorded already");

    let public: PublicStats = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json();
    assert_eq!(public.tokens_total, (5000 + 800 + 1100) + (100 + 10 + 1100) + (1 + 1 + 1100));
    assert_eq!((public.input_tokens, public.output_tokens, public.cache_read_tokens, public.cache_write_tokens), (5101, 811, 3000, 300));
    assert_eq!(public.claktories, 1);
    assert_eq!(public.since, START);

    let conn = db::open(&instance.db).unwrap();
    let saved = db::claktory(&conn, &claktory.id).unwrap().unwrap();
    assert_eq!(saved.counts, Some(Counts { agents: 13, projects: 3, devices: 2, ..Counts::default() }));
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
    body.counts = Some(Counts { agents: 2_000_000, projects: 1, devices: 1, ..Counts::default() });
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
    let mut body = check_in(vec![period("period-0000000000000040", instance.now(), vec![usage("claude-opus-4-8", 7000, 0)])]);
    body.periods[0].devices_connected = Some(1);
    body.periods[0].health = vec![Health { version: "v0.1.16".into(), rolled_back: 1, crashes: 0 }];
    body.devices = Some(vec![DeviceCount { platform: "ios".into(), kind: "phone".into(), app_version: "v0.1.16".into(), count: 1 }]);
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
             + (SELECT count(*) FROM activity_days WHERE claktory_id != '')
             + (SELECT count(*) FROM seen_days WHERE claktory_id = ?1) + (SELECT count(*) FROM periods)
             + (SELECT count(*) FROM health WHERE claktory_id != '') + (SELECT count(*) FROM claktory_devices)
             + (SELECT count(*) FROM devices_connected)",
            [&claktory.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(linked, 0);
    instance.advance(60);
    let after: PublicStats = instance.send("GET", "/v1/stats/public", Vec::new(), &[]).await.json();
    assert_eq!((after.tokens_total, after.claktories), (before.tokens_total, 0));
}

/// Polpo's cost-per-user §8: a Claktory's hourly activity from before the
/// day 90 days ago is rolled up to its days, so the account disk stops
/// growing by the hour; Download My Data, the daily export and the account
/// page read the same across the boundary, an hour that comes in late joins
/// its day, and deleting the account keeps the days only in the anonymous
/// totals.
#[tokio::test]
async fn hourly_activity_rolls_up_to_days_after_90_days() {
    const DAY: i64 = 86_400;
    let instance = start_located().await;
    let mut claktory = Claktory::new("203.0.113.32");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let now = instance.now();
    let today = now - now.rem_euclid(DAY);
    let (old, kept) = (today - 91 * DAY, today - 90 * DAY);
    let conn = db::open(&instance.db).unwrap();
    let add = |hour: i64, minutes: i64, working: i64, runs: i64| {
        conn.execute(
            "INSERT INTO activity (claktory_id, hour, agent_minutes, agents_working, runs) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![claktory.id, hour, minutes, working, runs],
        )
        .unwrap();
    };
    add(old + 3600, 60, 1, 2);
    add(old + 5 * 3600, 30, 3, 1);
    add(kept + 3600, 45, 2, 1);
    add(today - 3600, 15, 1, 1);
    let cookie = crate::web::web_sign_in(&instance, "matt@example.com", "192.0.2.32").await;
    let mine = |data: &serde_json::Value| data["claktories"][0].clone();
    let before = mine(&crate::web::get(&instance, "/account/data", &cookie).await.value());
    let minutes = |data: &serde_json::Value| -> i64 {
        ["activity", "activity_days"]
            .iter()
            .flat_map(|list| data[list].as_array().cloned().unwrap_or_default())
            .map(|row| row["agent_minutes"].as_i64().unwrap())
            .sum()
    };
    assert_eq!((before["activity"].as_array().unwrap().len(), minutes(&before)), (4, 150));
    let exported = series(&export(&instance), "day", "agent_minutes");
    let page = crate::web::get(&instance, "/account", &cookie).await.text();

    assert_eq!(db::roll_up(&conn, now).unwrap(), 2, "the old day's two hours");
    let after = mine(&crate::web::get(&instance, "/account/data", &cookie).await.value());
    assert_eq!(after["activity"].as_array().unwrap().len(), 2, "the hours of the last 90 days");
    assert_eq!(after["activity"][0]["hour"], kept + 3600);
    assert_eq!(
        after["activity_days"],
        serde_json::json!([{"day": db::day(old), "agent_minutes": 90, "agents_working": 3, "runs": 3}]),
        "summed, the most at once"
    );
    assert_eq!(minutes(&after), minutes(&before));
    assert_eq!(series(&export(&instance), "day", "agent_minutes"), exported);
    assert_eq!(crate::web::get(&instance, "/account", &cookie).await.text(), page);

    // Rolling again changes nothing; an hour of that day sent late joins it.
    assert_eq!(db::roll_up(&conn, now).unwrap(), 0);
    add(old + 7 * 3600, 10, 5, 1);
    assert_eq!(db::roll_up(&conn, now).unwrap(), 1);
    let days = mine(&crate::web::get(&instance, "/account/data", &cookie).await.value())["activity_days"].clone();
    assert_eq!(days, serde_json::json!([{"day": db::day(old), "agent_minutes": 100, "agents_working": 5, "runs": 4}]));
    // A day later, the day that was kept goes too.
    assert_eq!(db::roll_up(&conn, now + DAY).unwrap(), 1);

    // Deleting the account keeps them only in the anonymous totals.
    let page = crate::web::get(&instance, "/account", &cookie).await.text();
    let csrf = crate::web::field(&page, "csrf");
    crate::web::post_form(&instance, "/account/delete", &cookie, &format!("csrf={csrf}&confirm=yes")).await;
    let left: (i64, i64, i64) = conn
        .query_row(
            "SELECT (SELECT count(*) FROM activity_days WHERE claktory_id != ''), coalesce((SELECT sum(agent_minutes) FROM activity_days WHERE claktory_id = ''), 0),
                    coalesce((SELECT sum(agent_minutes) FROM activity WHERE claktory_id = ''), 0)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap();
    assert_eq!(left, (0, 100 + 45, 15));
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

/// #119: what exists (pipes, secrets, monitors, devices by kind), how far
/// setup got, and how each backend version fares after updates, counted
/// and never named; the admin page adds them up.
#[tokio::test]
async fn a_check_in_counts_devices_setup_and_update_health() {
    let instance = start_located().await;
    let mut claktory = Claktory::new("203.0.113.80");
    instance.sign_in(&mut claktory, "matt@example.com", "Mac mini").await;
    let now = instance.now();
    let mut first = period("period-0000000000000080", now, vec![]);
    first.devices_connected = Some(2);
    first.health = vec![Health { version: "v0.1.16".into(), rolled_back: 1, crashes: 2 }];
    let mut body = check_in(vec![first]);
    body.counts = Some(Counts { agents: 4, projects: 2, devices: 3, pipes: 1, secrets: 5, monitors: 2 });
    body.setup = Some(Setup { model_connected: true, first_run: false });
    let device = |platform: &str, kind: &str, count| DeviceCount {
        platform: platform.into(),
        kind: kind.into(),
        app_version: "v0.1.16".into(),
        count,
    };
    body.devices = Some(vec![device("macos", "laptop", 1), device("ios", "phone", 1), device("ipados", "tablet", 1)]);
    let checked: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await.json();
    assert_eq!((checked.periods_recorded, checked.periods_ignored), (1, 0));

    // Its answer lost, the same period comes again: its health isn't added twice.
    instance.advance(1);
    let again: CheckedIn = instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await.json();
    assert_eq!(again.periods_recorded, 0);

    // Later: a step reached stays reached, devices are as they are now.
    instance.advance(3600);
    let mut later = check_in(vec![]);
    later.counts = None;
    later.setup = Some(Setup { model_connected: false, first_run: true });
    later.devices = Some(vec![device("macos", "laptop", 1), device("android", "phone", 2)]);
    instance.signed(&claktory, "POST", "/v1/check-in", &later, None).await;

    let conn = db::open(&instance.db).unwrap();
    let report = stats::report(&conn, instance.now()).unwrap();
    for text in [
        "Active Claktories: daily 1, monthly 1 (daily/monthly 100%)",
        "30-day retention: 0 of 0 Claktories",
        "Setup reached: 1 of 1 Claktories connected a model, 1 ran an agent",
        "  v0.1.16: 1 (2)",
        "  android phone v0.1.16: 2 (1)",
        "  macos laptop v0.1.16: 1 (1)",
        "Devices by system:\n  android: 2\n  macos: 1",
        "2026-11-20: 2",
    ] {
        assert!(report.contains(text), "{text} in {report}");
    }
    assert!(!report.contains("ipados"), "devices are as the last check-in says: {report}");

    // A device kind or version that isn't a label is refused.
    instance.advance(1);
    let mut odd = check_in(vec![]);
    odd.devices = Some(vec![device("ios<b>", "phone", 1)]);
    let answer = instance.signed(&claktory, "POST", "/v1/check-in", &odd, None).await;
    assert_eq!((answer.status, answer.error().as_str()), (400, "invalid_request"));

    // The person's data says it all too.
    let cookie = crate::web::web_sign_in(&instance, "matt@example.com", "192.0.2.80").await;
    let data = crate::web::get(&instance, "/account/data", &cookie).await.value();
    let sent = &data["claktories"][0];
    assert_eq!(sent["counts"]["secrets"], 5);
    assert_eq!(sent["setup"]["first_run"], true);
    assert_eq!(sent["devices"][0]["platform"], "android");
    assert_eq!(sent["health"][0]["crashes"], 2);
    assert_eq!(sent["devices_connected"][0]["count"], 2);

    // A Claktory from before devices were counted leaves the list as it
    // was; one with none left says so.
    let devices =
        || -> i64 { db::open(&instance.db).unwrap().query_row("SELECT count(*) FROM claktory_devices", [], |row| row.get(0)).unwrap() };
    instance.advance(1);
    instance.signed(&claktory, "POST", "/v1/check-in", &check_in(vec![]), None).await;
    assert_eq!(devices(), 2, "none sent: kept");
    instance.advance(1);
    let mut none = check_in(vec![]);
    none.devices = Some(vec![]);
    instance.signed(&claktory, "POST", "/v1/check-in", &none, None).await;
    assert_eq!(devices(), 0, "an empty list: none now");
}

/// The export for Charts (#411): global counts per completed UTC day,
/// in schema 1's eleven columns, never anything of one Claktory.
fn export(instance: &Instance) -> serde_json::Value {
    let conn = rusqlite::Connection::open_with_flags(&instance.db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap();
    serde_json::from_str(&stats::export(&conn, instance.now()).expect("exports")).expect("JSON")
}

/// The rows of `metric` in `window`, by their day's start (seconds).
fn series(export: &serde_json::Value, window: &str, metric: &str) -> Vec<(i64, i64)> {
    export["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["window"] == window && row["metric"] == metric && row["dimension"] == "none")
        .map(|row| (row["bucket_start_ms"].as_i64().unwrap() / 1000, row["value"].as_i64().unwrap()))
        .collect()
}

#[tokio::test]
async fn the_charts_export_has_global_counts_for_completed_days_only() {
    let instance = start_located().await;
    let mut claktories = Vec::new();
    for (number, ip) in ["203.0.113.90", "198.51.100.91"].into_iter().enumerate() {
        let mut claktory = Claktory::new(ip);
        instance.sign_in(&mut claktory, &format!("p{number}@example.com"), "Mac").await;
        let mut body = check_in(vec![period(&format!("period-00000000000009{number}0"), instance.now(), vec![usage("gpt-5.6-sol", 1000, 5)])]);
        body.periods[0].health = vec![Health { version: "v0.1.19".into(), rolled_back: 1, crashes: 2 }];
        body.setup = Some(Setup { model_connected: true, first_run: number == 0 });
        instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;
        claktories.push(claktory);
    }
    let first_day = instance.now() - instance.now().rem_euclid(86_400);

    // The day isn't over: no completed day of it, only today's running
    // totals, partial (slice 3, Fugo 27512).
    let today = export(&instance);
    assert_eq!(series(&today, "day", "runs").last().map(|(day, _)| *day), Some(first_day - 86_400));
    assert!(series(&today, "day", "runs").iter().all(|(_, runs)| *runs == 0));
    assert_eq!(series(&today, "today", "runs"), [(first_day, 8)]);
    assert_eq!(series(&today, "today", "contact_claktories"), [(first_day, 2)]);
    assert_eq!(series(&today, "today", "input_tokens"), [(first_day, 2000)]);
    assert_eq!(series(&today, "today", "registered_claktories"), [(first_day, 2)]);
    assert_eq!(series(&today, "rolling30", "contact_claktories").len(), 90, "rolling30 for completed days only");
    let partial = today["rows"].as_array().unwrap().iter().filter(|row| row["window"] == "today").collect::<Vec<_>>();
    assert_eq!(partial.len(), 14, "each global metric of a day, rolling30 aside");
    assert!(partial.iter().all(|row| row["status"] == "partial" && row["bucket_start_ms"].as_i64() == Some(first_day * 1000)));
    // Again later, the same rows rewritten: nothing keeps the earlier ones.
    instance.advance(60);
    assert_eq!(export(&instance)["rows"].as_array().unwrap().iter().filter(|row| row["window"] == "today").count(), 14);

    // The next day, one of them is in touch again, with more work.
    instance.advance(86_400);
    let body = check_in(vec![period("period-0000000000000999", instance.now(), vec![usage("gpt-5.6-sol", 7, 0)])]);
    instance.signed(&claktories[0], "POST", "/v1/check-in", &body, None).await;
    let out = export(&instance);
    let text = out.to_string();

    // Schema 1: these eleven columns, global only, for the last 90
    // completed days, ending yesterday.
    assert_eq!((out["schema_version"].as_i64(), out["generated_at_ms"].as_i64()), (Some(1), Some(instance.now() * 1000)));
    for row in out["rows"].as_array().unwrap() {
        let mut columns: Vec<&str> = row.as_object().unwrap().keys().map(String::as_str).collect();
        columns.sort_unstable();
        assert_eq!(
            columns,
            [
                "bucket_end_ms",
                "bucket_start_ms",
                "dimension",
                "generated_at_ms",
                "label",
                "metric",
                "published_at_ms",
                "schema_version",
                "status",
                "value",
                "window"
            ]
        );
        // Hours, Claktories per user and countries have tests of their own.
        if row["window"] == "hour" || row["dimension"] == "claktory_count" || row["dimension"] == "country" {
            continue;
        }
        let partial = row["window"] == "today";
        let status = if partial { "partial" } else { "ok" };
        assert_eq!((row["dimension"].as_str(), row["label"].as_str(), row["status"].as_str()), (Some("none"), Some("All"), Some(status)));
        assert_eq!(row["bucket_end_ms"].as_i64().unwrap() - row["bucket_start_ms"].as_i64().unwrap(), 86_400_000);
        assert!(row["value"].as_i64().unwrap() >= 0);
        // Today only as `today`, partial.
        assert_eq!(row["bucket_end_ms"].as_i64().unwrap() > (first_day + 86_400) * 1000, partial, "{row}");
    }
    let runs = series(&out, "day", "runs");
    assert_eq!(runs.len(), 90);
    assert_eq!(runs.last().unwrap().0, first_day);
    assert_eq!(runs.first().unwrap().0, first_day - 89 * 86_400);

    // The completed day's totals, over both Claktories.
    let on = |window: &str, metric: &str| series(&out, window, metric).into_iter().find(|(day, _)| *day == first_day).unwrap().1;
    assert_eq!(on("day", "contact_claktories"), 2);
    assert_eq!(on("rolling30", "contact_claktories"), 2);
    assert_eq!(on("day", "registered_claktories"), 2);
    assert_eq!(on("day", "requests"), 20);
    assert_eq!(on("day", "input_tokens"), 2000);
    assert_eq!(on("day", "output_tokens"), 10);
    assert_eq!(on("day", "cache_read_tokens"), 2000);
    assert_eq!(on("day", "cache_write_tokens"), 200);
    assert_eq!(on("day", "agent_minutes"), 180);
    assert_eq!(on("day", "runs"), 8);
    assert_eq!(on("day", "rolled_back"), 2);
    assert_eq!(on("day", "crashes"), 4);
    assert_eq!(on("day", "registered_claktories"), 2);
    let before = |metric: &str| series(&out, "day", metric).into_iter().find(|(day, _)| *day == first_day - 86_400).unwrap().1;
    assert_eq!((before("registered_claktories"), before("contact_claktories")), (0, 0));
    // Setup reached: as now, of those registered by yesterday's end.
    let snapshot = |metric: &str| series(&out, "snapshot", metric);
    assert_eq!(snapshot("model_connected_claktories"), [(first_day, 2)]);
    assert_eq!(snapshot("first_run_claktories"), [(first_day, 1)]);

    // Nothing of any one of them: no id, email, address, place finer than
    // its country, model or version.
    for private in [&claktories[0].id, &claktories[1].id, "p0@example.com", "203.0.113", "Amsterdam", "North Holland", "Lyon", "gpt-5.6", "v0.1.19", "anthropic"] {
        assert!(!text.contains(private), "{private} in the export");
    }

    // Another day on: the Claktory in touch both days counts once in the
    // 30 days, never as a sum of the days.
    instance.advance(86_400);
    let out = export(&instance);
    let day = |window: &str, at: i64| series(&out, window, "contact_claktories").into_iter().find(|(day, _)| *day == at).unwrap().1;
    assert_eq!((day("day", first_day), day("day", first_day + 86_400)), (2, 1));
    assert_eq!(day("rolling30", first_day + 86_400), 2);
}

#[test]
fn the_charts_export_leaves_the_connection_as_it_was() {
    let dir = std::env::temp_dir().join(format!("claktory-export-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("account.db");
    drop(db::open(&path).unwrap());
    // A connection that could write still doesn't, and is as it was after.
    let conn = rusqlite::Connection::open(&path).unwrap();
    stats::export(&conn, 1_795_000_000).expect("exports");
    let query_only: bool = conn.query_row("PRAGMA query_only", [], |row| row.get(0)).unwrap();
    assert!(!query_only);
    assert!(conn.is_autocommit(), "no transaction left open");
    let _ = std::fs::remove_dir_all(&dir);
}

/// Slice 2 (#411): a month's breakdowns are published once it's over,
/// write once, every category exact (Matt, Bruno 29945), and nothing of
/// any one Claktory but counts.
#[tokio::test]
async fn a_month_is_published_once_exact_and_nothing_of_any_one() {
    let instance = start_located().await;
    let mut ids = Vec::new();
    for number in 0..6 {
        // Five in the Netherlands, one in France.
        let ip = if number < 5 { format!("203.0.113.{}", 110 + number) } else { "198.51.100.115".to_owned() };
        let mut claktory = Claktory::new(&ip);
        instance.sign_in(&mut claktory, &format!("m{number}@example.com"), "Mac").await;
        // Six use Opus; five Haiku; one a model with no label of its own.
        let second = if number < 5 { usage("claude-haiku-4-5", 10, 1) } else { usage("someones-own-model", 10, 1) };
        let body = check_in(vec![period(&format!("period-00000000000077{number}0"), instance.now(), vec![usage("claude-opus-4-8", 100, 1), second])]);
        instance.signed(&claktory, "POST", "/v1/check-in", &body, None).await;
        ids.push(claktory.id.clone());
    }
    let month = stats::month_of(instance.now());
    let mut conn = db::open(&instance.db).unwrap();

    // Not before it's over.
    let early = stats::publish(&mut conn, &month, instance.now()).unwrap_err();
    assert!(early.contains("isn't over yet"), "{early}");

    // The next month, it's due once.
    let (start, end) = stats::month_bounds(&month).unwrap();
    instance.advance(end - instance.now() + 3600);
    assert!(stats::publish_due(&mut conn, instance.now()).unwrap());
    assert!(!stats::publish_due(&mut conn, instance.now()).unwrap());
    let kept = || -> (i64, String) {
        conn.query_row("SELECT published_at, rows FROM analytics_months WHERE month = ?1", [&month], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
    };
    let first = kept();
    assert_eq!(first.0, instance.now());

    // Never again, never changed, never deleted.
    instance.advance(86_400);
    let mut again = db::open(&instance.db).unwrap();
    let refused = stats::publish(&mut again, &month, instance.now()).unwrap_err();
    assert!(refused.contains("published already"), "{refused}");
    assert!(again.execute("UPDATE analytics_months SET rows = '[]'", []).is_err());
    assert!(again.execute("DELETE FROM analytics_months", []).is_err());
    assert_eq!(kept(), first);

    // In the export, bucketed by the month, timed by its publication.
    let out = export(&instance);
    let text = out.to_string();
    let rows: Vec<&serde_json::Value> = out["rows"].as_array().unwrap().iter().filter(|row| row["window"] == "month").collect();
    for row in &rows {
        assert_eq!(
            (row["bucket_start_ms"].as_i64(), row["bucket_end_ms"].as_i64(), row["published_at_ms"].as_i64()),
            (Some(start * 1000), Some(end * 1000), Some(first.0 * 1000))
        );
    }
    let value = |metric: &str, dimension: &str, label: &str| {
        rows.iter()
            .find(|row| row["metric"] == metric && row["dimension"] == dimension && row["label"] == label)
            .map(|row| row["value"].as_i64().unwrap())
    };
    assert_eq!(value("requests", "none", "All"), Some(120));
    assert_eq!(value("requests", "provider", "anthropic"), Some(120));
    // Every listed model its own, exact (Matt, Bruno 29945); the unlisted
    // one is Other.
    assert_eq!(value("requests", "model", "claude-opus-4-8"), Some(60));
    assert_eq!(value("requests", "model", "claude-haiku-4-5"), Some(50));
    assert_eq!(value("requests", "model", "Other"), Some(10));
    // Countries are exact, one included (Matt, Bruno 29402).
    assert_eq!(value("backend_claktories", "country", "Netherlands"), Some(5));
    assert_eq!(value("backend_claktories", "country", "France"), Some(1));
    assert_eq!(value("backend_claktories", "country", "Other"), None);
    assert_eq!(value("backend_claktories", "backend_version", "v0.1.9"), Some(6));
    assert_eq!(value("backend_claktories", "backend_platform", "macos-arm64"), Some(6));
    assert_eq!(value("contact_claktories", "none", "All"), Some(6));
    for private in [ids[0].as_str(), ids[5].as_str(), "someones-own-model", "Amsterdam", "North Holland", "Lyon", "m0@example.com", "203.0.113"] {
        assert!(!text.contains(private), "{private} in the export");
    }
}

#[test]
fn a_breakdown_of_one_or_two_claktories_is_exact_too() {
    let dir = std::env::temp_dir().join(format!("claktory-publish-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("account.db");
    let mut conn = db::open(&path).unwrap();
    let (start, end) = stats::month_bounds("2026-10").unwrap();
    for (id, model) in [("c1", "claude-opus-4-8"), ("c2", "claude-opus-4-8"), ("", "claude-opus-4-8")] {
        conn.execute(
            "INSERT INTO usage (claktory_id, day, provider, model, requests, input_tokens, output_tokens, cache_read_tokens, cache_write_tokens)
             VALUES (?1, ?2, 'anthropic', ?3, 5, 1, 1, 1, 1)",
            rusqlite::params![id, db::day(start + 86_400), model],
        )
        .unwrap();
    }
    stats::publish(&mut conn, "2026-10", end + 10).unwrap();
    let out: serde_json::Value = serde_json::from_str(&stats::export(&conn, end + 20).unwrap()).unwrap();
    let rows: Vec<(String, String, String, Option<i64>, String)> = out["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["window"] == "month" && row["metric"] == "requests")
        .map(|row| {
            (
                row["dimension"].as_str().unwrap().to_owned(),
                row["label"].as_str().unwrap().to_owned(),
                row["status"].as_str().unwrap().to_owned(),
                row["value"].as_i64(),
                String::new(),
            )
        })
        .collect();
    assert_eq!(
        rows,
        [
            ("none".into(), "All".into(), "ok".into(), Some(15), String::new()),
            ("provider".into(), "anthropic".into(), "ok".into(), Some(15), String::new()),
            ("model".into(), "claude-opus-4-8".into(), "ok".into(), Some(15), String::new()),
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// A database of its own, for counts set up row by row.
fn scratch(name: &str) -> (std::path::PathBuf, rusqlite::Connection) {
    let dir = std::env::temp_dir().join(format!("claktory-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let conn = db::open(&dir.join("account.db")).unwrap();
    (dir, conn)
}

fn add_account(conn: &rusqlite::Connection, id: &str, created: i64) {
    conn.execute(
        "INSERT INTO accounts (id, email, created, last_sign_in) VALUES (?1, ?1 || '@example.com', ?2, ?2)",
        rusqlite::params![id, created],
    )
    .unwrap();
}

fn add_claktory(conn: &rusqlite::Connection, id: &str, account: &str, created: i64, last_seen: i64, country: &str, revoked: Option<i64>) {
    conn.execute(
        "INSERT INTO claktories (id, account_id, name, backend_key, token_hash, epoch, created, last_seen, version, platform, revoked_at, country)
         VALUES (?1, ?2, 'Mac', 'key', ?1, 1, ?3, ?4, 'v0.1.21', 'macos-arm64', ?5, ?6)",
        rusqlite::params![id, account, created, last_seen, revoked, country],
    )
    .unwrap();
}

fn seen_on(conn: &rusqlite::Connection, claktory: &str, account: &str, at: i64) {
    conn.execute("INSERT INTO seen_days (claktory_id, account_id, day) VALUES (?1, ?2, ?3)", rusqlite::params![claktory, account, db::day(at)])
        .unwrap();
}

/// The rows of `window` and `metric`: (bucket start, dimension, label, value, status).
fn rows_of(export: &serde_json::Value, window: &str, metric: &str) -> Vec<(i64, String, String, Option<i64>, String)> {
    export["rows"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|row| row["window"] == window && row["metric"] == metric)
        .map(|row| {
            (
                row["bucket_start_ms"].as_i64().unwrap() / 1000,
                row["dimension"].as_str().unwrap().to_owned(),
                row["label"].as_str().unwrap().to_owned(),
                row["value"].as_i64(),
                row["status"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// #411 follow-up: users are accounts, each counted once however many
/// Claktories it has, by the same rules as the Claktories' counts.
#[test]
fn users_are_accounts_counted_once_whatever_their_claktories() {
    let (dir, conn) = scratch("users");
    let today = 1_795_000_000 / 86_400 * 86_400;
    let now = today + 12 * 3600 + 1800;
    let yesterday = today - 86_400;
    add_account(&conn, "a1", today - 10 * 86_400);
    add_account(&conn, "a2", yesterday + 100);
    add_account(&conn, "a3", today + 100);
    // An account with no Claktory is still a registered user.
    add_account(&conn, "a4", today - 5 * 86_400);
    add_claktory(&conn, "c1a", "a1", today - 10 * 86_400, now, "Netherlands", None);
    add_claktory(&conn, "c1b", "a1", today - 9 * 86_400, now, "Netherlands", None);
    add_claktory(&conn, "c1c", "a1", today - 9 * 86_400, now, "Netherlands", Some(today - 2 * 86_400));
    add_claktory(&conn, "c2", "a2", yesterday + 200, now, "France", None);
    add_claktory(&conn, "c3", "a3", today + 200, now, "France", None);
    seen_on(&conn, "c1a", "a1", today - 3 * 86_400);
    for (claktory, account) in [("c1a", "a1"), ("c1b", "a1"), ("c2", "a2")] {
        seen_on(&conn, claktory, account, yesterday);
    }
    seen_on(&conn, "c3", "a3", today);
    let out: serde_json::Value = serde_json::from_str(&stats::export(&conn, now).unwrap()).unwrap();
    let on = |window: &str, metric: &str, at: i64| series(&out, window, metric).into_iter().find(|(day, _)| *day == at).unwrap().1;

    assert_eq!((on("day", "registered_users", yesterday), on("day", "new_users", yesterday)), (3, 1));
    assert_eq!((on("day", "registered_users", yesterday - 86_400), on("day", "new_users", yesterday - 86_400)), (2, 0));
    // Two Claktories of one account are one user.
    assert_eq!((on("day", "contact_users", yesterday), on("day", "contact_claktories", yesterday)), (2, 3));
    assert_eq!(on("day", "contact_users", today - 3 * 86_400), 1);
    assert_eq!(on("rolling30", "contact_users", yesterday), 2);
    // Today's, partial, like the Claktories'.
    for (metric, value) in [("registered_users", 4), ("new_users", 1), ("contact_users", 1)] {
        assert_eq!(rows_of(&out, "today", metric), [(today, "none".into(), "All".into(), Some(value), "partial".into())], "{metric}");
    }
    assert!(rows_of(&out, "rolling30", "contact_users").iter().all(|(day, ..)| *day < today));

    // Claktories per user, of those registered by yesterday's end and not
    // removed: a1's two, a2's one.
    assert_eq!(series(&out, "snapshot", "claktories_per_user_x100"), [(yesterday, 150)]);
    let per_user = rows_of(&out, "snapshot", "users_with_claktories");
    assert_eq!(per_user[0], (yesterday, "none".into(), "All".into(), Some(2), "ok".into()));
    // One user with one and one with two, exact (Matt, Bruno 29945).
    assert_eq!(
        per_user[1..],
        [
            (yesterday, "claktory_count".into(), "1".into(), Some(1), "ok".into()),
            (yesterday, "claktory_count".into(), "2".into(), Some(1), "ok".into()),
            (yesterday, "claktory_count".into(), "3-5".into(), Some(0), "ok".into()),
            (yesterday, "claktory_count".into(), "6+".into(), Some(0), "ok".into()),
        ]
    );
    // By country now: those in touch in the 30 days ending yesterday, a1
    // and a2 (a3, only today, isn't yet), and their three Claktories.
    let country = |metric: &str| -> Vec<(i64, String, Option<i64>)> {
        rows_of(&out, "rolling30", metric)
            .into_iter()
            .filter(|(_, dimension, ..)| dimension == "country")
            .map(|(start, _, label, value, _)| (start, label, value))
            .collect()
    };
    assert_eq!(country("contact_users"), [(yesterday, "France".into(), Some(1)), (yesterday, "Netherlands".into(), Some(1))]);
    assert_eq!(country("contact_claktories"), [(yesterday, "Netherlands".into(), Some(2)), (yesterday, "France".into(), Some(1))]);
    let text = out.to_string();
    for private in ["a1", "c1a", "@example.com"] {
        assert!(!text.contains(private), "{private} in the export");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// #411 follow-up: each UTC hour of the last 28 days, what checked in and
/// what worked, exact (Matt, Bruno 29945), the two latest partial.
#[test]
fn each_hour_counts_what_checked_in_and_worked_exactly() {
    let (dir, conn) = scratch("hours");
    let current = 1_795_000_000 / 3600 * 3600;
    let now = current + 1800;
    let (busy, few) = (current - 3 * 3600, current - 4 * 3600);
    let period = |claktory: &str, id: &str, received: i64| {
        conn.execute(
            "INSERT INTO periods (claktory_id, id, from_time, to_time, received) VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![claktory, id, received - 3600, received, received],
        )
        .unwrap();
    };
    let worked = |claktory: &str, hour: i64, minutes: i64| {
        conn.execute(
            "INSERT INTO activity (claktory_id, hour, agent_minutes, agents_working, runs) VALUES (?1, ?2, ?3, 1, 1)",
            rusqlite::params![claktory, hour, minutes],
        )
        .unwrap();
    };
    // Six Claktories of three users, each checking in twice in the busy
    // hour and working half of it; a deleted account's work adds an hour.
    for at in 0..6 {
        let (claktory, account) = (format!("b{at}"), format!("u{}", at / 2));
        add_claktory(&conn, &claktory, &account, current - 86_400, now, "France", None);
        period(&claktory, &format!("{claktory}-1"), busy + 60);
        period(&claktory, &format!("{claktory}-2"), busy + 3000);
        worked(&claktory, busy, 30);
    }
    worked("", busy, 60);
    // Four Claktories working hard.
    for at in 0..4 {
        let claktory = format!("f{at}");
        add_claktory(&conn, &claktory, &format!("v{at}"), current - 86_400, now, "France", None);
        worked(&claktory, few, 600);
    }
    // Five checking in in the last hour, its work not in yet.
    for at in 0..5 {
        period(&format!("b{at}"), &format!("b{at}-3"), current - 3600 + 600);
    }
    // Too long ago to be in it.
    period("b0", "b0-old", current - 28 * 86_400 - 60);

    let out: serde_json::Value = serde_json::from_str(&stats::export(&conn, now).unwrap()).unwrap();
    let hours: Vec<&serde_json::Value> = out["rows"].as_array().unwrap().iter().filter(|row| row["window"] == "hour").collect();
    assert_eq!(hours.len(), 28 * 24 * 4);
    for row in &hours {
        assert_eq!((row["dimension"].as_str(), row["label"].as_str()), (Some("none"), Some("All")));
        assert_eq!(row["bucket_end_ms"].as_i64().unwrap() - row["bucket_start_ms"].as_i64().unwrap(), 3_600_000);
        assert!(row["value"].as_i64().is_some_and(|value| value >= 0) && row["status"] != "suppressed", "{row}");
    }
    let first = hours.iter().map(|row| row["bucket_start_ms"].as_i64().unwrap() / 1000).min().unwrap();
    assert_eq!(first, current - (28 * 24 - 1) * 3600);
    let at = |metric: &str, hour: i64| {
        rows_of(&out, "hour", metric).into_iter().find(|(start, ..)| *start == hour).map(|(_, _, _, value, status)| (value, status)).unwrap()
    };
    let ok = |value: i64| (Some(value), "ok".to_owned());
    // Each Claktory once in its hour, however often it checked in.
    assert_eq!(at("online_claktories", busy), ok(6));
    assert_eq!(at("working_claktories", busy), ok(6));
    // Each user once.
    assert_eq!(at("working_users", busy), ok(3));
    // 3 hours of agents' work plus the deleted account's one: 4 agents on average.
    assert_eq!(at("agents_working_avg_x100", busy), ok(400));
    assert_eq!(at("working_claktories", few), ok(4));
    assert_eq!(at("working_users", few), ok(4));
    assert_eq!(at("agents_working_avg_x100", few), ok(4000));
    // The last hour is partial; the current one is too, and empty so far.
    assert_eq!(at("online_claktories", current - 3600), (Some(5), "partial".to_owned()));
    assert_eq!(at("online_claktories", current), (Some(0), "partial".to_owned()));
    assert_eq!(at("online_claktories", busy + 3600), ok(0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// #411 follow-up: users by country in a month's publication, each user
/// once whatever their Claktories, every country exact (Matt, Bruno
/// 29402).
#[test]
fn a_month_maps_users_by_country_each_once_and_every_country_exact() {
    let (dir, mut conn) = scratch("users-by-country");
    let (start, end) = stats::month_bounds("2026-10").unwrap();
    let mid = start + 10 * 86_400;
    let user = |account: &str, claktories: &[(&str, &str, i64)]| {
        add_account(&conn, account, start - 86_400);
        for (claktory, country, last_seen) in claktories {
            add_claktory(&conn, claktory, account, start - 86_400, *last_seen, country, None);
            seen_on(&conn, claktory, account, mid);
        }
    };
    // Five users in the Netherlands, one of them with three Claktories.
    user("n1", &[("n1a", "Netherlands", mid), ("n1b", "Netherlands", mid), ("n1c", "Netherlands", mid)]);
    for at in 2..=5 {
        user(&format!("n{at}"), &[(&format!("n{at}a"), "Netherlands", mid)]);
    }
    // Four users in Germany with six Claktories.
    user("g1", &[("g1a", "Germany", mid), ("g1b", "Germany", mid), ("g1c", "Germany", mid)]);
    for at in 2..=4 {
        user(&format!("g{at}"), &[(&format!("g{at}a"), "Germany", mid)]);
    }
    // One whose Claktory last in touch is in France; one in a place that
    // isn't a country's name.
    user("f1", &[("f1a", "Germany", mid - 86_400), ("f1b", "France", mid)]);
    user("o1", &[("o1a", "Paris, 75001", mid)]);
    // A deleted account's day, under its stand-in.
    seen_on(&conn, "deleted-x", "deleted-x", mid);
    stats::publish(&mut conn, "2026-10", end + 10).unwrap();
    let out: serde_json::Value = serde_json::from_str(&stats::export(&conn, end + 20).unwrap()).unwrap();
    let users = rows_of(&out, "month", "contact_users");
    let shown: Vec<(String, String, Option<i64>)> = users.into_iter().map(|(_, dimension, label, value, _)| (dimension, label, value)).collect();
    assert_eq!(
        shown,
        [
            ("none".into(), "All".into(), Some(12)),
            ("country".into(), "Netherlands".into(), Some(5)),
            ("country".into(), "Germany".into(), Some(4)),
            ("country".into(), "France".into(), Some(1)),
            ("country".into(), "Unknown".into(), Some(1)),
            ("country".into(), "Other".into(), Some(1)),
        ]
    );
    assert!(!out.to_string().contains("75001"));
    // Claktories by country: Germany's seven, f1's first among them.
    let germany = rows_of(&out, "month", "backend_claktories").into_iter().find(|(_, dimension, label, ..)| dimension == "country" && label == "Germany");
    assert_eq!(germany.map(|row| row.3), Some(Some(7)));
    // By country only in a month and the latest 30 days, never by day or hour.
    assert!(
        out["rows"].as_array().unwrap().iter().all(|row| row["dimension"] != "country" || row["window"] == "month" || row["window"] == "rolling30")
    );
    let _ = std::fs::remove_dir_all(&dir);
}
