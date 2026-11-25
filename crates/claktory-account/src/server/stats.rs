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
    // Claktories, and the accounts behind them (users, #411 follow-up).
    let in_touch = by_day(
        "SELECT day, count(DISTINCT claktory_id), count(DISTINCT account_id) FROM seen_days WHERE day >= ?1 AND day <= ?2 GROUP BY day",
        2,
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
        add(start, "day", "contact_users", of(&in_touch, &date, 1));
        // The 30 days ending with this one, counted once each: never a
        // sum of the days'. Only for completed days.
        if start != today {
            let since = day(start - 29 * DAY);
            let (claktories, users): (i64, i64) = conn.query_row(
                "SELECT count(DISTINCT claktory_id), count(DISTINCT account_id) FROM seen_days WHERE day >= ?1 AND day <= ?2",
                [&since, &date],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            add(start, "rolling30", "contact_claktories", claktories);
            add(start, "rolling30", "contact_users", users);
        }
        // Registered at the day's end, and not removed by then.
        let end = start + DAY;
        let registered: i64 = conn.query_row(
            "SELECT count(*) FROM claktories WHERE created < ?1 AND (revoked_at IS NULL OR revoked_at >= ?1)",
            [end],
            |row| row.get(0),
        )?;
        add(start, "day", "registered_claktories", registered);
        // Accounts at the day's end, and those made that day; deleted
        // accounts are gone from both, every day.
        let (users, new): (i64, i64) = conn.query_row(
            "SELECT count(*), coalesce(sum(created >= ?1), 0) FROM accounts WHERE created < ?2",
            [start, end],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        add(start, "day", "registered_users", users);
        add(start, "day", "new_users", new);
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
    // Claktories per user, of the same Claktories, as now: the users who
    // have one, the average, and how many have how many.
    let mut statement =
        conn.prepare("SELECT count(*) FROM claktories WHERE revoked_at IS NULL AND created < ?1 GROUP BY account_id")?;
    let per_user: Vec<i64> = statement.query_map([today], |row| row.get(0))?.collect::<rusqlite::Result<_>>()?;
    let users = per_user.len() as i64;
    let claktories: i64 = per_user.iter().sum();
    add(yesterday, "snapshot", "users_with_claktories", users);
    add(yesterday, "snapshot", "claktories_per_user_x100", if users > 0 { (claktories * 100 + users / 2) / users } else { 0 });
    for (label, value) in claktory_counts(&per_user) {
        rows.push(Row {
            published: now,
            start: yesterday,
            end: today,
            window: "snapshot".into(),
            metric: "users_with_claktories".into(),
            dimension: "claktory_count".into(),
            label: label.into(),
            value: Some(value),
            status: "ok".into(),
        });
    }
    // Users and Claktories by country now, for the map (Bruno 29945):
    // those in touch in the 30 days ending yesterday, as rolling30's are
    // counted; a Claktory in its own country, a user in their Claktory's.
    let since = day(today - 30 * DAY);
    let mut statement = conn.prepare(
        "SELECT seen.claktory_id, claktories.country, 1
         FROM (SELECT DISTINCT claktory_id FROM seen_days WHERE day >= ?1 AND day < ?2) AS seen
         LEFT JOIN claktories ON claktories.id = seen.claktory_id",
    )?;
    let claktories: Vec<Found> =
        statement.query_map([&since, &today_day], |row| Ok((row.get(0)?, row.get(1)?, vec![row.get(2)?])))?.collect::<rusqlite::Result<_>>()?;
    for (metric, found) in [("contact_users", users_in_touch(conn, &since, &today_day)?), ("contact_claktories", claktories)] {
        let mut countries = std::collections::BTreeMap::<String, Part>::new();
        for (_, country, values) in found {
            countries.entry(canonical_country(country.as_deref())).or_default().add(&values);
        }
        for (label, part) in breakdown(countries, None) {
            rows.push(Row {
                published: now,
                start: yesterday,
                end: today,
                window: "rolling30".into(),
                metric: metric.into(),
                dimension: "country".into(),
                label,
                value: part.values.first().copied(),
                status: "ok".into(),
            });
        }
    }
    rows.extend(hour_rows(conn, now)?);
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

/// Users by how many Claktories they have: `1`, `2`, `3-5` and `6+`,
/// exact, small ones included (Matt's decision, Bruno 29945: the export
/// is his alone).
fn claktory_counts(per_user: &[i64]) -> Vec<(&'static str, i64)> {
    let count = |range: std::ops::RangeInclusive<i64>| per_user.iter().filter(|n| range.contains(n)).count() as i64;
    vec![("1", count(1..=1)), ("2", count(2..=2)), ("3-5", count(3..=5)), ("6+", count(6..=i64::MAX))]
}

/// The hours an export covers: the last 28 days', the current one with
/// them, never by any dimension.
pub const EXPORT_HOURS: i64 = 28 * 24;
const HOUR: i64 = 3_600;

/// Each UTC hour of the last [`EXPORT_HOURS`] (#411 follow-up): Claktories
/// whose check-in reached the service in it, Claktories and users whose
/// agents worked in it, and how many agents worked at once on average
/// (agent minutes over 60, times 100), over all of them; exact, small
/// numbers included (Matt, Bruno 29945). The two latest hours are partial:
/// an hour's work reaches the service at the check-in after it.
fn hour_rows(conn: &Connection, now: i64) -> rusqlite::Result<Vec<Row>> {
    use std::collections::HashMap;
    let current = now - now.rem_euclid(HOUR);
    let first = current - (EXPORT_HOURS - 1) * HOUR;
    let by_hour = |sql: &str| -> rusqlite::Result<HashMap<i64, Vec<i64>>> {
        let mut statement = conn.prepare(sql)?;
        let columns = statement.column_count();
        statement
            .query_map([first], |row| {
                let numbers = (1..columns).map(|column| row.get::<_, Option<i64>>(column).map(Option::unwrap_or_default));
                Ok((row.get(0)?, numbers.collect::<rusqlite::Result<Vec<i64>>>()?))
            })?
            .collect()
    };
    let online = by_hour(
        "SELECT received - received % 3600, count(DISTINCT claktory_id) FROM periods WHERE received >= ?1 GROUP BY received - received % 3600",
    )?;
    // Deleted accounts' work (Claktory '') adds to the agents and to no
    // one's count.
    let working = by_hour(
        "SELECT hour - hour % 3600, count(DISTINCT CASE WHEN activity.claktory_id != '' THEN activity.claktory_id END),
                count(DISTINCT claktories.account_id), sum(agent_minutes)
         FROM activity LEFT JOIN claktories ON claktories.id = activity.claktory_id
         WHERE hour >= ?1 AND (agent_minutes > 0 OR runs > 0) GROUP BY hour - hour % 3600",
    )?;
    let mut rows = Vec::new();
    for start in (0..EXPORT_HOURS).map(|at| first + at * HOUR) {
        let of = |map: &HashMap<i64, Vec<i64>>, at: usize| map.get(&start).map_or(0, |numbers| numbers[at]);
        let status = if start >= current - HOUR { "partial" } else { "ok" };
        for (metric, value) in [
            ("online_claktories", of(&online, 0)),
            ("working_claktories", of(&working, 0)),
            ("working_users", of(&working, 1)),
            ("agents_working_avg_x100", of(&working, 2) * 100 / 60),
        ] {
            rows.push(Row {
                published: now,
                start,
                end: start + HOUR,
                window: "hour".into(),
                metric: metric.into(),
                dimension: "none".into(),
                label: "All".into(),
                value: Some(value),
                status: status.into(),
            });
        }
    }
    Ok(rows)
}

// ---- Monthly publications (#411, slice 2) ----

/// The most labels a breakdown other than countries has, Other among
/// them: the smallest past them are added up as Other.
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

/// The label a raw value is published under: itself when it's on `list`,
/// Unknown when there's none, else Other. Decided before anything is
/// counted, so no other text ever reaches a publication.
fn canonical(raw: Option<&str>, list: &[&str]) -> String {
    match raw.map(str::trim) {
        None | Some("" | "unknown") => "Unknown".to_owned(),
        Some(raw) => list.iter().find(|known| **known == raw).map_or_else(|| "Other".to_owned(), |known| (*known).to_owned()),
    }
}

/// A country's label: its name as the GeoIP file has it in English
/// ("Netherlands"; the only place one comes from), Unknown when there's
/// none, else Other when it isn't a country's name in form (letters,
/// spaces and `.,'()-`, 60 characters at most). Every country gets its
/// own, for the map (Matt, Bruno 29402); never a region or city.
fn canonical_country(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty() && *raw != "unknown") else { return "Unknown".to_owned() };
    let named = raw.chars().count() <= 60
        && raw.chars().next().is_some_and(char::is_alphabetic)
        && raw.chars().all(|char| char.is_alphabetic() || " .,'()-".contains(char));
    if named { raw.to_owned() } else { "Other".to_owned() }
}

/// A version's label: "v" and three numbers ("v0.1.20"), Unknown when
/// there's none, else Other. A fixed form, never free text.
fn canonical_version(raw: Option<&str>) -> String {
    let Some(raw) = raw.map(str::trim).filter(|raw| !raw.is_empty() && *raw != "unknown") else { return "Unknown".to_owned() };
    let parts: Vec<&str> = raw.strip_prefix('v').map(|rest| rest.split('.').collect()).unwrap_or_default();
    let numbers = parts.len() == 3 && parts.iter().all(|part| (1..=4).contains(&part.len()) && part.bytes().all(|byte| byte.is_ascii_digit()));
    if numbers { raw.to_owned() } else { "Other".to_owned() }
}

/// One category of a breakdown: its amounts, one per metric.
#[derive(Default, Clone)]
struct Part {
    values: Vec<i64>,
}

impl Part {
    fn add(&mut self, values: &[i64]) {
        if self.values.len() < values.len() {
            self.values.resize(values.len(), 0);
        }
        for (sum, value) in self.values.iter_mut().zip(values) {
            *sum += value;
        }
    }
}

/// A breakdown as published: every label with its exact amounts, small
/// ones included, the largest first (Matt's decision, Bruno 29402 and
/// 29945: the export is his alone). Past `most` labels, the smallest are
/// added up as Other, last.
fn breakdown(parts: std::collections::BTreeMap<String, Part>, most: Option<usize>) -> Vec<(String, Part)> {
    let mut other: Option<Part> = None;
    let mut named: Vec<(String, Part)> = Vec::new();
    for (label, part) in parts {
        if label == "Other" { other = Some(part) } else { named.push((label, part)) }
    }
    named.sort_by(|a, b| b.1.values.first().cmp(&a.1.values.first()).then(a.0.cmp(&b.0)));
    if let Some(most) = most
        && named.len() + usize::from(other.is_some()) > most
    {
        for (_, part) in named.split_off(most - 1) {
            other.get_or_insert_with(Part::default).add(&part.values);
        }
    }
    named.extend(other.map(|other| ("Other".to_owned(), other)));
    named
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

/// The users in touch from day `first` to before day `after`, each once,
/// with the country of their Claktory last in touch (one still registered
/// first), as now: (account, country, [1]).
fn users_in_touch(conn: &Connection, first: &str, after: &str) -> rusqlite::Result<Vec<Found>> {
    let mut statement = conn.prepare(
        "SELECT seen.account_id,
                (SELECT country FROM claktories WHERE claktories.account_id = seen.account_id
                 ORDER BY revoked_at IS NOT NULL, last_seen DESC LIMIT 1)
         FROM (SELECT DISTINCT account_id FROM seen_days WHERE day >= ?1 AND day < ?2) AS seen",
    )?;
    statement.query_map([first, after], |row| Ok((row.get(0)?, row.get(1)?, vec![1])))?.collect()
}

/// A month's rows: global totals, then each breakdown.
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
        for (_, raw, values) in found {
            parts.entry(label(raw.as_deref())).or_default().add(values);
        }
        parts
    };
    let users = users_in_touch(conn, &first, &after)?;
    total("contact_users", users.len() as i64);
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
    breakdowns.push((&["backend_claktories"], "country", group(&census("country")?, &canonical_country)));
    breakdowns.push((&["contact_users"], "country", group(&users, &canonical_country)));
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
        // Every country its own label, for the map.
        let most = if dimension == "country" { None } else { Some(MAX_LABELS) };
        for (label, part) in breakdown(parts, most) {
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
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(of: &[(&str, i64)], most: Option<usize>) -> Vec<(String, i64)> {
        let mut parts = std::collections::BTreeMap::<String, Part>::new();
        for (label, amount) in of {
            parts.entry((*label).to_owned()).or_default().add(&[*amount]);
        }
        breakdown(parts, most).into_iter().map(|(label, part)| (label, part.values[0])).collect()
    }

    #[test]
    fn a_breakdown_shows_every_label_exactly_the_largest_first() {
        // One is its own label; Other comes last.
        let all = shown(&[("x", 1), ("y", 20), ("Other", 3), ("x", 1)], Some(MAX_LABELS));
        assert_eq!(all, [("y".into(), 20), ("x".into(), 2), ("Other".into(), 3)]);
    }

    #[test]
    fn a_breakdown_has_twelve_labels_at_most_but_countries_all_theirs() {
        let many: Vec<(String, i64)> = (0..15_i64).map(|at| (format!("label{at:02}"), at + 1)).collect();
        let many: Vec<(&str, i64)> = many.iter().map(|(label, amount)| (label.as_str(), *amount)).collect();
        let capped = shown(&many, Some(MAX_LABELS));
        assert_eq!(capped.len(), MAX_LABELS);
        // The four smallest, 1 to 4, added up.
        assert_eq!(capped.last(), Some(&("Other".into(), 10)));
        assert_eq!(shown(&many, None).len(), 15);
    }

    #[test]
    fn only_listed_labels_are_published() {
        assert_eq!(canonical(Some("anthropic"), PROVIDERS), "anthropic");
        assert_eq!(canonical(Some("my-own-proxy"), PROVIDERS), "Other");
        assert_eq!(canonical(None, PROVIDERS), "Unknown");
        for (raw, label) in [
            (Some("Netherlands"), "Netherlands"),
            (Some("Türkiye"), "Türkiye"),
            (Some("Bosnia and Herzegovina"), "Bosnia and Herzegovina"),
            (Some("Congo (Kinshasa)"), "Congo (Kinshasa)"),
            (Some("Côte d'Ivoire"), "Côte d'Ivoire"),
            (None, "Unknown"),
            (Some(""), "Unknown"),
            (Some("Amsterdam, 1012 AB"), "Other"),
            (Some("<b>x</b>"), "Other"),
            (Some("-x"), "Other"),
        ] {
            assert_eq!(canonical_country(raw), label, "{raw:?}");
        }
        assert_eq!(canonical_country(Some(&"a".repeat(61))), "Other");
        assert_eq!(canonical(Some("unknown"), DEVICE_KINDS), "Unknown");
        assert_eq!(canonical_version(Some("v0.1.20")), "v0.1.20");
        for odd in ["2026.11.19-1958", "v0.1.20-dev", "v0.1", "v1.2.3.4", "v0.1.20 x", "v00000.1.2"] {
            assert_eq!(canonical_version(Some(odd)), "Other", "{odd}");
        }
    }

    /// Users with each of `counts` Claktories.
    fn users(counts: &[(i64, usize)]) -> Vec<i64> {
        counts.iter().flat_map(|(claktories, users)| std::iter::repeat_n(*claktories, *users)).collect()
    }

    #[test]
    fn claktories_per_user_are_exact_small_ones_included() {
        let counts = claktory_counts(&users(&[(1, 9), (2, 1), (3, 2), (5, 3), (6, 1), (10, 4)]));
        assert_eq!(counts, [("1", 9), ("2", 1), ("3-5", 5), ("6+", 5)]);
        assert_eq!(claktory_counts(&[]), [("1", 0), ("2", 0), ("3-5", 0), ("6+", 0)]);
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
