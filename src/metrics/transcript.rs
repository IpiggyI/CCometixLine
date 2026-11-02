//! Session and activity metrics from the bounded transcript summary.

use super::{Ctx, Metric, MetricMeta, OptKind, OptSpec, Options, Reading, fmt};
use crate::style::{Category, GlyphTier, IconKey};
use crate::widgets::WidgetKind;
use std::collections::BTreeMap;

fn meta(
    id: &'static str,
    category: Category,
    name: &'static str,
    summary: &'static str,
    icon: IconKey,
    templates: [&'static str; 3],
    fields: &'static [&'static str],
) -> MetricMeta {
    MetricMeta {
        id,
        category,
        name,
        summary,
        icon,
        templates,
        fields,
        widget: WidgetKind::Text,
        levels: &[],
        min_cc: None,
        needs_mod: false,
        options: &[],
        stateful: true,
        priority: 2,
    }
}

pub(super) fn metrics() -> Vec<Metric> {
    vec![
        Metric {
            meta: meta(
                "advisor",
                Category::Identity,
                "Advisor",
                "Advisor model reported in the session transcript",
                IconKey::Advisor,
                ["{name}", "{name}", "{name}"],
                &["id", "name"],
            ),
            compute: advisor,
        },
        Metric {
            meta: meta(
                "session.start",
                Category::Session,
                "Session start",
                "First timestamp in the session transcript",
                IconKey::SessionStart,
                ["{at}[ · {ago} ago]", "{at}", "{ago}"],
                &["at", "ago"],
            ),
            compute: |ctx, _| time(ctx, ctx.transcript()?.start),
        },
        Metric {
            meta: meta(
                "session.last_reply",
                Category::Session,
                "Last reply",
                "Newest assistant reply in the session transcript",
                IconKey::LastReply,
                ["{at}[ · {ago} ago]", "{at}", "{ago}"],
                &["at", "ago"],
            ),
            compute: |ctx, _| time(ctx, ctx.transcript()?.last_reply),
        },
        Metric {
            meta: meta(
                "activity.skills",
                Category::Activity,
                "Skills",
                "Skill calls in the session transcript",
                IconKey::Skills,
                [
                    "{count} calls[ · {running}][ · {done}]",
                    "{count}[ · {running}]",
                    "{count}",
                ],
                &["running", "done", "count"],
            ),
            compute: skills,
        },
        Metric {
            meta: MetricMeta {
                options: &[OptSpec {
                    key: "top",
                    kind: OptKind::Number,
                    default: "4",
                    summary: "Number of most-used tools to show",
                }],
                ..meta(
                    "activity.tool_calls",
                    Category::Activity,
                    "Tool calls",
                    "Tool calls in the session transcript",
                    IconKey::Tools,
                    ["{counts}", "{counts}", "{count}"],
                    &["counts", "count", "last"],
                )
            },
            compute: tool_calls,
        },
        Metric {
            meta: meta(
                "activity.hooks",
                Category::Activity,
                "Hooks",
                "Hook results since the latest user prompt in the session transcript",
                IconKey::Hooks,
                ["{last}[ · {counts}]", "{last}", "{last}"],
                &["last", "counts", "failed"],
            ),
            compute: hooks,
        },
    ]
}

fn advisor(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let id = ctx.transcript()?.advisor.as_deref()?;
    let resolved = ctx.registry.resolve(id, None, None);
    Some(Reading::default().set("id", id).set("name", resolved.name))
}

fn time(ctx: &Ctx, at: Option<i64>) -> Option<Reading> {
    let at = at?;
    let secs = ctx.now.saturating_sub(at).max(0) as u64;
    Some(
        Reading::default()
            .set("at", fmt::clock(at, ctx.now, ctx.tz))
            .set(
                "ago",
                if secs == 0 {
                    "0s".into()
                } else {
                    fmt::duration_ms(secs.saturating_mul(1000))
                },
            ),
    )
}

fn skills(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let skills = &ctx.transcript()?.skills;
    (skills.count > 0).then(|| {
        Reading::default()
            .set(
                "running",
                skills
                    .running
                    .iter()
                    .take(3)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", "),
            )
            .set(
                "done",
                skills.done[skills.done.len().saturating_sub(4)..].join(", "),
            )
            .set("count", skills.count.to_string())
    })
}

fn tool_calls(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let calls = &ctx.transcript()?.tool_calls;
    if calls.count == 0 {
        return None;
    }
    let top = opts
        .get("top")
        .and_then(|v| v.as_integer())
        .unwrap_or(4)
        .max(1) as usize;
    let ascii = ctx.glyphs == GlyphTier::Ascii;
    let last = calls
        .last
        .as_ref()
        .map(|(name, failed)| format!("{name} {}", mark(ascii, *failed)))
        .unwrap_or_default();
    Some(
        Reading::default()
            .set("counts", counts(&calls.counts, top, ascii))
            .set("count", calls.count.to_string())
            .set("last", last),
    )
}

fn mark(ascii: bool, failed: bool) -> &'static str {
    match (ascii, failed) {
        (true, false) => "ok",
        (true, true) => "x",
        (false, false) => "✓",
        (false, true) => "✗",
    }
}

fn counts(values: &BTreeMap<String, usize>, top: usize, ascii: bool) -> String {
    let mut counts: Vec<_> = values.iter().collect();
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    let times = if ascii { "x" } else { "×" };
    counts
        .into_iter()
        .take(top)
        .map(|(event, n)| format!("{event}{times}{n}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn hooks(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let hooks = &ctx.transcript()?.hooks;
    let name = hooks.last.as_ref()?;
    let ascii = ctx.glyphs == GlyphTier::Ascii;
    Some(
        Reading::default()
            .set("last", format!("{name} {}", mark(ascii, hooks.last_failed)))
            .set("counts", counts(&hooks.counts, usize::MAX, ascii))
            .set(
                "failed",
                if hooks.failed == 0 {
                    String::new()
                } else {
                    hooks.failed.to_string()
                },
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::metrics::{McpSource, TranscriptSource};
    use crate::models::Registry;
    use crate::state::transcript::{Hooks, Skills, Summary, ToolCalls};
    use chrono::FixedOffset;

    fn reading(ctx: &Ctx, id: &str) -> Option<Reading> {
        super::super::read(super::super::find(id).unwrap(), ctx, &Options::new())
    }

    fn text(reading: &Reading, field: &str) -> String {
        reading.get(field).unwrap().text.clone()
    }

    fn summary() -> Summary {
        Summary {
            start: Some(480),
            last_reply: Some(11_940),
            advisor: Some("claude-opus-5-5".into()),
            skills: Skills {
                running: ["a", "b", "c", "d"].map(str::to_owned).to_vec(),
                done: ["e", "f", "g", "h", "i"].map(str::to_owned).to_vec(),
                count: 9,
            },
            hooks: Hooks {
                last: Some("Stop".into()),
                last_failed: true,
                counts: [("Stop".into(), 1), ("PreToolUse".into(), 5)].into(),
                failed: 1,
            },
            tool_calls: Box::new(ToolCalls {
                counts: [
                    ("Bash".into(), 47),
                    ("Edit".into(), 11),
                    ("Grep".into(), 5),
                    ("Read".into(), 5),
                    ("Skill".into(), 2),
                ]
                .into(),
                count: 70,
                last: Some(("Edit".into(), false)),
            }),
        }
    }

    #[test]
    fn fixed_sources_render_all_fields_without_reading_live_files() {
        let input =
            Input::parse(r#"{"transcript_path":"/missing", "cost":{"total_duration_ms":0}}"#)
                .unwrap();
        let registry = Registry::default();
        let ctx = Ctx::new(
            &input,
            &registry,
            12_000,
            FixedOffset::east_opt(3600).unwrap(),
            GlyphTier::Unicode,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary())))
        .with_mcp(McpSource::Fixed(Some(vec![
            "z".into(),
            "a".into(),
            "a".into(),
        ])));
        let advisor = reading(&ctx, "advisor").unwrap();
        assert_eq!(text(&advisor, "name"), "Opus 5.5");
        assert_eq!(text(&advisor, "id"), "claude-opus-5-5");
        let start = reading(&ctx, "session.start").unwrap();
        assert_eq!(text(&start, "at"), "01:08");
        assert_eq!(text(&start, "ago"), "3h12m");
        assert_eq!(
            text(&reading(&ctx, "session.last_reply").unwrap(), "ago"),
            "1m"
        );
        let skills = reading(&ctx, "activity.skills").unwrap();
        assert_eq!(text(&skills, "running"), "a, b, c");
        assert_eq!(text(&skills, "done"), "f, g, h, i");
        assert_eq!(text(&skills, "count"), "9");
        let hooks = reading(&ctx, "activity.hooks").unwrap();
        assert_eq!(text(&hooks, "last"), "Stop ✗");
        assert_eq!(text(&hooks, "counts"), "PreToolUse×5 Stop×1");
        assert_eq!(text(&hooks, "failed"), "1");
        let calls = reading(&ctx, "activity.tool_calls").unwrap();
        assert_eq!(text(&calls, "counts"), "Bash×47 Edit×11 Grep×5 Read×5");
        assert_eq!(text(&calls, "count"), "70");
        assert_eq!(text(&calls, "last"), "Edit ✓");
        let mcp = reading(&ctx, "mcp").unwrap();
        assert_eq!(text(&mcp, "names"), "a, z");
        assert_eq!(text(&mcp, "count"), "2");
    }

    #[test]
    fn ascii_hook_marks_and_optional_failure_count() {
        let input = Input::default();
        let registry = Registry::default();
        for failed in [false, true] {
            let mut summary = summary();
            summary.hooks.last_failed = failed;
            summary.hooks.failed = usize::from(failed);
            let ctx = Ctx::new(
                &input,
                &registry,
                12_000,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Ascii,
            )
            .with_transcript(TranscriptSource::Fixed(Some(summary)));
            let hooks = reading(&ctx, "activity.hooks").unwrap();
            assert_eq!(
                text(&hooks, "last"),
                if failed { "Stop x" } else { "Stop ok" }
            );
            assert_eq!(text(&hooks, "counts"), "PreToolUsex5 Stopx1");
            assert_eq!(hooks.get("failed").is_some(), failed);
        }
    }

    #[test]
    fn tool_call_top_is_clamped_and_ascii_marks_follow_the_glyph_tier() {
        let input = Input::default();
        let registry = Registry::default();
        let mut summary = summary();
        summary.tool_calls.last = Some(("Grep".into(), true));
        let ctx = Ctx::new(
            &input,
            &registry,
            0,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Ascii,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary)));
        for (top, expected) in [(0, "Bashx47"), (-2, "Bashx47"), (2, "Bashx47 Editx11")] {
            let opts = [("top".into(), toml::Value::Integer(top))]
                .into_iter()
                .collect();
            let reading = tool_calls(&ctx, &opts).unwrap();
            assert_eq!(text(&reading, "counts"), expected);
            assert_eq!(text(&reading, "last"), "Grep x");
        }
    }

    #[test]
    fn absent_or_empty_sources_hide_metrics() {
        let input = Input::default();
        let registry = Registry::default();
        for summary in [None, Some(Summary::default())] {
            let ctx = Ctx::new(
                &input,
                &registry,
                0,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            )
            .with_transcript(TranscriptSource::Fixed(summary))
            .with_mcp(McpSource::Fixed(Some(vec![])));
            for id in [
                "advisor",
                "session.start",
                "session.last_reply",
                "activity.skills",
                "activity.tool_calls",
                "activity.hooks",
                "mcp",
            ] {
                assert!(reading(&ctx, id).is_none(), "{id}");
            }
        }
    }
}
