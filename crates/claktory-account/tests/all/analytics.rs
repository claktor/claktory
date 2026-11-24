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
        .filter(|row| row["window"] == window && row["metric"] == metric)
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
    assert_eq!(partial.len(), 11, "each global metric of a day, rolling30 aside");
    assert!(partial.iter().all(|row| row["status"] == "partial" && row["bucket_start_ms"].as_i64() == Some(first_day * 1000)));
    // Again later, the same rows rewritten: nothing keeps the earlier ones.
    instance.advance(60);
    assert_eq!(export(&instance)["rows"].as_array().unwrap().iter().filter(|row| row["window"] == "today").count(), 11);

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

    // Nothing of any one of them: no id, email, address, place, model or version.
    for private in [&claktories[0].id, &claktories[1].id, "p0@example.com", "203.0.113", "Netherlands", "Amsterdam", "gpt-5.6", "v0.1.19", "anthropic"] {
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
/// write once, each category with five Claktories behind it or folded
/// into Other, and nothing of any one Claktory.
#[tokio::test]
async fn a_month_is_published_once_with_small_groups_folded_and_nothing_of_any_one() {
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
    // Opus has six behind it; Haiku's five go into Other, which the one
    // unlisted model alone couldn't make safe.
    assert_eq!(value("requests", "model", "claude-opus-4-8"), Some(60));
    assert_eq!(value("requests", "model", "Other"), Some(60));
    assert_eq!(value("requests", "model", "claude-haiku-4-5"), None);
    // France alone is too few, and Other then takes in the Netherlands.
    assert_eq!(value("backend_claktories", "country", "Other"), Some(6));
    assert_eq!(value("backend_claktories", "backend_version", "v0.1.9"), Some(6));
    assert_eq!(value("backend_claktories", "backend_platform", "macos-arm64"), Some(6));
    assert_eq!(value("contact_claktories", "none", "All"), Some(6));
    for private in [ids[0].as_str(), ids[5].as_str(), "someones-own-model", "France", "Netherlands", "Amsterdam", "m0@example.com", "203.0.113"] {
        assert!(!text.contains(private), "{private} in the export");
    }
}

#[test]
fn too_few_claktories_leave_a_breakdown_suppressed_and_only_its_total() {
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
            ("provider".into(), "Other".into(), "suppressed".into(), None, String::new()),
            ("model".into(), "Other".into(), "suppressed".into(), None, String::new()),
        ]
    );
    let _ = std::fs::remove_dir_all(&dir);
}
