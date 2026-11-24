//! The usage counts, from the service's own database and nothing else: for
//! `claktory-account stats` and the admin page.

use std::fmt::Write as _;

use rusqlite::{Connection, ToSql};

use super::db::day;

pub fn report(conn: &Connection, now: i64) -> rusqlite::Result<String> {
    let one = |sql: &str, args: &[&dyn ToSql]| -> rusqlite::Result<i64> { conn.query_row(sql, args, |row| row.get(0)) };
    let mut out = String::new();
    let today = day(now);
    let week = day(now - 6 * 86_400);
    let month = day(now - 29 * 86_400);

    let accounts = one("SELECT count(*) FROM accounts", &[])?;
    let claktories = one("SELECT count(*) FROM claktories WHERE revoked_at IS NULL", &[])?;
    let removed = one("SELECT count(*) FROM claktories WHERE revoked_at IS NOT NULL", &[])?;
    let _ = writeln!(out, "Accounts: {}", grouped(accounts));
    let _ = writeln!(out, "Claktories: {} ({} removed)", grouped(claktories), grouped(removed));
    let active = |since: &str, column: &str| one(&format!("SELECT count(DISTINCT {column}) FROM seen_days WHERE day >= ?1"), &[&since]);
    let _ = writeln!(
        out,
        "In touch today, in the last 7 days, in the last 30 days: Claktories {} / {} / {}; accounts {} / {} / {}",
        active(&today, "claktory_id")?,
        active(&week, "claktory_id")?,
        active(&month, "claktory_id")?,
        active(&today, "account_id")?,
        active(&week, "account_id")?,
        active(&month, "account_id")?,
    );
    let daily = active(&today, "claktory_id")?;
    let monthly = active(&month, "claktory_id")?;
    let ratio = if monthly > 0 { format!("{}%", daily * 100 / monthly) } else { "–".to_owned() };
    let _ = writeln!(out, "Active Claktories: daily {}, monthly {} (daily/monthly {ratio})", grouped(daily), grouped(monthly));
    // Of those first in touch 30 days ago or more, the share in touch on
    // their 30th day or later.
    let (cohort, retained): (i64, i64) = conn.query_row(
        "WITH first AS (SELECT claktory_id, min(day) AS day FROM seen_days GROUP BY claktory_id)
         SELECT count(*), coalesce(sum(EXISTS (SELECT 1 FROM seen_days s WHERE s.claktory_id = first.claktory_id
                                                 AND s.day >= date(first.day, '+30 days'))), 0)
         FROM first WHERE first.day <= date(?1, '-30 days')",
        [&today],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let share = if cohort > 0 { format!("{}%", retained * 100 / cohort) } else { "–".to_owned() };
    let _ = writeln!(
        out,
        "30-day retention: {} of {} Claktories first in touch 30 or more days ago were in touch again 30 days on or later ({share})",
        grouped(retained),
        grouped(cohort)
    );
    let (agents, projects, devices, pipes, secrets, monitors): (i64, i64, i64, i64, i64, i64) = conn.query_row(
        "SELECT coalesce(sum(agents), 0), coalesce(sum(projects), 0), coalesce(sum(devices), 0), coalesce(sum(pipes), 0),
                coalesce(sum(secrets), 0), coalesce(sum(monitors), 0)
         FROM claktories WHERE revoked_at IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
    )?;
    let _ = writeln!(
        out,
        "Now, over all Claktories: {} agents, {} projects, {} devices, {} pipes, {} secrets, {} active monitors",
        grouped(agents),
        grouped(projects),
        grouped(devices),
        grouped(pipes),
        grouped(secrets),
        grouped(monitors)
    );
    let (model, ran): (i64, i64) = conn.query_row(
        "SELECT coalesce(sum(model_connected), 0), coalesce(sum(first_run), 0) FROM claktories WHERE revoked_at IS NULL",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let _ = writeln!(
        out,
        "Setup reached: {} of {} Claktories connected a model, {} ran an agent",
        grouped(model),
        grouped(claktories),
        grouped(ran)
    );

    let (input, output, read, write): (i64, i64, i64, i64) = conn.query_row(
        "SELECT coalesce(sum(input_tokens), 0), coalesce(sum(output_tokens), 0), coalesce(sum(cache_read_tokens), 0), coalesce(sum(cache_write_tokens), 0) FROM usage",
        [],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
    )?;
    let _ = writeln!(
        out,
        "\nTokens, all time: {} (input {}, output {}, cache reads {}, cache writes {})",
        grouped(input + output + read + write),
        grouped(input),
        grouped(output),
        grouped(read),
        grouped(write)
    );
    section(
        &mut out,
        "Tokens by provider and model, all time (requests)",
        rows(
            conn,
            "SELECT provider || ' ' || model, sum(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens), sum(requests)
             FROM usage GROUP BY provider, model ORDER BY 2 DESC",
            &[],
        )?,
    );
    section(
        &mut out,
        "Tokens per day, last 30 days (requests)",
        rows(
            conn,
            "SELECT day, sum(input_tokens + output_tokens + cache_read_tokens + cache_write_tokens), sum(requests)
             FROM usage WHERE day >= ?1 GROUP BY day ORDER BY day DESC",
            &[&month],
        )?,
    );
    section(
        &mut out,
        "Claktories and accounts in touch per day, last 30 days",
        rows(
            conn,
            "SELECT day, count(DISTINCT claktory_id), count(DISTINCT account_id) FROM seen_days WHERE day >= ?1 GROUP BY day ORDER BY day DESC",
            &[&month],
        )?,
    );
    let hour = now - now.rem_euclid(3600);
    section(
        &mut out,
        "Agent hours by hour of the day (UTC), last 30 days (runs)",
        rows(
            conn,
            "SELECT printf('%02d:00', (hour % 86400) / 3600), sum(agent_minutes) / 60, sum(runs)
             FROM activity WHERE hour >= ?1 GROUP BY 1 ORDER BY 1",
            &[&(hour - 30 * 86_400)],
        )?,
    );
    section(
        &mut out,
        "Agent hours per day, last 30 days (runs)",
        rows(
            conn,
            "SELECT date(hour, 'unixepoch'), sum(agent_minutes) / 60, sum(runs) FROM activity WHERE hour >= ?1 GROUP BY 1 ORDER BY 1 DESC",
            &[&(hour - 30 * 86_400)],
        )?,
    );
    section(
        &mut out,
        "Updates rolled back and crashes by backend version, last 30 days",
        rows(
            conn,
            "SELECT version, sum(rolled_back), sum(crashes) FROM health WHERE day >= ?1 GROUP BY version
             ORDER BY sum(rolled_back) + sum(crashes) DESC, version",
            &[&month],
        )?,
    );
    section(
        &mut out,
        "Devices by system, kind and app version (Claktories)",
        rows(
            conn,
            "SELECT platform || ' ' || kind || ' ' || app_version, sum(count), count(DISTINCT claktory_id) FROM claktory_devices
             WHERE claktory_id IN (SELECT id FROM claktories WHERE revoked_at IS NULL)
             GROUP BY platform, kind, app_version ORDER BY 2 DESC, 1",
            &[],
        )?,
    );
    section(
        &mut out,
        "Devices by system",
        rows(
            conn,
            "SELECT platform, sum(count) FROM claktory_devices WHERE claktory_id IN (SELECT id FROM claktories WHERE revoked_at IS NULL)
             GROUP BY platform ORDER BY 2 DESC, 1",
            &[],
        )?,
    );
    section(
        &mut out,
        "Devices connected per day, last 30 days",
        rows(conn, "SELECT day, sum(count) FROM devices_connected WHERE day >= ?1 GROUP BY day ORDER BY day DESC", &[&month])?,
    );
    section(
        &mut out,
        "Countries",
        rows(
            conn,
            "SELECT coalesce(country, 'Unknown'), count(*) FROM claktories WHERE revoked_at IS NULL GROUP BY 1 ORDER BY 2 DESC, 1",
            &[],
        )?,
    );
    for (title, column) in [("Versions", "version"), ("Platforms", "platform")] {
        section(
            &mut out,
            title,
            rows(
                conn,
                &format!(
                    "SELECT {column}, count(*) FROM claktories WHERE revoked_at IS NULL GROUP BY {column} ORDER BY count(*) DESC, {column}"
                ),
                &[],
            )?,
        );
    }
    section(
        &mut out,
        "Claktories per account",
        rows(
            conn,
            "SELECT n, count(*) FROM (SELECT count(*) AS n FROM claktories WHERE revoked_at IS NULL GROUP BY account_id) GROUP BY n ORDER BY n",
            &[],
        )?,
    );
    section(&mut out, "Sign-ins per day, last 30", rows(conn, "SELECT day, count FROM sign_ins ORDER BY day DESC LIMIT 30", &[])?);
    section(
        &mut out,
        "Emails the mail provider refused, today and yesterday (UTC)",
        rows(
            conn,
            "SELECT substr(key, 14), sum(count) FROM limits WHERE key LIKE 'mail-refused:%' AND window >= ?1 GROUP BY key ORDER BY key",
            &[&(now - now.rem_euclid(86_400) - 86_400)],
        )?,
    );
    Ok(out)
}

fn section(out: &mut String, title: &str, rows: Vec<(String, Vec<i64>)>) {
    let _ = writeln!(out, "\n{title}:");
    for (label, numbers) in rows {
        let numbers: Vec<String> = numbers.into_iter().map(grouped).collect();
        let (first, rest) = numbers.split_first().map_or(("", &[][..]), |(first, rest)| (first.as_str(), rest));
        if rest.is_empty() {
            let _ = writeln!(out, "  {label}: {first}");
        } else {
            let _ = writeln!(out, "  {label}: {first} ({})", rest.join(", "));
        }
    }
}

/// A label, then numbers.
fn rows(conn: &Connection, sql: &str, args: &[&dyn ToSql]) -> rusqlite::Result<Vec<(String, Vec<i64>)>> {
    let mut statement = conn.prepare(sql)?;
    let columns = statement.column_count();
    statement
        .query_map(args, |row| {
            let label = match row.get::<_, rusqlite::types::Value>(0)? {
                rusqlite::types::Value::Integer(number) => number.to_string(),
                rusqlite::types::Value::Text(text) => text,
                _ => String::new(),
            };
            let numbers = (1..columns)
                .map(|column| row.get::<_, Option<i64>>(column).map(Option::unwrap_or_default))
                .collect::<rusqlite::Result<_>>()?;
            Ok((label, numbers))
        })?
        .collect()
}

/// 1234567 as "1,234,567".
pub fn grouped(number: i64) -> String {
    let digits = number.unsigned_abs().to_string();
    let mut out = String::new();
    for (at, digit) in digits.chars().enumerate() {
        if at > 0 && (digits.len() - at).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    if number < 0 { format!("-{out}") } else { out }
}

// ---- The export for Charts (#411) ----

/// The export's schema: its columns and what they mean are API.md's
/// "Usage export".
pub const EXPORT_SCHEMA: i64 = 1;
/// Completed UTC days the export covers, ending yesterday.
pub const EXPORT_DAYS: i64 = 90;
/// The most rows, and bytes, an export holds: past them it fails, rather
/// than cut anything off.
pub const EXPORT_MAX_ROWS: usize = 10_000;
pub const EXPORT_MAX_BYTES: usize = 2_000_000;

const DAY: i64 = 86_400;

/// One row of the export, as API.md's "Usage export" has it.
struct Row {
    published: i64,
    start: i64,
    end: i64,
    window: String,
    metric: String,
    dimension: String,
    label: String,
    value: Option<i64>,
    status: String,
}

/// `claktory-account stats --format charts-json` (#411): Claktory's usage
/// as global counts per completed UTC day, the last [`EXPORT_DAYS`], for
/// Charts: no identifier, label or record of any one Claktory, account or
/// device, only totals over all of them. Read in one read transaction, so
/// every number is from the same moment; `conn` should be opened read only.
/// Errors say what failed, never what was read.
pub fn export(conn: &Connection, now: i64) -> Result<String, String> {
    // Nothing here writes, whatever the caller opened; as it was after.
    let was: bool = conn.query_row("PRAGMA query_only", [], |row| row.get(0)).map_err(|error| format!("couldn't start reading: {error}"))?;
    conn.execute_batch("PRAGMA query_only = ON; BEGIN").map_err(|error| format!("couldn't start reading: {error}"))?;
    let rows = export_rows(conn, now);
    let _ = conn.execute_batch("ROLLBACK");
    if !was {
        let _ = conn.execute_batch("PRAGMA query_only = OFF");
    }
    let rows = rows.map_err(|error| format!("couldn't read the counts: {error}"))?;
    if rows.len() > EXPORT_MAX_ROWS {
        return Err(format!("the export would have {} rows, over the {EXPORT_MAX_ROWS} it may", rows.len()));
    }
    let generated = now * 1000;
    let rows: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|row| {
            serde_json::json!({
                "schema_version": EXPORT_SCHEMA,
                "generated_at_ms": generated,
                "published_at_ms": row.published * 1000,
                "bucket_start_ms": row.start * 1000,
                "bucket_end_ms": row.end * 1000,
                "window": row.window,
                "metric": row.metric,
                "dimension": row.dimension,
                "label": row.label,
                "value": row.value,
                "status": row.status,
            })
        })
        .collect();
    let out = serde_json::to_string(&serde_json::json!({"schema_version": EXPORT_SCHEMA, "generated_at_ms": generated, "rows": rows}))
        .map_err(|error| error.to_string())?;
    if out.len() > EXPORT_MAX_BYTES {
        return Err(format!("the export would be {} bytes, over the {EXPORT_MAX_BYTES} it may", out.len()));
    }
    Ok(out)
}

fn export_rows(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Row>> {
    let today = now - now.rem_euclid(DAY);
    let first = today - EXPORT_DAYS * DAY;
    let days: Vec<i64> = (0..EXPORT_DAYS).map(|at| first + at * DAY).collect();
    let (first_day, today_day) = (day(first), day(today));

    // Per day, by its date: one query each, over the days covered.
    let by_day = |sql: &str, columns: usize, args: &[&dyn ToSql]| -> rusqlite::Result<std::collections::HashMap<String, Vec<i64>>> {
        let mut statement = conn.prepare(sql)?;
        statement
            .query_map(args, |row| {
                let numbers = (1..=columns).map(|column| row.get::<_, Option<i64>>(column).map(Option::unwrap_or_default));
                Ok((row.get::<_, String>(0)?, numbers.collect::<rusqlite::Result<Vec<i64>>>()?))
            })?
            .collect()
    };
    let in_touch = by_day(
        "SELECT day, count(DISTINCT claktory_id) FROM seen_days WHERE day >= ?1 AND day <= ?2 GROUP BY day",
        1,
        &[&first_day, &today_day],
    )?;
    let usage = by_day(
        "SELECT day, sum(requests), sum(input_tokens), sum(output_tokens), sum(cache_read_tokens), sum(cache_write_tokens)
         FROM usage WHERE day >= ?1 AND day <= ?2 GROUP BY day",
        5,
        &[&first_day, &today_day],
    )?;
    let activity = by_day(
        "SELECT date(hour - hour % 86400, 'unixepoch'), sum(agent_minutes), sum(runs) FROM activity
         WHERE hour >= ?1 AND hour < ?2 GROUP BY hour - hour % 86400",
        2,
        &[&first, &(today + DAY)],
    )?;
    let health = by_day(
        "SELECT day, sum(rolled_back), sum(crashes) FROM health WHERE day >= ?1 AND day <= ?2 GROUP BY day",
        2,
        &[&first_day, &today_day],
    )?;

    let mut rows = Vec::new();
    // Today, so far (slice 3, Fugo 27512): its own window, `partial`,
    // never passed off as a completed day.
    let mut add = |start: i64, window: &str, metric: &str, value: i64| {
        let (window, status) = if start == today { ("today", "partial") } else { (window, "ok") };
        rows.push(Row {
            published: now,
            start,
            end: start + DAY,
            window: window.into(),
            metric: metric.into(),
            dimension: "none".into(),
            label: "All".into(),
            value: Some(value),
            status: status.into(),
        });
    };
    let of = |map: &std::collections::HashMap<String, Vec<i64>>, date: &str, at: usize| map.get(date).map_or(0, |numbers| numbers[at]);
    for &start in days.iter().chain([&today]) {
        let date = day(start);
        add(start, "day", "contact_claktories", of(&in_touch, &date, 0));
        // The 30 days ending with this one, counted once each: never a
        // sum of the days'. Only for completed days.
        if start != today {
            let since = day(start - 29 * DAY);
            let rolling: i64 = conn.query_row(
                "SELECT count(DISTINCT claktory_id) FROM seen_days WHERE day >= ?1 AND day <= ?2",
                [&since, &date],
                |row| row.get(0),
            )?;
            add(start, "rolling30", "contact_claktories", rolling);
        }
        // Registered at the day's end, and not removed by then.
        let end = start + DAY;
        let registered: i64 = conn.query_row(
            "SELECT count(*) FROM claktories WHERE created < ?1 AND (revoked_at IS NULL OR revoked_at >= ?1)",
            [end],
            |row| row.get(0),
        )?;
        add(start, "day", "registered_claktories", registered);
        for (at, metric) in ["requests", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens"].into_iter().enumerate() {
            add(start, "day", metric, of(&usage, &date, at));
        }
        add(start, "day", "agent_minutes", of(&activity, &date, 0));
        add(start, "day", "runs", of(&activity, &date, 1));
        add(start, "day", "rolled_back", of(&health, &date, 0));
        add(start, "day", "crashes", of(&health, &date, 1));
    }
    // Setup reached has no time of its own: what the Claktories registered
    // by yesterday's end, and not removed, have reached as of now.
    let yesterday = today - DAY;
    let (model, ran): (i64, i64) = conn.query_row(
        "SELECT coalesce(sum(model_connected), 0), coalesce(sum(first_run), 0) FROM claktories WHERE revoked_at IS NULL AND created < ?1",
        [today],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    add(yesterday, "snapshot", "model_connected_claktories", model);
    add(yesterday, "snapshot", "first_run_claktories", ran);
    // The latest months published, as they were published.
    let mut statement = conn.prepare("SELECT month, published_at, rows FROM analytics_months ORDER BY month DESC LIMIT ?1")?;
    let months: Vec<(String, i64, String)> =
        statement.query_map([EXPORT_MONTHS], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?.collect::<rusqlite::Result<_>>()?;
    for (month, published, kept) in months.into_iter().rev() {
        let (Some((start, end)), Ok(kept)) = (month_bounds(&month), serde_json::from_str::<Vec<Published>>(&kept)) else {
            tracing::error!(month, "a publication couldn't be read; it's left out");
            continue;
        };
        for row in kept {
            rows.push(Row {
                published,
                start,
                end,
                window: row.window,
                metric: row.metric,
                dimension: row.dimension,
                label: row.label,
                value: row.value,
                status: row.status,
            });
        }
    }
    Ok(rows)
}

// ---- Monthly publications (#411, slice 2) ----

/// The fewest distinct Claktories behind a category for it to be
/// published on its own (Fugo): fewer, it's folded into Other.
pub const SUPPORT: usize = 5;
/// The most labels a breakdown has, Other among them.
pub const MAX_LABELS: usize = 12;
/// The most publications an export carries, the latest.
pub const EXPORT_MONTHS: i64 = 6;

/// Providers, as Claktory names them.
const PROVIDERS: &[&str] = &["anthropic", "openai", "openai-codex", "openrouter", "opencode"];
/// Models with a label of their own; any other is Other. A new model gets
/// one only through a reviewed change here.
const MODELS: &[&str] = &[
    "claude-fable-5",
    "claude-haiku-4-5",
    "claude-haiku-4-5-20251001",
    "claude-opus-4-5",
    "claude-opus-4-5-20251101",
    "claude-opus-4-6",
    "claude-opus-4-7",
    "claude-opus-4-8",
    "claude-opus-5",
    "claude-opus-5-5",
    "claude-sonnet-4-5",
    "claude-sonnet-4-5-20250929",
    "claude-sonnet-4-6",
    "claude-sonnet-5",
    "claude-sonnet-5-5",
    "gpt-5.3-codex-spark",
    "gpt-5.4",
    "gpt-5.4-mini",
    "gpt-5.5",
    "gpt-5.6-luna",
    "gpt-5.6-sol",
    "gpt-5.6-terra",
    "gpt-6.1-sol",
];
/// Backend platforms, as factoryd names them ("macos-arm64").
const BACKEND_PLATFORMS: &[&str] = &["macos-arm64", "macos-x86_64", "linux-arm64", "linux-x86_64"];
/// Device systems and kinds, as the apps name them.
const DEVICE_PLATFORMS: &[&str] = &["macos", "ios", "ipados", "android", "linux", "windows"];
const DEVICE_KINDS: &[&str] = &["desktop", "laptop", "phone", "tablet"];
/// Countries with a label of their own, as the GeoIP database names them
/// in English; any other is Other.
const COUNTRIES: &[&str] = &[
    "Argentina",
    "Australia",
    "Austria",
    "Bangladesh",
    "Belgium",
    "Brazil",
    "Bulgaria",
    "Canada",
    "Chile",
    "China",
    "Colombia",
    "Croatia",
    "Czechia",
    "Denmark",
    "Egypt",
    "Estonia",
    "Finland",
    "France",
    "Germany",
    "Greece",
    "Hong Kong",
    "Hungary",
    "India",
    "Indonesia",
    "Ireland",
    "Israel",
    "Italy",
    "Japan",
    "Kenya",
    "Latvia",
    "Lithuania",
    "Luxembourg",
    "Malaysia",
    "Mexico",
    "Netherlands",
    "New Zealand",
    "Nigeria",
    "Norway",
    "Pakistan",
    "Peru",
    "Philippines",
    "Poland",
    "Portugal",
    "Romania",
    "Serbia",
    "Singapore",
    "Slovakia",
    "Slovenia",
    "South Africa",
    "South Korea",
    "Spain",
    "Sweden",
    "Switzerland",
    "Taiwan",
    "Thailand",
    "Türkiye",
    "Ukraine",
    "United Arab Emirates",
    "United Kingdom",
    "United States",
    "Vietnam",
];

/// The label a raw value is published under: itself when it's on `list`,
/// Unknown when there's none, else Other. Decided before anything is
/// counted, so no other text ever reaches a publication.
fn canonical(raw: Option<&str>, list: &[&str]) -> String {
    match raw.map(str::trim) {
        None | Some("" | "unknown") => "Unknown".to_owned(),
        Some(raw) => list.iter().find(|known| **known == raw).map_or_else(|| "Other".to_owned(), |known| (*known).to_owned()),
    }
}

/// A version's label: "v" and three numbers ("v0.1.20"), Unknown when
/// there's none, else Other. A fixed form, never free text.
fn canonical_version(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty() && *raw != "unknown") else { return "Unknown".to_owned() };
    let parts: Vec<&str> = raw.strip_prefix('v').map(|rest| rest.split('.').collect()).unwrap_or_default();
    let numbers = parts.len() == 3 && parts.iter().all(|part| (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit()));
    if numbers { raw.to_owned() } else { "Other".to_owned() }
}

/// One category of a breakdown: its amounts (one per metric) and the
/// Claktories behind them. Anonymous totals (deleted accounts', id '')
/// add to the amounts and to no one's support.
#[derive(Default, Clone)]
struct Part {
    values: Vec<i64>,
    behind: std::collections::HashSet<String>,
}

impl Part {
    fn add(&mut self, claktory: &str, values: &[i64]) {
        if self.values.len() < values.len() {
            self.values.resize(values.len(), 0);
        }
        for (sum, value) in self.values.iter_mut().zip(values) {
            *sum += value;
        }
        if !claktory.is_empty() {
            self.behind.insert(claktory.to_owned());
        }
    }

    fn absorb(&mut self, other: Part) {
        if self.values.len() < other.values.len() {
            self.values.resize(other.values.len(), 0);
        }
        for (sum, value) in self.values.iter_mut().zip(&other.values) {
            *sum += value;
        }
        self.behind.extend(other.behind);
    }
}

/// A breakdown as published (Fugo's rules, analytics-charts.md §3): at
/// most [`MAX_LABELS`] labels, the rest folded into Other; then every
/// category with fewer than [`SUPPORT`] distinct Claktories behind it
/// folded into Other too; then, while Other has fewer, the next smallest
/// category absorbed into it. When even all of it together has too few,
/// None: it's suppressed, and only the global total stands.
fn disclosed(parts: std::collections::BTreeMap<String, Part>) -> Option<Vec<(String, Part)>> {
    if parts.is_empty() {
        return Some(Vec::new());
    }
    let mut other: Option<Part> = None;
    let fold = |other: &mut Option<Part>, part: Part| other.get_or_insert_with(Part::default).absorb(part);
    let mut named: Vec<(String, Part)> = Vec::new();
    for (label, part) in parts {
        if label == "Other" { fold(&mut other, part) } else { named.push((label, part)) }
    }
    // Largest first: by Claktories behind, then the first amount, then label.
    let order = |a: &(String, Part), b: &(String, Part)| {
        b.1.behind.len().cmp(&a.1.behind.len()).then(b.1.values.first().cmp(&a.1.values.first())).then(a.0.cmp(&b.0))
    };
    named.sort_by(order);
    if named.len() + usize::from(other.is_some()) > MAX_LABELS {
        for (_, part) in named.split_off(MAX_LABELS - 1) {
            fold(&mut other, part);
        }
    }
    let (kept, small): (Vec<_>, Vec<_>) = named.into_iter().partition(|(_, part)| part.behind.len() >= SUPPORT);
    named = kept;
    for (_, part) in small {
        fold(&mut other, part);
    }
    while other.as_ref().is_some_and(|other| other.behind.len() < SUPPORT) {
        let smallest = named.pop()?;
        fold(&mut other, smallest.1);
    }
    if let Some(other) = other {
        named.push(("Other".to_owned(), other));
    }
    Some(named)
}

/// A publication's row: bucketed by its month, timed by the publication.
#[derive(serde::Serialize, serde::Deserialize)]
struct Published {
    window: String,
    metric: String,
    dimension: String,
    label: String,
    value: Option<i64>,
    status: String,
}

/// `YYYY-MM`'s bounds, unix seconds, end exclusive.
pub fn month_bounds(month: &str) -> Option<(i64, i64)> {
    let (year, number) = month.split_once('-')?;
    if month.len() != 7 || year.len() != 4 || number.len() != 2 {
        return None;
    }
    let start = jiff::civil::Date::new(year.parse().ok()?, number.parse().ok()?, 1).ok()?;
    let next = start.checked_add(jiff::Span::new().months(1)).ok()?;
    let epoch = jiff::civil::Date::new(1970, 1, 1).ok()?;
    let second = |date: jiff::civil::Date| date.since(epoch).ok().map(|span| i64::from(span.get_days()) * DAY);
    Some((second(start)?, second(next)?))
}

/// The month `unix` is in, `YYYY-MM` (UTC).
pub fn month_of(unix: i64) -> String {
    jiff::Timestamp::from_second(unix).map(|time| time.strftime("%Y-%m").to_string()).unwrap_or_default()
}

/// The month before the one `now` is in: the one due for publication.
pub fn last_month(now: i64) -> String {
    let start = month_bounds(&month_of(now)).map_or(now, |(start, _)| start);
    month_of(start - 1)
}

/// Publishes the month before `now`'s, once: what the service's hourly
/// upkeep does. Ok(true) when it published it now.
pub fn publish_due(conn: &mut Connection, now: i64) -> Result<bool, String> {
    let month = last_month(now);
    let done: bool = conn
        .query_row("SELECT EXISTS (SELECT 1 FROM analytics_months WHERE month = ?1)", [&month], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if done {
        return Ok(false);
    }
    publish(conn, &month, now).map(|()| true)
}

/// Publishes `month`'s breakdowns, write once: a month already published
/// is refused, and logged, never changed (the table refuses it too). Only
/// a month that's over. Its counts of what Claktories last reported
/// (versions, platforms, countries, devices) are taken now, `now` being
/// their time: they're never made up for a month already past.
pub fn publish(conn: &mut Connection, month: &str, now: i64) -> Result<(), String> {
    let (start, end) = month_bounds(month).ok_or_else(|| format!("{month} isn't a month (YYYY-MM)"))?;
    if end > now {
        return Err(format!("{month} isn't over yet"));
    }
    let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(|error| error.to_string())?;
    let done: bool = tx
        .query_row("SELECT EXISTS (SELECT 1 FROM analytics_months WHERE month = ?1)", [month], |row| row.get(0))
        .map_err(|error| error.to_string())?;
    if done {
        tracing::warn!(month, "a publication of a month already published was refused");
        return Err(format!("{month} is published already, and a published month is never changed"));
    }
    let rows = month_rows(&tx, start, end).map_err(|error| format!("couldn't count {month}: {error}"))?;
    let rows = serde_json::to_string(&rows).map_err(|error| error.to_string())?;
    tx.execute("INSERT INTO analytics_months (month, published_at, rows) VALUES (?1, ?2, ?3)", rusqlite::params![month, now, rows])
        .map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    tracing::info!(month, "published the month's usage breakdowns");
    Ok(())
}

/// A row a breakdown is counted from: the Claktory ('' for anonymous
/// totals), its raw label, and its amounts.
type Found = (String, Option<String>, Vec<i64>);

/// A month's rows: global totals, then each breakdown as disclosed.
fn month_rows(conn: &Connection, start: i64, end: i64) -> rusqlite::Result<Vec<Published>> {
    use std::collections::BTreeMap;
    let (first, after) = (day(start), day(end));
    let mut rows = Vec::new();
    let mut total = |metric: &str, value: i64| {
        rows.push(Published {
            window: "month".into(),
            metric: metric.into(),
            dimension: "none".into(),
            label: "All".into(),
            value: Some(value),
            status: "ok".into(),
        });
    };
    let in_touch: i64 = conn.query_row(
        "SELECT count(DISTINCT claktory_id) FROM seen_days WHERE day >= ?1 AND day < ?2",
        [&first, &after],
        |row| row.get(0),
    )?;
    total("contact_claktories", in_touch);

    const USAGE: [&str; 5] = ["requests", "input_tokens", "output_tokens", "cache_read_tokens", "cache_write_tokens"];
    const HEALTH: [&str; 2] = ["rolled_back", "crashes"];
    // (claktory, label, amounts), read once per breakdown.
    let read = |sql: &str, args: &[&dyn ToSql], width: usize| -> rusqlite::Result<Vec<Found>> {
        let mut statement = conn.prepare(sql)?;
        statement
            .query_map(args, |row| {
                let values = (2..2 + width).map(|at| row.get::<_, Option<i64>>(at).map(Option::unwrap_or_default));
                Ok((row.get(0)?, row.get(1)?, values.collect::<rusqlite::Result<Vec<i64>>>()?))
            })?
            .collect()
    };
    let group = |found: &[Found], label: &dyn Fn(Option<&str>) -> String| {
        let mut parts: BTreeMap<String, Part> = BTreeMap::new();
        for (claktory, raw, values) in found {
            parts.entry(label(raw.as_deref())).or_default().add(claktory, values);
        }
        parts
    };
    let mut breakdowns: Vec<(&[&str], &str, BTreeMap<String, Part>)> = Vec::new();

    let by = |column: &str| {
        format!(
            "SELECT claktory_id, {column}, sum(requests), sum(input_tokens), sum(output_tokens), sum(cache_read_tokens), sum(cache_write_tokens)
             FROM usage WHERE day >= ?1 AND day < ?2 GROUP BY claktory_id, {column}"
        )
    };
    let usage = read(&by("provider"), &[&first, &after], 5)?;
    let sums = usage.iter().fold(vec![0; 5], |mut sums, (_, _, values)| {
        sums.iter_mut().zip(values).for_each(|(sum, value)| *sum += value);
        sums
    });
    for (metric, sum) in USAGE.iter().zip(&sums) {
        total(metric, *sum);
    }
    breakdowns.push((&USAGE[..], "provider", group(&usage, &|raw| canonical(raw, PROVIDERS))));
    let models = read(&by("model"), &[&first, &after], 5)?;
    breakdowns.push((&USAGE[..], "model", group(&models, &|raw| canonical(raw, MODELS))));

    let health = read(
        "SELECT claktory_id, version, sum(rolled_back), sum(crashes) FROM health WHERE day >= ?1 AND day < ?2 GROUP BY claktory_id, version",
        &[&first, &after],
        2,
    )?;
    let sums = health.iter().fold(vec![0; 2], |mut sums, (_, _, values)| {
        sums.iter_mut().zip(values).for_each(|(sum, value)| *sum += value);
        sums
    });
    for (metric, sum) in HEALTH.iter().zip(&sums) {
        total(metric, *sum);
    }
    breakdowns.push((&HEALTH[..], "backend_version", group(&health, &canonical_version)));

    // What Claktories last reported, as now: those registered by the
    // month's end and not removed.
    let census = |column: &str| {
        read(
            &format!("SELECT id, {column}, 1 FROM claktories WHERE revoked_at IS NULL AND created < ?1"),
            &[&end],
            1,
        )
    };
    let versions = census("version")?;
    total("backend_claktories", versions.len() as i64);
    breakdowns.push((&["backend_claktories"], "backend_version", group(&versions, &canonical_version)));
    breakdowns.push((&["backend_claktories"], "backend_platform", group(&census("platform")?, &|raw| canonical(raw, BACKEND_PLATFORMS))));
    breakdowns.push((&["backend_claktories"], "country", group(&census("country")?, &|raw| canonical(raw, COUNTRIES))));
    let devices = |column: &str| {
        read(
            &format!(
                "SELECT claktory_id, {column}, sum(count) FROM claktory_devices
                 WHERE claktory_id IN (SELECT id FROM claktories WHERE revoked_at IS NULL AND created < ?1) GROUP BY claktory_id, {column}"
            ),
            &[&end],
            1,
        )
    };
    let platforms = devices("platform")?;
    total("reported_devices", platforms.iter().map(|(_, _, values)| values[0]).sum());
    breakdowns.push((&["reported_devices"], "device_platform", group(&platforms, &|raw| canonical(raw, DEVICE_PLATFORMS))));
    breakdowns.push((&["reported_devices"], "device_kind", group(&devices("kind")?, &|raw| canonical(raw, DEVICE_KINDS))));
    breakdowns.push((&["reported_devices"], "app_version", group(&devices("app_version")?, &canonical_version)));

    for (metrics, dimension, parts) in breakdowns {
        match disclosed(parts) {
            Some(shown) => {
                for (label, part) in shown {
                    for (at, metric) in metrics.iter().enumerate() {
                        rows.push(Published {
                            window: "month".into(),
                            metric: (*metric).into(),
                            dimension: dimension.into(),
                            label: label.clone(),
                            value: Some(part.values.get(at).copied().unwrap_or_default()),
                            status: "ok".into(),
                        });
                    }
                }
            }
            None => {
                for metric in metrics {
                    rows.push(Published {
                        window: "month".into(),
                        metric: (*metric).into(),
                        dimension: dimension.into(),
                        label: "Other".into(),
                        value: None,
                        status: "suppressed".into(),
                    });
                }
            }
        }
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A breakdown of `(label, claktories behind it, amount)`.
    fn parts(of: &[(&str, &[&str], i64)]) -> std::collections::BTreeMap<String, Part> {
        let mut parts = std::collections::BTreeMap::<String, Part>::new();
        for (label, behind, amount) in of {
            let part = parts.entry((*label).to_owned()).or_default();
            if behind.is_empty() {
                part.add("", &[*amount]);
            }
            for (at, claktory) in behind.iter().enumerate() {
                part.add(claktory, &[if at == 0 { *amount } else { 0 }]);
            }
        }
        parts
    }

    fn shown(parts: std::collections::BTreeMap<String, Part>) -> Option<Vec<(String, usize, i64)>> {
        disclosed(parts).map(|shown| shown.into_iter().map(|(label, part)| (label, part.behind.len(), part.values[0])).collect())
    }

    #[test]
    fn a_category_needs_five_claktories_and_overlaps_count_once() {
        let five = ["a", "b", "c", "d", "e"];
        // Two groups of three that overlap are four Claktories, not six:
        // too few, together or apart.
        assert_eq!(shown(parts(&[("x", &["a", "b", "c"], 10), ("y", &["b", "c", "d"], 20)])), None);
        // Five behind it: its own label.
        assert_eq!(shown(parts(&[("x", &five, 7)])), Some(vec![("x".into(), 5, 7)]));
        // A small one folds into Other, which, still too small, takes in the
        // next smallest until it's safe.
        let folded = shown(parts(&[("big", &["a", "b", "c", "d", "e", "f"], 60), ("mid", &five, 50), ("tiny", &["z"], 1)]));
        assert_eq!(folded, Some(vec![("big".into(), 6, 60), ("Other".into(), 6, 51)]));
    }

    #[test]
    fn anonymous_totals_count_in_amounts_but_never_toward_five() {
        let four = ["a", "b", "c", "d"];
        assert_eq!(shown(parts(&[("x", &four, 10), ("x", &[], 1000)])), None);
        let five = ["a", "b", "c", "d", "e"];
        assert_eq!(shown(parts(&[("x", &five, 10), ("x", &[], 1000)])), Some(vec![("x".into(), 5, 1010)]));
    }

    #[test]
    fn a_breakdown_has_twelve_labels_at_most() {
        let mut map = std::collections::BTreeMap::<String, Part>::new();
        for at in 0..15_i64 {
            for one in 0..5 {
                map.entry(format!("label{at:02}")).or_default().add(&format!("{at}-{one}"), &[at]);
            }
        }
        let shown = shown(map).expect("shown");
        assert_eq!(shown.len(), MAX_LABELS);
        assert_eq!(shown.last().map(|(label, behind, _)| (label.as_str(), *behind)), Some(("Other", 20)));
    }

    #[test]
    fn only_listed_labels_are_published() {
        assert_eq!(canonical(Some("anthropic"), PROVIDERS), "anthropic");
        assert_eq!(canonical(Some("my-own-proxy"), PROVIDERS), "Other");
        assert_eq!(canonical(None, COUNTRIES), "Unknown");
        assert_eq!(canonical(Some("unknown"), DEVICE_KINDS), "Unknown");
        assert_eq!(canonical_version(Some("v0.1.20")), "v0.1.20");
        for odd in ["2026.11.19-1958", "v0.1.20-dev", "v0.1", "v1.2.3.4", "v0.1.20 x", "v00000.1.2"] {
            assert_eq!(canonical_version(Some(odd)), "Other", "{odd}");
        }
    }

    #[test]
    fn months_have_their_bounds_and_the_one_due_is_the_last() {
        let (start, end) = month_bounds("2026-11").expect("a month");
        assert_eq!((month_of(start), month_of(end - 1), month_of(end)), ("2026-11".into(), "2026-11".into(), "2026-12".into()));
        assert_eq!(end - start, 30 * DAY);
        assert_eq!(last_month(month_bounds("2027-01").unwrap().0 + 5), "2026-12");
        assert!(month_bounds("2026-13").is_none() && month_bounds("26-1").is_none());
    }
}
