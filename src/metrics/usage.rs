//! Context, rate limits, cost, prompt cache and session time.

use super::projection::{self, Limit};
use super::{Ctx, LEVELS, Metric, MetricMeta, OptKind, OptSpec, Options, Reading, fmt};
use crate::input::{ContextWindow, QuotaKind, SpendLimit, Window};
use crate::style::{Category, GlyphTier, IconKey, Role, Severity};
use crate::version::gate;
use crate::widgets::WidgetKind;

const FIVE_HOURS: i64 = 5 * 3600;
const SEVEN_DAYS: i64 = 7 * 86_400;

/// Thresholds for the cache countdown, in percent of the cache lifetime
/// already used up (the reading drains; see `Reading::draining`). They are
/// shares of the TTL so a 5-minute and a 1-hour cache warn alike:
/// - warn at 75%: the last quarter, 1:15 left of 5m or 15:00 of 1h, enough
///   to send a prompt and keep the cache;
/// - crit at 90%: the last tenth, 0:30 left of 5m or 6:00 of 1h, about to go
///   cold.
///
/// Plenty left (under 75%) keeps the item's own color.
const CACHE_LEVELS: &[(f64, Severity)] = &[(75.0, Severity::Warn), (90.0, Severity::Crit)];

fn meta(
    id: &'static str,
    category: Category,
    name: &'static str,
    summary: &'static str,
    icon: IconKey,
) -> MetricMeta {
    MetricMeta {
        id,
        category,
        name,
        summary,
        icon,
        widget: WidgetKind::Text,
        templates: ["", "", ""],
        fields: &[],
        levels: &[],
        min_cc: None,
        needs_mod: false,
        options: &[],
        stateful: false,
        priority: 3,
    }
}

const WINDOW_FIELDS: &[&str] = &["pct", "reset", "left", "pace", "arrow", "eta"];
const WINDOW_OPTIONS: &[OptSpec] = &[
    OptSpec {
        key: "countdown",
        kind: OptKind::Choice(&["compact", "spaced", "coarse"]),
        default: "compact",
        summary: "How to format the time left",
    },
    OptSpec {
        key: "reset_format",
        kind: OptKind::Text,
        default: "",
        summary: "Local reset time as a chrono strftime format; unset uses the default",
    },
    OptSpec {
        key: "pool",
        kind: OptKind::Choice(&["auto", "gemini", "3p", "bare"]),
        default: "auto",
        summary: "Quota pool to read when both Gemini and 3P pools exist",
    },
    OptSpec {
        key: "label",
        kind: OptKind::Choice(&["none", "auto", "short", "full", "pad"]),
        default: "none",
        summary: "Show the quota pool group label next to the icon",
    },
    OptSpec {
        key: "pace_alert",
        kind: OptKind::Flag,
        default: "true",
        summary: "Escalate color when linear burn projection reaches 90% or 100% before reset",
    },
];

const SCOPED_OPTIONS: &[OptSpec] = &[
    OptSpec {
        key: "model",
        kind: OptKind::Text,
        default: "Fable",
        summary: "Model whose weekly limit to show, matched as a name prefix; empty takes the first",
    },
    OptSpec {
        key: "countdown",
        kind: OptKind::Choice(&["compact", "spaced", "coarse"]),
        default: "compact",
        summary: "How to format the time left",
    },
    OptSpec {
        key: "reset_format",
        kind: OptKind::Text,
        default: "",
        summary: "Local reset time as a chrono strftime format; unset uses the default",
    },
];

pub(super) fn metrics() -> Vec<Metric> {
    vec![
        Metric {
            meta: MetricMeta {
                widget: WidgetKind::Gauge,
                templates: [
                    "{pct}%[ · {tokens}][ {compact_badge}]",
                    "{pct}%[ {compact_badge}]",
                    "{pct}%",
                ],
                fields: &[
                    "pct",
                    "tokens",
                    "window",
                    "left",
                    "cc_pct",
                    "compactions",
                    "compact_badge",
                ],
                options: &[
                    OptSpec {
                        key: "token_style",
                        kind: OptKind::Choice(&["short", "decimal"]),
                        default: "short",
                        summary: "Keep one decimal for millions",
                    },
                    OptSpec {
                        key: "compact_window",
                        kind: OptKind::Number,
                        default: "0",
                        summary: "Measure against this auto-compact window in tokens; 0 uses the model window",
                    },
                ],
                levels: LEVELS,
                priority: 5,
                ..meta(
                    "context",
                    Category::Context,
                    "Context",
                    "Context window use, corrected for models Claude Code can't size",
                    IconKey::Context,
                )
            },
            compute: context,
        },
        Metric {
            meta: MetricMeta {
                widget: WidgetKind::Bar,
                templates: [
                    "{pct}%[ ({left}, {reset})][ · {arrow}]",
                    "{pct}%[ · {arrow}]",
                    "{pct}%",
                ],
                fields: WINDOW_FIELDS,
                options: WINDOW_OPTIONS,
                levels: LEVELS,
                priority: 4,
                ..meta(
                    "quota.5h",
                    Category::Quota,
                    "5-hour limit",
                    "Subscription 5-hour window",
                    IconKey::Quota,
                )
            },
            compute: five_hour,
        },
        Metric {
            meta: MetricMeta {
                widget: WidgetKind::Bar,
                templates: [
                    "{pct}%[ ({left}, {reset})][ · {arrow}]",
                    "{pct}%[ · {arrow}]",
                    "{pct}%",
                ],
                fields: WINDOW_FIELDS,
                options: WINDOW_OPTIONS,
                levels: LEVELS,
                priority: 3,
                ..meta(
                    "quota.7d",
                    Category::Quota,
                    "7-day limit",
                    "Subscription weekly window",
                    IconKey::QuotaWeek,
                )
            },
            compute: seven_day,
        },
        Metric {
            meta: MetricMeta {
                widget: WidgetKind::Pace,
                templates: ["{name} {pct}%[ · {reset}]", "{name} {pct}%", "{pct}%"],
                fields: &["name", "pct", "reset", "left", "pace", "arrow", "eta"],
                options: SCOPED_OPTIONS,
                levels: LEVELS,
                ..meta(
                    "quota.model",
                    Category::Quota,
                    "Model weekly limit",
                    "A model's own weekly window, such as Fable's, from a usage snapshot file",
                    IconKey::Quota,
                )
            },
            compute: model_week,
        },
        Metric {
            meta: MetricMeta {
                widget: WidgetKind::Bar,
                templates: ["{pct}%[ · {used}/{limit}]", "{pct}%", "{pct}%"],
                fields: &[
                    "pct", "used", "limit", "period", "reset", "left", "pace", "arrow", "eta",
                ],
                min_cc: Some(gate::SPEND_LIMIT),
                options: WINDOW_OPTIONS,
                levels: LEVELS,
                ..meta(
                    "quota.spend",
                    Category::Quota,
                    "Spend limit",
                    "Claude apps gateway spend limit",
                    IconKey::Spend,
                )
            },
            compute: spend,
        },
        Metric {
            meta: MetricMeta {
                templates: ["{which} limit in {left}", "{which} {left}", "{left}"],
                fields: &["which", "at", "left"],
                ..meta(
                    "quota.eta",
                    Category::Quota,
                    "Limit run-out",
                    "When the first rate or spend limit runs out at the current pace",
                    IconKey::Eta,
                )
            },
            compute: quota_eta,
        },
        Metric {
            meta: MetricMeta {
                templates: ["{usd}", "{usd}", "{usd}"],
                fields: &["usd"],
                priority: 4,
                ..meta(
                    "cost",
                    Category::Cost,
                    "Cost",
                    "Session cost so far",
                    IconKey::Cost,
                )
            },
            compute: cost,
        },
        Metric {
            meta: MetricMeta {
                templates: [
                    "[{left}][{cold}][{sep}][{hit}% hit]",
                    "[{left}][{cold}][{sep}][{hit}%]",
                    "[{left}][{cold}]",
                ],
                fields: &[
                    "left", "cold", "rewarm", "state", "ttl", "hit", "misses", "cause", "sep",
                ],
                levels: CACHE_LEVELS,
                priority: 2,
                ..meta(
                    "cache",
                    Category::Cache,
                    "Prompt cache",
                    "Time until the prompt cache goes cold, and the share of input read from it",
                    IconKey::Cache,
                )
            },
            compute: cache,
        },
        Metric {
            meta: MetricMeta {
                templates: ["{dur}", "{dur}", "{dur}"],
                fields: &["dur"],
                priority: 2,
                ..meta(
                    "duration",
                    Category::Session,
                    "Duration",
                    "Session wall-clock time",
                    IconKey::Duration,
                )
            },
            compute: duration,
        },
    ]
}

/// Tokens in the context: the reported total, else the last request's input.
pub(super) fn context_tokens(cw: &ContextWindow) -> Option<u64> {
    cw.total_input_tokens.or_else(|| {
        let u = cw.current_usage.as_ref()?;
        Some(
            u.input_tokens.unwrap_or(0)
                + u.cache_creation_input_tokens.unwrap_or(0)
                + u.cache_read_input_tokens.unwrap_or(0),
        )
    })
}

fn context(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let style = opts
        .get("token_style")
        .and_then(|v| v.as_str())
        .unwrap_or("short");
    let cw = ctx.input.context_window.as_ref()?;
    let resolved = ctx.model();
    let compact = opts
        .get("compact_window")
        .and_then(|v| v.as_integer())
        .filter(|n| *n > 0)
        .map(|n| n as u64);
    let window = compact
        .or(resolved.and_then(|r| r.window))
        .or(cw.context_window_size)?;
    let tokens = context_tokens(cw)
        .filter(|t| *t > 0)
        .or_else(|| ctx.transcript().and_then(|t| t.context_tokens))
        .or(if cw.used_percentage.is_some() || window == 0 {
            Some(0)
        } else {
            None
        })?;
    let corrected = compact.is_some() || resolved.is_some_and(|r| r.window_corrected);
    let ratio = match cw.used_percentage {
        Some(p) if !corrected => p / 100.0,
        _ if window > 0 => (tokens as f64 / window as f64).min(1.0),
        _ => 0.0,
    };
    let mut r = Reading::default()
        .set("pct", fmt::percent(ratio))
        .set("window", fmt::tokens_style(window, style))
        .set(
            "left",
            fmt::tokens_style(window.saturating_sub(tokens), style),
        )
        .ratio(ratio);
    // Before the first response there is nothing to count; leave the field
    // unset so templates drop it instead of showing "0 tokens".
    if tokens > 0 {
        r = r.set("tokens", fmt::tokens_style(tokens, style));
    }
    if corrected && let Some(p) = cw.used_percentage {
        r = r.set("cc_pct", fmt::percent(p / 100.0));
    }
    if let Some(compactions) = ctx.transcript().map(|t| t.compactions).filter(|c| *c > 0) {
        let badge = if ctx.glyphs == GlyphTier::Ascii {
            format!("r{compactions}x")
        } else {
            format!("↻{compactions}×")
        };
        r = r.set("compactions", compactions.to_string()).set_role(
            "compact_badge",
            badge,
            Role::Chip(Severity::Warn),
        );
    }
    Some(r)
}

/// A rate-limit window: usage, time to reset, pace against elapsed time and
/// when it runs out at the current rate.
fn window(w: &Window, length: i64, limit: Limit, ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let used = w.used_percentage? / 100.0;
    let resets = w.resets_at.map(|t| t as i64);
    if resets.is_some_and(|t| t <= ctx.now) {
        // The window already reset; this reading is stale.
        return None;
    }
    let mut r = Reading::default()
        .set("pct", fmt::percent(used))
        .ratio(used);
    if let Some(at) = resets {
        let left = at - ctx.now;
        let elapsed = (1.0 - left as f64 / length as f64).clamp(0.0, 1.0);
        let delta = used - elapsed;
        let points = (delta * 100.0).round();
        let (up, down, even) = match ctx.glyphs {
            GlyphTier::Ascii => ("+", "-", ""),
            _ => ("↑", "↓", "±"),
        };
        // Within half a point of the clock it is on pace: neither way.
        let (arrow, role) = if points == 0.0 {
            (even, Role::Muted)
        } else if points > 0.0 {
            (up, Role::Bad)
        } else {
            (down, Role::Good)
        };
        r = r
            .set(
                "reset",
                fmt::clock_format(
                    at,
                    ctx.now,
                    ctx.tz,
                    opts.get("reset_format").and_then(|v| v.as_str()),
                ),
            )
            .set(
                "left",
                fmt::countdown_style(
                    left,
                    opts.get("countdown")
                        .and_then(|v| v.as_str())
                        .unwrap_or("compact"),
                ),
            )
            .set_role("pace", fmt::signed_percent(delta), role)
            .set_role("arrow", format!("{arrow}{}%", points.abs()), role);
        r.elapsed = Some(elapsed);
        let pace_alert = opts
            .get("pace_alert")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        const EPS: f64 = 1e-9;
        if pace_alert && used + EPS >= 0.10 && elapsed > 0.0 {
            let projected = used / elapsed;
            if projected > 1.0 + EPS {
                r = r.severity(Severity::Crit);
            } else if projected + EPS >= 0.90 {
                r = r.severity(Severity::Warn);
            }
        }
        if let Some(out) = projection::run_out(ctx, limit, used, at, length) {
            r = r.set("eta", fmt::clock(out, ctx.now, ctx.tz));
        }
    }
    Some(r)
}

/// Mark a window's countdown and reset time as steady: they describe the
/// clock, so they keep the item's color instead of following the level.
fn steady(mut r: Reading, fields: &[&'static str]) -> Reading {
    for name in fields {
        if let Some(text) = r.get(name).map(|f| f.text.clone()) {
            r = r.set_role(name, text, Role::Steady);
        }
    }
    r
}

fn pool_prefix(label: &str, pool: &str) -> Option<&'static str> {
    match label {
        "full" | "auto" => match pool {
            "gemini" => Some("gemini"),
            "3p" => Some("3p"),
            _ => None,
        },
        "pad" => match pool {
            "gemini" => Some("gemini"),
            "3p" => Some("3p    "),
            _ => None,
        },
        "short" => match pool {
            "gemini" => Some("G"),
            "3p" => Some("3P"),
            _ => None,
        },
        _ => None,
    }
}

fn resolved_window(
    kind: QuotaKind,
    length: i64,
    limit: Limit,
    steady_fields: &[&'static str],
    ctx: &Ctx,
    opts: &Options,
) -> Option<Reading> {
    let pool = opts.get("pool").and_then(|v| v.as_str()).unwrap_or("auto");
    let label = opts.get("label").and_then(|v| v.as_str()).unwrap_or("none");
    let resolved = ctx.input.quota_window(pool, kind, ctx.now)?;
    let w = Window {
        used_percentage: Some(resolved.used_percentage),
        resets_at: resolved.resets_at.map(|t| t as f64),
    };
    let mut r = window(&w, length, limit, ctx, opts)?;
    if let Some(prefix) = pool_prefix(label, &resolved.pool) {
        r = r.prefix(prefix);
    }
    Some(steady(r, steady_fields))
}

fn five_hour(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    resolved_window(
        QuotaKind::FiveHour,
        FIVE_HOURS,
        Limit::FiveHour,
        &["left", "reset"],
        ctx,
        opts,
    )
}

fn seven_day(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    resolved_window(
        QuotaKind::Weekly,
        SEVEN_DAYS,
        Limit::SevenDay,
        &["reset"],
        ctx,
        opts,
    )
}

/// The per-model weekly window the `model` option names, read from the
/// usage snapshot (see `state::usage_snapshot`).
fn model_week(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let want = opts
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("Fable")
        .trim()
        .to_lowercase();
    let w = ctx
        .state
        .scoped
        .iter()
        .find(|w| w.name.to_lowercase().starts_with(&want))?;
    let reading = window(
        &Window {
            used_percentage: w.used_percentage,
            resets_at: w.resets_at,
        },
        SEVEN_DAYS,
        Limit::Scoped,
        ctx,
        opts,
    )?;
    Some(reading.set("name", w.name.clone()))
}

/// A spend limit as a window, and the window's length.
fn spend_window(s: &SpendLimit) -> (Window, i64) {
    let length = match s.period.as_deref() {
        Some("daily") => 86_400,
        Some("weekly") => SEVEN_DAYS,
        _ => 30 * 86_400,
    };
    let w = Window {
        used_percentage: s.used_percentage,
        resets_at: s.resets_at,
    };
    (w, length)
}

fn spend(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let s = ctx.input.rate_limits.as_ref()?.spend_limit.as_ref()?;
    let (w, length) = spend_window(s);
    let mut r = window(&w, length, Limit::Spend, ctx, opts)?;
    if let Some(used) = s.used_usd {
        r = r.set("used", fmt::money(used));
    }
    if let Some(limit) = s.limit_usd {
        r = r.set("limit", fmt::money(limit));
    }
    if let Some(period) = &s.period {
        r = r.set("period", period);
    }
    if r.ratio.is_some_and(|x| x >= 1.0) {
        r = r.severity(Severity::Crit);
    }
    Some(r)
}

/// The limit that runs out first at the current pace, if any runs out
/// before it resets.
fn quota_eta(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let mut windows: Vec<(&'static str, Limit, f64, i64, i64)> = Vec::new();
    if let Some(rl) = ctx.input.rate_limits.as_ref() {
        let spend = rl.spend_limit.as_ref().map(spend_window);
        for (which, limit, w, length) in [
            ("5h", Limit::FiveHour, rl.five_hour.as_ref(), FIVE_HOURS),
            ("7d", Limit::SevenDay, rl.seven_day.as_ref(), SEVEN_DAYS),
            (
                "spend",
                Limit::Spend,
                spend.as_ref().map(|(w, _)| w),
                spend.as_ref().map_or(0, |(_, l)| *l),
            ),
        ] {
            if let Some(w) = w
                && let (Some(used), Some(resets)) = (w.used_percentage, w.resets_at)
            {
                windows.push((which, limit, used / 100.0, resets as i64, length));
            }
        }
    }
    if ctx.input.quota.is_some() {
        for (which, pool, kind, limit, length) in [
            (
                "G 5h",
                "gemini",
                QuotaKind::FiveHour,
                Limit::FiveHour,
                FIVE_HOURS,
            ),
            (
                "G 7d",
                "gemini",
                QuotaKind::Weekly,
                Limit::SevenDay,
                SEVEN_DAYS,
            ),
            (
                "3P 5h",
                "3p",
                QuotaKind::FiveHour,
                Limit::FiveHour,
                FIVE_HOURS,
            ),
            (
                "3P 7d",
                "3p",
                QuotaKind::Weekly,
                Limit::SevenDay,
                SEVEN_DAYS,
            ),
        ] {
            if let Some(w) = ctx.input.quota_window(pool, kind, ctx.now)
                && let Some(resets) = w.resets_at
            {
                windows.push((which, limit, w.used_percentage / 100.0, resets, length));
            }
        }
    }
    let (which, at) = windows
        .into_iter()
        .filter_map(|(which, limit, used, resets, length)| {
            Some((
                which,
                projection::run_out(ctx, limit, used, resets, length)?,
            ))
        })
        .min_by_key(|(_, at)| *at)?;
    let left = at - ctx.now;
    Some(
        Reading::default()
            .set("which", which)
            .set("at", fmt::clock(at, ctx.now, ctx.tz))
            .set("left", fmt::countdown(left))
            .severity(if left < 1_800 {
                Severity::Crit
            } else {
                Severity::Warn
            }),
    )
}

fn cost(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let usd = ctx
        .input
        .cost
        .as_ref()
        .and_then(|c| c.usd())
        .or(ctx.state.session_cost)?;
    Some(Reading::default().set("usd", fmt::money(usd)))
}

fn cache_hit_ratio_fallback(ctx: &Ctx) -> Option<f64> {
    if let Some(db) = ctx.state.db.as_ref() {
        let denom = db.last_cache_read_tokens + db.last_input_tokens;
        if denom > 0 {
            return Some(db.last_cache_read_tokens as f64 / denom as f64);
        }
    }
    if let Some(u) = ctx
        .input
        .context_window
        .as_ref()
        .and_then(|c| c.current_usage.as_ref())
    {
        let cr = u.cache_read_input_tokens.unwrap_or(0);
        let inp = u.input_tokens.unwrap_or(0);
        let denom = cr + inp;
        if denom > 0 {
            return Some(cr as f64 / denom as f64);
        }
    }
    let t = ctx.transcript()?;
    let denom = t.session_tokens.cache_read + t.session_tokens.input;
    (denom > 0).then(|| t.session_tokens.cache_read as f64 / denom as f64)
}

fn cache(ctx: &Ctx, _: &Options) -> Option<Reading> {
    if let Some(pc) = ctx.input.prompt_cache.as_ref() {
        if pc.caching_observed == Some(false) {
            return None;
        }
        let ttl = match pc.ttl.as_deref() {
            Some("1h") => 3_600,
            _ => 300,
        };
        let left = pc
            .expires_at
            .map(|t| t as i64 - ctx.now)
            .filter(|l| *l > 0 && pc.warm == Some(true));
        let mut r = match left {
            // The color follows the share of the lifetime left: see `CACHE_LEVELS`.
            Some(left) => Reading::default()
                .set("left", fmt::countdown(left))
                .set("state", "warm")
                .ratio(left as f64 / ttl as f64)
                .draining(),
            // Cold is settled, not urgent: shown quietly, whatever the levels.
            None => Reading::default()
                .set_role("cold", "cold", Role::Muted)
                .set("state", "cold")
                .ratio(0.0)
                .severity(Severity::Normal),
        };
        if let Some((usd, _)) = projection::rewarm_cost(ctx) {
            r = r.set("rewarm", fmt::money(usd));
        }
        // The first request only writes the cache, so its 0% says nothing yet.
        let hit = pc
            .hit_ratio
            .filter(|_| pc.requests.unwrap_or(0) > 1)
            .or_else(|| cache_hit_ratio_fallback(ctx));
        if let Some(hit) = hit {
            r = r.set("sep", " · ").set("hit", fmt::percent(hit));
        }
        if let Some(m) = pc.misses.filter(|m| *m > 0) {
            r = r.set("misses", m.to_string());
        }
        if let Some(cause) = pc.last_miss_cause.as_ref().and_then(|c| c.causes.first()) {
            // `tools_changed` → `tools changed`, `ttl_expired_5m` → `ttl expired 5m`.
            r = r.set("cause", cause.replace('_', " "));
        }
        return Some(r.set("ttl", pc.ttl.clone().unwrap_or_default()));
    }
    let hit = cache_hit_ratio_fallback(ctx)?;
    Some(
        Reading::default()
            .set("hit", fmt::percent(hit))
            .ratio(0.0)
            .severity(Severity::Normal),
    )
}

fn duration(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let ms = ctx
        .input
        .cost
        .as_ref()
        .and_then(|c| c.total_duration_ms)
        .or_else(|| {
            let start = ctx
                .state
                .db
                .as_ref()
                .and_then(|d| d.first_call_at)
                .or_else(|| ctx.transcript().and_then(|t| t.start))?;
            Some((ctx.now.saturating_sub(start).max(0) * 1000) as f64)
        })?;
    Some(Reading::default().set("dur", fmt::duration_ms(ms as u64)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_compact_window_replaces_the_model_window() {
        let input = crate::input::Input::parse(
            r#"{"context_window":{"total_input_tokens":120000,"context_window_size":1000000,"used_percentage":12}}"#,
        )
        .unwrap();
        let registry = crate::models::Registry::default();
        let ctx = Ctx::new(
            &input,
            &registry,
            0,
            chrono::FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        let plain = context(&ctx, &Options::new()).unwrap();
        assert_eq!(plain.get("pct").unwrap().text, "12");
        let opts = toml::from_str("compact_window = 400000").unwrap();
        let compact = context(&ctx, &opts).unwrap();
        assert_eq!(compact.get("pct").unwrap().text, "30");
        assert_eq!(compact.get("window").unwrap().text, "400k");
        assert_eq!(compact.get("left").unwrap().text, "280k");
        assert_eq!(compact.get("cc_pct").unwrap().text, "12");
    }

    #[test]
    fn context_token_style_applies_to_all_token_fields() {
        let input = crate::input::Input::parse(
            r#"{"context_window":{"total_input_tokens":1200000,"context_window_size":2200000}}"#,
        )
        .unwrap();
        let registry = crate::models::Registry::default();
        let ctx = Ctx::new(
            &input,
            &registry,
            0,
            chrono::FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        let short = context(&ctx, &Options::new()).unwrap();
        let opts = toml::from_str("token_style = 'decimal'").unwrap();
        let decimal = context(&ctx, &opts).unwrap();
        for (field, expected) in [("tokens", "1.2M"), ("window", "2.2M"), ("left", "1.0M")] {
            assert_eq!(decimal.get(field).unwrap().text, expected);
        }
        assert_eq!(short.get("left").unwrap().text, "1M");
    }

    #[test]
    fn quota_options_and_links_reach_each_window() {
        let input = crate::input::Input::parse(r#"{"rate_limits":{"five_hour":{"used_percentage":30,"resets_at":7620},"seven_day":{"used_percentage":30,"resets_at":7620},"spend_limit":{"used_percentage":30,"resets_at":7620}}}"#).unwrap();
        let registry = crate::models::Registry::default();
        let ctx = Ctx::new(
            &input,
            &registry,
            0,
            chrono::FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        for id in ["quota.5h", "quota.7d", "quota.spend"] {
            let metric = metrics().into_iter().find(|m| m.meta.id == id).unwrap();
            assert!(metric.meta.options.iter().any(|o| o.key == "countdown"));
            assert!(metric.meta.options.iter().any(|o| o.key == "reset_format"));
            let default = (metric.compute)(&ctx, &Options::new()).unwrap();
            assert_eq!(default.get("left").unwrap().text, "2h07m");
            for (style, expected) in [("spaced", "2h 7m"), ("coarse", "2h")] {
                let opts =
                    toml::from_str(&format!("countdown = '{style}'\nreset_format = '%H:%M:%S'"))
                        .unwrap();
                let reading = (metric.compute)(&ctx, &opts).unwrap();
                assert_eq!(reading.get("left").unwrap().text, expected);
                assert_eq!(reading.get("reset").unwrap().text, "02:07:00");
                assert_eq!(reading.link, None);
            }
        }
        let ctx = ctx.with_state(crate::state::StateView {
            scoped: vec![crate::state::usage_snapshot::ScopedWindow {
                name: "Fable 5.2".into(),
                used_percentage: Some(30.0),
                resets_at: Some(7620.0),
            }],
            ..crate::state::StateView::default()
        });
        let reading = model_week(&ctx, &Options::new()).unwrap();
        assert_eq!(reading.get("name").unwrap().text, "Fable 5.2");
        assert_eq!(reading.get("pct").unwrap().text, "30");
        assert_eq!(reading.get("left").unwrap().text, "2h07m");
        let opts = toml::from_str("model = 'opus'").unwrap();
        assert!(model_week(&ctx, &opts).is_none());
        assert!(
            metrics()
                .into_iter()
                .find(|m| m.meta.id == "context")
                .unwrap()
                .meta
                .options
                .iter()
                .any(|o| o.key == "token_style")
        );
    }

    #[test]
    fn on_pace_is_neither_ahead_nor_behind() {
        // 30% used with 30% of the five hours gone.
        let now = 1_000_000;
        let resets = now + FIVE_HOURS * 7 / 10;
        let json = format!(
            r#"{{"rate_limits": {{"five_hour": {{"used_percentage": 30, "resets_at": {resets}}}}}}}"#
        );
        let input = crate::input::Input::parse(&json).unwrap();
        let registry = crate::models::Registry::default();
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        for (tier, text) in [(GlyphTier::Nerd, "±0%"), (GlyphTier::Ascii, "0%")] {
            let ctx = Ctx::new(&input, &registry, now, tz, tier);
            let reading = five_hour(&ctx, &Options::new()).unwrap();
            let arrow = reading.get("arrow").unwrap();
            assert_eq!((arrow.text.as_str(), arrow.role), (text, Some(Role::Muted)));
        }
    }

    #[test]
    fn context_renders_gauge_and_compaction_badge() {
        let meta = metrics()
            .into_iter()
            .find(|m| m.meta.id == "context")
            .unwrap()
            .meta;
        assert_eq!(meta.widget, WidgetKind::Gauge);

        let registry = crate::models::Registry::default();
        let tz = chrono::FixedOffset::east_opt(0).unwrap();

        let input_zero =
            crate::input::Input::parse(r#"{"context_window":{"context_window_size":0}}"#).unwrap();
        let ctx_zero = Ctx::new(&input_zero, &registry, 0, tz, GlyphTier::Unicode);
        let r_zero = context(&ctx_zero, &Options::new()).unwrap();
        assert_eq!(r_zero.get("pct").unwrap().text, "0");
        assert!(r_zero.get("compact_badge").is_none());

        let input = crate::input::Input::parse(
            r#"{"context_window":{"total_input_tokens":50000,"context_window_size":200000}}"#,
        )
        .unwrap();
        for (tier, expected) in [(GlyphTier::Nerd, "↻2×"), (GlyphTier::Ascii, "r2x")] {
            let summary = crate::state::transcript::Summary {
                compactions: 2,
                ..crate::state::transcript::Summary::default()
            };
            let ctx = Ctx::new(&input, &registry, 0, tz, tier)
                .with_transcript(crate::metrics::TranscriptSource::Fixed(Some(summary)));
            let r = context(&ctx, &Options::new()).unwrap();
            let badge = r.get("compact_badge").unwrap();
            assert_eq!(badge.text, expected);
            assert_eq!(badge.role, Some(Role::Chip(Severity::Warn)));
            assert_eq!(r.get("compactions").unwrap().text, "2");
        }
    }

    #[test]
    fn quota_pools_labels_and_pace_alerts() {
        let m5 = metrics()
            .into_iter()
            .find(|m| m.meta.id == "quota.5h")
            .unwrap()
            .meta;
        let m7 = metrics()
            .into_iter()
            .find(|m| m.meta.id == "quota.7d")
            .unwrap()
            .meta;
        assert_eq!(m5.icon, IconKey::Quota);
        assert_eq!(m7.icon, IconKey::QuotaWeek);
        assert_ne!(
            crate::style::glyphs::icon(m5.icon, GlyphTier::Unicode),
            crate::style::glyphs::icon(m7.icon, GlyphTier::Unicode)
        );

        let now = 1_000_000;
        let reset_g5 = now + FIVE_HOURS / 2; // 50% elapsed
        let reset_g7 = now + SEVEN_DAYS / 2; // 50% elapsed
        let input = crate::input::Input::parse(&format!(
            r#"{{
              "quota": {{
                "gemini-5h": {{"remaining_fraction": 0.45, "reset_in_seconds": {}}},
                "gemini-weekly": {{"remaining_fraction": 0.70, "reset_in_seconds": {}}},
                "3p-5h": {{"remaining_fraction": 0.50, "reset_in_seconds": 0}},
                "3p-weekly": {{"remaining_fraction": 0.61, "reset_in_seconds": 0}}
              }}
            }}"#,
            reset_g5 - now,
            reset_g7 - now
        ))
        .unwrap();
        let registry = crate::models::Registry::default();
        let tz = chrono::FixedOffset::east_opt(0).unwrap();
        let ctx = Ctx::new(&input, &registry, now, tz, GlyphTier::Nerd);

        let g5_opts: Options = toml::from_str("pool = 'gemini'\nlabel = 'full'").unwrap();
        let r_g5 = five_hour(&ctx, &g5_opts).unwrap();
        assert_eq!(r_g5.prefix.as_deref(), Some("gemini"));
        assert_eq!(r_g5.get("pct").unwrap().text, "55");
        assert_eq!(r_g5.severity, Some(Severity::Crit)); // 0.55 / 0.50 = 1.10 > 1.0
        let arrow = r_g5.get("arrow").unwrap();
        assert_eq!((arrow.text.as_str(), arrow.role), ("↑5%", Some(Role::Bad)));

        let g7_opts: Options = toml::from_str("pool = 'gemini'\nlabel = 'short'").unwrap();
        let r_g7 = seven_day(&ctx, &g7_opts).unwrap();
        assert_eq!(r_g7.prefix.as_deref(), Some("G"));
        assert_eq!(r_g7.get("pct").unwrap().text, "30");
        assert_eq!(
            (
                r_g7.get("arrow").unwrap().text.as_str(),
                r_g7.get("arrow").unwrap().role
            ),
            ("↓20%", Some(Role::Good))
        );

        let p3_opts: Options = toml::from_str("pool = '3p'\nlabel = 'short'").unwrap();
        let r_p3_7d = seven_day(&ctx, &p3_opts).unwrap();
        assert_eq!(r_p3_7d.prefix.as_deref(), Some("3P"));
        assert_eq!(r_p3_7d.get("pct").unwrap().text, "39");
        assert!(r_p3_7d.get("reset").is_none());

        // Exact 0.55 remaining (used = 0.44999999999999996) at 50% elapsed (projected = 0.90 - eps) still triggers Warn.
        let warn_input = crate::input::Input::parse(&format!(
            r#"{{"quota": {{"gemini-5h": {{"remaining_fraction": 0.55, "reset_in_seconds": {}}}}}}}"#,
            reset_g5 - now
        ))
        .unwrap();
        let warn_ctx = Ctx::new(&warn_input, &registry, now, tz, GlyphTier::Nerd);
        let r_warn = five_hour(&warn_ctx, &g5_opts).unwrap();
        assert_eq!(r_warn.severity, Some(Severity::Warn));
    }
}
