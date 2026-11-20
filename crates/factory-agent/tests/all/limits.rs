use factory_agent::clock::parse_iso_ms;
use factory_agent::limits::{
    UsageLimit, UsageReset, chatgpt_plan, parse_anthropic, parse_anthropic_plan, parse_anthropic_resets, parse_codex, parse_codex_resets,
};
use serde_json::json;

fn limit(id: &str, label: &str, used_percent: f64, resets_at: u64) -> UsageLimit {
    UsageLimit { id: id.into(), label: label.into(), used_percent, resets_at }
}

#[test]
fn rfc_3339_times_read_as_milliseconds_since_1970() {
    assert_eq!(parse_iso_ms("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(parse_iso_ms("2026-11-24T21:00:00Z"), Some(1_795_554_000_000));
    assert_eq!(parse_iso_ms("2026-11-24T21:00:00.573963+00:00"), Some(1_795_554_000_573));
    assert_eq!(parse_iso_ms("2026-11-24T22:30:00.5+01:30"), Some(1_795_554_000_500));
    assert_eq!(parse_iso_ms("2026-11-24T16:00:00-0500"), Some(1_795_554_000_000));
    assert_eq!(parse_iso_ms("2024-02-29T12:00:00Z"), Some(1_709_208_000_000));
    for bad in ["", "soon", "2026-13-01T00:00:00Z", "2026-11-24", "2026-11-24T21:00Z", "1969-12-31T23:59:59Z"] {
        assert_eq!(parse_iso_ms(bad), None, "{bad}");
    }
}

/// What Anthropic's OAuth usage endpoint answered for a Max account, trimmed.
#[test]
fn anthropic_reports_the_weekly_limit_and_each_model_scoped_one() {
    let usage = json!({
        "five_hour": null,
        "seven_day": {"utilization": 15.0, "resets_at": "2026-11-24T21:00:00.573963+00:00", "limit_dollars": null},
        "seven_day_oauth_apps": null,
        "seven_day_opus": null,
        "seven_day_sonnet": null,
        "extra_usage": {"is_enabled": false, "monthly_limit": 100, "utilization": 0.0},
        "limits": [
            {"kind": "weekly_all", "group": "weekly", "percent": 15, "resets_at": "2026-11-24T21:00:00.573963+00:00", "scope": null, "is_active": true},
            {"kind": "weekly_scoped", "group": "weekly", "percent": 15, "resets_at": "2026-11-24T21:00:00.574121+00:00",
             "scope": {"model": {"id": null, "display_name": "Fable"}, "surface": null}, "is_active": false}
        ]
    });
    assert_eq!(
        parse_anthropic(&usage),
        [limit("weekly", "Weekly", 15.0, 1_795_554_000_573), limit("weekly_fable", "Weekly (Fable)", 15.0, 1_795_554_000_574)]
    );
}

#[test]
fn anthropic_windows_keep_their_order_and_their_reset_may_be_unknown() {
    let usage = json!({
        "five_hour": {"utilization": 42.5, "resets_at": "2026-11-18T23:00:00Z"},
        "seven_day": {"utilization": 120.0, "resets_at": null},
        "seven_day_opus": {"utilization": 3.0, "resets_at": "2026-11-24T21:00:00Z"},
        "seven_day_sonnet": {"utilization": null},
        "limits": [
            {"kind": "weekly_scoped", "percent": 3, "resets_at": "2026-11-24T21:00:00Z", "scope": {"model": {"display_name": "Opus"}}},
            {"kind": "something_new", "percent": 50, "resets_at": "2026-11-24T21:00:00Z"}
        ]
    });
    assert_eq!(
        parse_anthropic(&usage),
        [
            limit("weekly", "Weekly", 100.0, 0),
            limit("five_hour", "5-hour", 42.5, 1_795_042_800_000),
            limit("weekly_opus", "Weekly (Opus)", 3.0, 1_795_554_000_000),
        ]
    );
    assert_eq!(parse_anthropic(&json!({})), []);
}

#[test]
fn codex_names_its_windows_by_length_with_the_weekly_one_first() {
    let now = 1_795_000_000_000;
    let usage = json!({
        "plan_type": "plus",
        "rate_limit": {
            "allowed": true,
            "limit_reached": false,
            "primary_window": {"used_percent": 7, "limit_window_seconds": 18000, "reset_after_seconds": 3600, "reset_at": 1_795_003_600},
            "secondary_window": {"used_percent": 31, "limit_window_seconds": 604_800, "reset_after_seconds": 86_400}
        },
        "credits": {"has_credits": false, "unlimited": false, "balance": "0"},
        "additional_rate_limits": [
            {"limit_name": "GPT-5.3-Codex-Spark", "metered_feature": "codex_bengalfox",
             "rate_limit": {"primary_window": {"used_percent": 0, "limit_window_seconds": 604_800, "reset_at": 1_795_500_000},
                            "secondary_window": null}}
        ]
    });
    assert_eq!(
        parse_codex(&usage, now),
        [
            limit("weekly", "Weekly", 31.0, now + 86_400_000),
            limit("five_hour", "5-hour", 7.0, 1_795_003_600_000),
            limit("weekly_gpt_5_3_codex_spark", "Weekly (GPT-5.3-Codex-Spark)", 0.0, 1_795_500_000_000),
        ]
    );
}

#[test]
fn codex_free_plans_and_unusual_windows() {
    let usage = json!({
        "rate_limit": {
            "primary_window": {"used_percent": 100, "limit_window_seconds": 2_592_000, "reset_at": 1_796_000_000},
            "secondary_window": {"used_percent": 12.5, "limit_window_seconds": 7200}
        }
    });
    assert_eq!(parse_codex(&usage, 0), [limit("30_day", "30-day", 100.0, 1_796_000_000_000), limit("2_hour", "2-hour", 12.5, 0)]);
    assert_eq!(parse_codex(&json!({"rate_limit": null}), 0), []);
}

/// What Anthropic's OAuth profile endpoint answered for a Max 20x account, trimmed.
#[test]
fn anthropic_names_the_plan_its_profile_names() {
    let profile = json!({
        "account": {"has_claude_max": true, "has_claude_pro": false},
        "organization": {
            "organization_type": "claude_max", "billing_type": "stripe_subscription",
            "rate_limit_tier": "default_claude_max_20x", "subscription_status": "active",
            "plan_display_name": "Ultra", "plan_display_label": "Ultra plan"
        }
    });
    assert_eq!(parse_anthropic_plan(&profile), "Claude Ultra");
    let named = json!({"organization": {"plan_display_name": "Claude Max"}});
    assert_eq!(parse_anthropic_plan(&named), "Claude Max");
}

#[test]
fn without_a_plan_name_anthropic_plans_come_from_the_organization_or_account() {
    let organization = |kind: &str, tier: &str| json!({"organization": {"organization_type": kind, "rate_limit_tier": tier}});
    assert_eq!(parse_anthropic_plan(&organization("claude_max", "default_claude_max_20x")), "Claude Max 20x");
    assert_eq!(parse_anthropic_plan(&organization("claude_max", "default_claude_max_5x")), "Claude Max 5x");
    assert_eq!(parse_anthropic_plan(&organization("claude_max", "")), "Claude Max");
    assert_eq!(parse_anthropic_plan(&organization("claude_pro", "default_claude_ai")), "Claude Pro");
    assert_eq!(parse_anthropic_plan(&organization("claude_team", "")), "Claude Team");
    assert_eq!(parse_anthropic_plan(&json!({"account": {"has_claude_max": true}})), "Claude Max");
    assert_eq!(parse_anthropic_plan(&json!({"account": {"has_claude_max": false, "has_claude_pro": true}})), "Claude Pro");
    assert_eq!(parse_anthropic_plan(&json!({"account": {}, "organization": {"organization_type": "api"}})), "");
}

/// A plan Claktory doesn't know yet reads as Anthropic names it, never as Max.
#[test]
fn an_unknown_anthropic_tier_reads_as_its_tier_says() {
    let organization = |kind: &str, tier: &str| json!({"organization": {"organization_type": kind, "rate_limit_tier": tier}});
    assert_eq!(parse_anthropic_plan(&organization("claude_max", "default_claude_ultra")), "Claude Ultra");
    assert_eq!(parse_anthropic_plan(&organization("claude_max", "default_claude_max_40x")), "Claude Max 40x");
    assert_eq!(parse_anthropic_plan(&organization("claude_ultra", "")), "Claude Ultra");
    assert_eq!(parse_anthropic_plan(&organization("claude_ultra", "default_claude_ultra_2x")), "Claude Ultra 2x");
    let account = json!({"account": {"has_claude_max": true}, "organization": {"rate_limit_tier": "default_claude_ultra"}});
    assert_eq!(parse_anthropic_plan(&account), "Claude Ultra");
    // A name that isn't one is passed over, as Claude Code does.
    let odd = json!({"organization": {"plan_display_name": "<b>Max</b>", "organization_type": "claude_max", "rate_limit_tier": "default_claude_max_5x"}});
    assert_eq!(parse_anthropic_plan(&odd), "Claude Max 5x");
    let named = json!({"organization": {"plan_display_name": "Max 20x", "organization_type": "claude_max"}});
    assert_eq!(parse_anthropic_plan(&named), "Claude Max 20x");
}

#[test]
fn chatgpt_plans_are_named_as_chatgpt_names_them() {
    assert_eq!(chatgpt_plan("plus"), "ChatGPT Plus");
    assert_eq!(chatgpt_plan("pro"), "ChatGPT Pro");
    assert_eq!(chatgpt_plan("free_workspace"), "ChatGPT Free Workspace");
    assert_eq!(chatgpt_plan("education"), "ChatGPT Edu");
    assert_eq!(chatgpt_plan("unknown"), "");
    assert_eq!(chatgpt_plan(""), "");
}

#[test]
fn a_chatgpt_access_token_carries_its_plan() {
    use base64::Engine;
    let claims = json!({"https://api.openai.com/auth": {"chatgpt_account_id": "account-1", "chatgpt_plan_type": "pro"}});
    let payload = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(claims.to_string());
    let token = format!("header.{payload}.signature");
    assert_eq!(factory_agent::providers::openai_codex::oauth::plan_type(&token).as_deref(), Some("pro"));
    assert_eq!(factory_agent::providers::openai_codex::oauth::plan_type("not.a.token"), None);
}

/// The usage's `cedar_ember` block, as Claude Code 2.1.327 reads it: a Fable
/// grant usable now, a full one that works only at a limit, a session one
/// during a cooldown, and a spent one.
fn cedar_ember() -> serde_json::Value {
    json!({"cedar_ember": {
        "eligible": true,
        "at_limit": false,
        "exhausted": [],
        "next_grant_id": "fable_1",
        "grants": [
            {"id": "fable_1", "label": "Fable", "resets_total": 3, "resets_left": 2, "ends_at": "2027-03-08T08:00:00Z",
             "clears": ["five_hour", "seven_day_overage_included"], "paused": false, "usable_now": true,
             "use_requires_limit": false, "gives_back_weekly": true, "blocking": [], "percent_used": {"seven_day": 80}},
            {"id": "full_1", "label": "Full", "resets_total": 1, "resets_left": 1, "ends_at": "2026-12-10T22:00:00Z",
             "clears": ["five_hour", "seven_day"], "paused": false, "usable_now": false, "use_requires_limit": true,
             "type": "full", "blocking": []},
            {"id": "spent", "resets_total": 1, "resets_left": 0, "clears": ["five_hour"], "paused": false, "usable_now": false},
            {"id": "BAD ID", "resets_left": 1, "clears": ["five_hour"], "paused": false, "usable_now": true}
        ]
    }})
}

#[test]
fn claude_resets_are_read_by_kind_with_what_they_refill_and_why_one_waits() {
    let resets = parse_anthropic_resets(&cedar_ember());
    assert_eq!(
        resets,
        [
            UsageReset {
                id: "fable_1".into(),
                label: "Fable reset".into(),
                detail: "Gives back to your weekly limit what Fable used. It also refills your 5-hour limit.".into(),
                left: 2,
                total: 3,
                expires_at: parse_iso_ms("2027-03-08T08:00:00Z").expect("time"),
                usable: true,
                unavailable: String::new(),
                window_ids: vec!["weekly_fable".into(), "weekly".into(), "five_hour".into()],
            },
            UsageReset {
                id: "full_1".into(),
                label: "Full reset".into(),
                detail: "Refills all your limits.".into(),
                left: 1,
                total: 1,
                expires_at: parse_iso_ms("2026-12-10T22:00:00Z").expect("time"),
                usable: false,
                unavailable: "It works only at a usage limit.".into(),
                window_ids: vec!["five_hour".into(), "weekly".into()],
            },
        ]
    );
}

#[test]
fn claude_resets_say_why_they_wait_and_none_show_for_an_account_not_eligible() {
    let mut usage = cedar_ember();
    usage["cedar_ember"]["cooldown_until"] = json!("2026-11-20T15:00:00Z");
    assert_eq!(parse_anthropic_resets(&usage)[1].unavailable, "A reset was just used on this account. Try again in a minute.");
    usage["cedar_ember"]["grants"][1]["unavailable_reason"] = json!("blocked_five_hour");
    assert_eq!(parse_anthropic_resets(&usage)[1].unavailable, "It can't be used while you're at your 5-hour limit.");
    usage["cedar_ember"]["grants"][1]["paused"] = json!(true);
    assert_eq!(parse_anthropic_resets(&usage)[1].unavailable, "It's paused for now.");

    usage["cedar_ember"]["eligible"] = json!(false);
    assert_eq!(parse_anthropic_resets(&usage), []);
    assert_eq!(parse_anthropic_resets(&json!({"five_hour": {"utilization": 3.0}})), [], "a usage read without the block");
}

#[test]
fn chatgpt_resets_are_counted_even_when_their_list_says_less() {
    let limits = parse_codex(
        &json!({"rate_limit": {
            "primary_window": {"used_percent": 50.0, "limit_window_seconds": 18_000},
            "secondary_window": {"used_percent": 20.0, "limit_window_seconds": 604_800}
        }}),
        0,
    );
    let unlisted = parse_codex_resets(3, &serde_json::Value::Null, &limits);
    assert_eq!(unlisted.len(), 1);
    assert_eq!((unlisted[0].id.as_str(), unlisted[0].label.as_str(), unlisted[0].left), ("", "Full reset", 3), "ChatGPT picks one");
    assert_eq!(unlisted[0].window_ids, ["weekly", "five_hour"]);
    assert!(unlisted[0].usable);

    let list = json!({"available_count": 3, "credits": [
        {"id": "a", "status": "available", "title": "Launch reset", "description": "Resets your Codex limits."}
    ]});
    let short = parse_codex_resets(3, &list, &limits);
    assert_eq!(
        (short[0].id.as_str(), short[0].label.as_str(), short[0].detail.as_str(), short[0].left),
        ("a", "Launch reset", "Resets your Codex limits.", 3)
    );
}
