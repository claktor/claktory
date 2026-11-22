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
