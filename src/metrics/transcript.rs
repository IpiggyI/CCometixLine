//! Session and activity metrics from the bounded transcript summary.

use super::{Ctx, Metric, MetricMeta, Options, Reading, fmt};
use crate::style::{Category, GlyphTier, IconKey};
use crate::widgets::WidgetKind;

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
                "Newest assistant reply in the transcript tail",
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
                "Skill calls in the transcript tail",
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
            meta: meta(
                "activity.hooks",
                Category::Activity,
                "Hooks",
                "Hook results since the latest user prompt in the transcript tail",
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

fn hooks(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let hooks = &ctx.transcript()?.hooks;
    let name = hooks.last.as_ref()?;
    let ascii = ctx.glyphs == GlyphTier::Ascii;
    let mark = match (ascii, hooks.last_failed) {
        (true, false) => "ok",
        (true, true) => "x",
        (false, false) => "✓",
        (false, true) => "✗",
    };
    let mut counts: Vec<_> = hooks.counts.iter().collect();
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    let times = if ascii { "x" } else { "×" };
    let counts = counts
        .iter()
        .map(|(event, n)| format!("{event}{times}{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    Some(
        Reading::default()
            .set("last", format!("{name} {mark}"))
            .set("counts", counts)
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
    use crate::state::transcript::{Hooks, Skills, Summary};
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
                "activity.hooks",
                "mcp",
            ] {
                assert!(reading(&ctx, id).is_none(), "{id}");
            }
        }
    }
}
