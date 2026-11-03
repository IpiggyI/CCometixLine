//! Session and activity metrics from the bounded transcript summary.

use super::{Ctx, Metric, MetricMeta, OptKind, OptSpec, Options, Reading, fmt};
use crate::state::transcript::{Agent, Status, clean, short_event};
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
                ["{now}", "{now}", "{count}"],
                &["running", "done", "count", "now", "total", "arrow"],
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
                    "activity.tools",
                    Category::Activity,
                    "Tools",
                    "Running tools and counts of the latest completed calls",
                    IconKey::Tools,
                    ["{line}", "{now}", "{total}"],
                    &[
                        "tools", "count", "elapsed", "now", "line", "counts", "total", "arrow",
                    ],
                )
            },
            compute: tools,
        },
        Metric {
            meta: meta(
                "activity.agents",
                Category::Activity,
                "Agents",
                "Running and just-finished subagents",
                IconKey::Agent,
                ["{now}", "{now}", "{count}"],
                &["count", "types", "now", "counts", "arrow"],
            ),
            compute: agents,
        },
        Metric {
            meta: meta(
                "activity.todos",
                Category::Activity,
                "Todos",
                "Current todo and completion progress in this session",
                IconKey::Todos,
                ["{current} ({progress})", "{current}", "{progress}"],
                &["current", "progress"],
            ),
            compute: todos,
        },
        Metric {
            meta: meta(
                "activity.hooks",
                Category::Activity,
                "Hooks",
                "Hook results since the latest user prompt in the session transcript",
                IconKey::Hooks,
                ["{last} {arrow} {counts}", "{last}", "{failed}"],
                &["last", "counts", "failed", "arrow"],
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
        let now = skills
            .running
            .first()
            .map(|s| format!("{} {}", ctx.glyphs.running(), s))
            .or_else(|| {
                let mut names: Vec<&str> = Vec::new();
                for name in &skills.done {
                    if !names.contains(&name.as_str()) {
                        names.push(name);
                    }
                }
                (!names.is_empty())
                    .then(|| format!("{} {}", ctx.glyphs.finished(false), names.join(", ")))
            })
            .unwrap_or_default();
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
            .set("now", now)
            .set(
                "total",
                format!(
                    "{} {}",
                    skills.count,
                    if skills.count == 1 { "skill" } else { "skills" }
                ),
            )
            .set("arrow", ctx.glyphs.arrow())
    })
}

fn tool_name(name: &str) -> &str {
    if name.starts_with("mcp__") {
        name.splitn(3, "__").nth(2).unwrap_or(name)
    } else {
        name
    }
}

/// Finished calls counted over this many newest calls, as claude-hud does.
const TOOLS_KEPT: usize = 20;

/// claude-hud `shortenPath`: at most 20 characters, keeping the file name.
fn short_target(target: &str) -> String {
    let target = target.replace('\\', "/");
    if target.chars().count() <= 20 {
        return target;
    }
    let file = target
        .rsplit('/')
        .next()
        .filter(|f| !f.is_empty())
        .unwrap_or(&target);
    if file.chars().count() >= 20 {
        truncate(file, 20)
    } else {
        format!(".../{file}")
    }
}

/// `✓ Edit ×6 | ✓ Read ×3 | +2 more`, most used first.
fn finished_counts(values: &BTreeMap<String, usize>, top: usize, ctx: &Ctx) -> String {
    let mut counts: Vec<_> = values.iter().collect();
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    let times = if ctx.glyphs == GlyphTier::Ascii {
        "x"
    } else {
        "×"
    };
    let remaining = counts.len().saturating_sub(top);
    let mut shown: Vec<_> = counts
        .into_iter()
        .take(top)
        .map(|(name, n)| format!("{} {name} {times}{n}", ctx.glyphs.finished(false)))
        .collect();
    if remaining > 0 {
        shown.push(format!("+{remaining} more"));
    }
    shown.join(" | ")
}

fn tools(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let Some(summary) = ctx.transcript() else {
        return bridge_tools(ctx, opts);
    };
    let calls = &summary.tool_calls;
    if calls.count == 0 {
        return None;
    }
    let top = opts
        .get("top")
        .and_then(|v| v.as_integer())
        .unwrap_or(4)
        .clamp(1, 128) as usize;
    let running: Vec<_> = calls
        .recent
        .iter()
        .filter(|t| t.status == Status::Running)
        .collect();
    let finished: Vec<_> = calls
        .recent
        .iter()
        .filter(|t| t.status != Status::Running)
        .collect();
    let now = running
        .iter()
        .skip(running.len().saturating_sub(2))
        .map(|t| {
            format!(
                "{} {}{}",
                ctx.glyphs.running(),
                tool_name(&t.name),
                if t.target.is_empty() {
                    String::new()
                } else {
                    format!(": {}", short_target(&t.target))
                }
            )
        })
        .collect::<Vec<_>>()
        .join(" | ");
    let mut named = BTreeMap::new();
    for t in finished
        .iter()
        .skip(finished.len().saturating_sub(TOOLS_KEPT))
    {
        *named.entry(tool_name(&t.name).to_owned()).or_insert(0usize) += 1;
    }
    let done = finished_counts(&named, top, ctx);
    let line = [now.as_str(), done.as_str()]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" | ");
    let mut session = BTreeMap::new();
    for (name, count) in &calls.counts {
        *session.entry(tool_name(name).to_owned()).or_default() += count;
    }
    let mut r = Reading::default()
        .set("now", now)
        .set("line", line)
        .set(
            "counts",
            counts(&session, top, ctx.glyphs == GlyphTier::Ascii),
        )
        .set("total", calls.count.to_string())
        .set("count", running.len().to_string())
        .set(
            "tools",
            running
                .iter()
                .map(|t| tool_name(&t.name))
                .collect::<Vec<_>>()
                .join(" · "),
        )
        .set("arrow", ctx.glyphs.arrow());
    if let Some(start) = running.iter().map(|t| t.start).min() {
        r = r.set(
            "elapsed",
            fmt::duration_ms((ctx.now.saturating_sub(start).max(0) as u64).saturating_mul(1000)),
        );
    }
    Some(r)
}

fn bridge_tools(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let legacy = super::live::tools(ctx, opts)?;
    let mut tools: Vec<_> = ctx.state.bridge.as_ref()?.tools.iter().take(256).collect();
    tools.sort_by_key(|t| t.since);
    let mut parts = tools
        .iter()
        .take(2)
        .map(|t| {
            format!(
                "{} {}",
                ctx.glyphs.running(),
                clean(tool_name(&t.name), 128)
            )
        })
        .collect::<Vec<_>>();
    if tools.len() > 2 {
        parts.push(format!("+{} more", tools.len() - 2));
    }
    let total = legacy.get("count")?.text.clone();
    let now = parts.join(" | ");
    Some(
        legacy
            .set("line", now.clone())
            .set("now", now)
            .set("total", total)
            .set("arrow", ctx.glyphs.arrow()),
    )
}

fn short_model(model: &str) -> String {
    let model = clean(model, 128);
    let id = model.split('[').next().unwrap_or(&model).to_lowercase();
    let Some(id) = id.strip_prefix("claude-") else {
        return model;
    };
    let mut parts: Vec<_> = id.split('-').collect();
    if parts
        .last()
        .is_some_and(|p| p.len() == 8 && p.chars().all(|c| c.is_ascii_digit()))
    {
        parts.pop();
    }
    let family = |p: &str| matches!(p, "opus" | "sonnet" | "haiku");
    let (name, version) = if parts.first().is_some_and(|p| family(p)) {
        (parts[0], &parts[1..])
    } else if parts.last().is_some_and(|p| family(p)) {
        (parts[parts.len() - 1], &parts[..parts.len() - 1])
    } else {
        return model;
    };
    if version.is_empty()
        || !version
            .iter()
            .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
    {
        return model;
    }
    format!("{name}-{}", version.join("."))
}

fn agent_now(ctx: &Ctx, agent: &Agent) -> String {
    let badge = [
        agent.model.as_deref().map(short_model),
        agent.effort.clone(),
    ]
    .into_iter()
    .flatten()
    .filter(|s| !s.is_empty())
    .collect::<Vec<_>>()
    .join(" ");
    let elapsed = agent
        .end
        .unwrap_or(ctx.now)
        .saturating_sub(agent.start)
        .max(0) as u64;
    let elapsed = if elapsed < 60 {
        format!("{elapsed}s")
    } else if elapsed < 3600 {
        format!("{}m {}s", elapsed / 60, elapsed % 60)
    } else {
        format!("{}h {}m", elapsed / 3600, elapsed / 60 % 60)
    };
    format!(
        "{} {}{}{} ({elapsed})",
        if agent.end.is_none() {
            ctx.glyphs.running()
        } else {
            ctx.glyphs.finished(false)
        },
        clean(agent.kind.rsplit(':').next().unwrap_or(&agent.kind), 24),
        if badge.is_empty() {
            String::new()
        } else {
            format!(" [{badge}]")
        },
        if agent.description.is_empty() {
            String::new()
        } else {
            format!(
                " {} {}",
                if ctx.glyphs == GlyphTier::Ascii {
                    "."
                } else {
                    "·"
                },
                truncate(&agent.description, 24)
            )
        }
    )
}

fn agents(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let Some(summary) = ctx.transcript() else {
        let legacy = super::live::agents(ctx, opts)?;
        let active: Vec<_> = ctx
            .state
            .bridge
            .as_ref()?
            .agents
            .iter()
            .filter(|a| a.is_active())
            .take(128)
            .collect();
        let now = active
            .iter()
            .map(|a| format!("{} {}", ctx.glyphs.running(), clean(&a.kind, 64)))
            .collect::<Vec<_>>()
            .join(" | ");
        return Some(
            legacy
                .set("now", now)
                .set(
                    "counts",
                    agent_counts(active.len(), active.len(), ctx.glyphs),
                )
                .set("arrow", ctx.glyphs.arrow()),
        );
    };
    if summary.agent_count == 0 {
        return None;
    }
    let running: Vec<_> = summary.agents.iter().filter(|a| a.end.is_none()).collect();
    let mut recent: Vec<_> = summary
        .agents
        .iter()
        .filter(|a| {
            a.end
                .is_some_and(|end| end <= ctx.now && ctx.now.saturating_sub(end) <= 60)
        })
        .collect();
    recent.sort_by_key(|a| a.end);
    let shown = running.len().min(2);
    let slots = 2 - shown;
    let mut now: Vec<_> = running
        .iter()
        .skip(running.len().saturating_sub(2))
        .chain(recent.iter().skip(recent.len().saturating_sub(slots)))
        .map(|a| agent_now(ctx, a))
        .collect::<Vec<_>>();
    if running.len() > 2 {
        now.push(format!("+{} more", running.len() - 2));
    }
    // Nothing running or just finished: hide the row instead of falling back
    // to a bare count.
    if now.is_empty() {
        return None;
    }
    Some(
        Reading::default()
            .set("now", now.join(" | "))
            .set("count", running.len().to_string())
            .set(
                "types",
                running
                    .iter()
                    .map(|a| a.kind.as_str())
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(" · "),
            )
            .set(
                "counts",
                agent_counts(summary.agent_count, running.len(), ctx.glyphs),
            )
            .set("arrow", ctx.glyphs.arrow()),
    )
}

fn agent_counts(total: usize, running: usize, glyphs: GlyphTier) -> String {
    format!(
        "Agent{}{total}{}",
        if glyphs == GlyphTier::Ascii {
            "x"
        } else {
            "×"
        },
        if running > 0 {
            format!(" ({running} running)")
        } else {
            String::new()
        }
    )
}

fn truncate(text: &str, max: usize) -> String {
    let text = clean(text, 256);
    if text.chars().count() > max {
        format!("{}...", text.chars().take(max - 3).collect::<String>())
    } else {
        text
    }
}

fn todos(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let todos = &ctx.transcript()?.todos;
    if todos.is_empty() {
        return None;
    }
    let done = todos.iter().filter(|t| t.status == "completed").count();
    let current = if let Some(todo) = todos.iter().find(|t| t.status == "in_progress") {
        format!("{} {}", ctx.glyphs.current(), truncate(&todo.content, 50))
    } else if done == todos.len() {
        format!("{} all done", ctx.glyphs.finished(false))
    } else {
        return None;
    };
    Some(
        Reading::default()
            .set("current", current)
            .set("progress", format!("{done}/{}", todos.len())),
    )
}

fn counts(values: &BTreeMap<String, usize>, top: usize, ascii: bool) -> String {
    let mut counts: Vec<_> = values.iter().collect();
    counts.sort_by(|(a, x), (b, y)| y.cmp(x).then_with(|| a.cmp(b)));
    let times = if ascii { "x" } else { "×" };
    let remaining = counts.len().saturating_sub(top);
    let mut shown: Vec<_> = counts
        .into_iter()
        .take(top)
        .map(|(event, n)| format!("{event}{times}{n}"))
        .collect();
    if remaining > 0 {
        shown.push(format!("+{remaining} more"));
    }
    shown.join(" ")
}

fn hooks(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let hooks = &ctx.transcript()?.hooks;
    let name = hooks.last.as_ref()?;
    let ascii = ctx.glyphs == GlyphTier::Ascii;
    let mut events = BTreeMap::new();
    for (event, n) in &hooks.counts {
        *events.entry(short_event(event)).or_default() += n;
    }
    Some(
        Reading::default()
            .set(
                "last",
                format!("{} {name}", ctx.glyphs.finished(hooks.last_failed)),
            )
            .set("counts", counts(&events, usize::MAX, ascii))
            .set("arrow", ctx.glyphs.arrow())
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
    use crate::state::transcript::{Hooks, Skills, Summary, Tool, ToolCalls};
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
            skills: Box::new(Skills {
                running: ["a", "b", "c", "d"].map(str::to_owned).to_vec(),
                done: ["e", "f", "g", "h", "i"].map(str::to_owned).to_vec(),
                count: 9,
            }),
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
                    ("Write".into(), 2),
                ]
                .into(),
                count: 70,
                last: Some(("Edit".into(), false)),
                recent: vec![Tool {
                    id: "edit".into(),
                    name: "Edit".into(),
                    target: "lib.rs".into(),
                    status: Status::Error,
                    ..Tool::default()
                }],
            }),
            ..Summary::default()
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
        assert_eq!(text(&hooks, "last"), "✗ Stop");
        assert_eq!(text(&hooks, "counts"), "Pre×5 Stop×1");
        assert_eq!(text(&hooks, "failed"), "1");
        let calls = reading(&ctx, "activity.tools").unwrap();
        assert_eq!(
            text(&calls, "counts"),
            "Bash×47 Edit×11 Grep×5 Read×5 +1 more"
        );
        assert_eq!(text(&calls, "total"), "70");
        assert!(calls.get("now").is_none());
        assert_eq!(text(&calls, "line"), "✓ Edit ×1");
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
                if failed { "x Stop" } else { "+ Stop" }
            );
            assert_eq!(text(&hooks, "counts"), "Prex5 Stopx1");
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
        for (top, expected) in [
            (0, "Bashx47 +4 more"),
            (-2, "Bashx47 +4 more"),
            (2, "Bashx47 Editx11 +3 more"),
        ] {
            let opts = [("top".into(), toml::Value::Integer(top))]
                .into_iter()
                .collect();
            let reading = tools(&ctx, &opts).unwrap();
            assert_eq!(text(&reading, "counts"), expected);
            assert_eq!(text(&reading, "line"), "+ Edit x1");
        }
    }

    fn rendered(ctx: &Ctx, id: &str) -> String {
        let metric = super::super::find(id).unwrap();
        let reading = reading(ctx, id).unwrap();
        crate::widgets::template::render(metric.meta.templates[0], &reading)
            .iter()
            .map(|p| p.text.as_str())
            .collect()
    }

    #[test]
    fn all_activity_segments_are_single_line_and_ascii_marks_follow_the_tier() {
        let scenario = crate::scenarios::find("normal").unwrap();
        let input = scenario.input(crate::scenarios::BASE);
        let registry = Registry::default();
        for tier in [GlyphTier::Unicode, GlyphTier::Nerd, GlyphTier::Ascii] {
            let ctx = Ctx::new(
                &input,
                &registry,
                crate::scenarios::BASE,
                FixedOffset::east_opt(0).unwrap(),
                tier,
            )
            .with_git(crate::metrics::GitSource::Fixed(None))
            .with_transcript(TranscriptSource::Fixed(
                scenario.transcript(crate::scenarios::BASE),
            ))
            .with_mcp(McpSource::Fixed(scenario.mcp()))
            .with_state(scenario.state(crate::scenarios::BASE));
            for id in [
                "activity.tools",
                "activity.agents",
                "activity.todos",
                "activity.skills",
                "activity.hooks",
                "env",
            ] {
                let output = rendered(&ctx, id);
                assert!(!output.is_empty(), "{id}");
                assert!(!output.contains(['\n', '\r']), "{id}: {output}");
                if tier == GlyphTier::Ascii {
                    assert!(output.is_ascii(), "{id}: {output}");
                }
            }
            let agents = rendered(&ctx, "activity.agents");
            if tier == GlyphTier::Ascii {
                assert_eq!(
                    agents,
                    "* Explore [haiku-4.5 high] . scan the crate (1m 2s)"
                );
            } else {
                assert_eq!(
                    agents,
                    "◐ Explore [haiku-4.5 high] · scan the crate (1m 2s)"
                );
            }
        }
    }

    #[test]
    fn agent_recent_window_and_totals_survive_after_now_expires() {
        let input = Input::default();
        let registry = Registry::default();
        let mut summary = Summary {
            agent_count: 5,
            ..Summary::default()
        };
        for (id, end) in [
            ("running", None),
            ("recent", Some(95)),
            ("stale", Some(39)),
            ("future", Some(101)),
        ] {
            summary.agents.push(Agent {
                id: id.into(),
                kind: id.into(),
                start: 1,
                end,
                ..Agent::default()
            });
        }
        let ctx = Ctx::new(
            &input,
            &registry,
            100,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary.clone())));
        let result = reading(&ctx, "activity.agents").unwrap();
        assert_eq!(text(&result, "counts"), "Agent×5 (1 running)");
        assert_eq!(
            text(&result, "now"),
            "◐ running (1m 39s) | ✓ recent (1m 34s)"
        );
        summary.agents[0].end = Some(1);
        let ctx = Ctx::new(
            &input,
            &registry,
            1000,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary)));
        assert!(reading(&ctx, "activity.agents").is_none());
    }

    #[test]
    fn tool_now_limits_running_and_latest_results_and_shortens_mcp_names() {
        let input = Input::default();
        let registry = Registry::default();
        let mut summary = Summary::default();
        summary.tool_calls.count = 7;
        for (id, status) in [
            ("old", Status::Running),
            ("new", Status::Running),
            ("latest", Status::Running),
            ("done1", Status::Completed),
            ("done2", Status::Completed),
            ("done3", Status::Error),
        ] {
            summary.tool_calls.recent.push(Tool {
                id: id.into(),
                name: "mcp__docs__search".into(),
                target: id.into(),
                status,
                ..Tool::default()
            });
        }
        let ctx = Ctx::new(
            &input,
            &registry,
            100,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary)));
        let tools = reading(&ctx, "activity.tools").unwrap();
        assert_eq!(text(&tools, "now"), "◐ search: new | ◐ search: latest");
        assert_eq!(
            rendered(&ctx, "activity.tools"),
            "◐ search: new | ◐ search: latest | ✓ search ×3"
        );
    }

    #[test]
    fn todos_show_only_in_progress_or_all_done_and_skills_show_latest() {
        use crate::state::transcript::Todo;
        let input = Input::default();
        let registry = Registry::default();
        for (status, expected) in [
            ("pending", None),
            ("in_progress", Some("▸ work (0/1)")),
            ("completed", Some("✓ all done (1/1)")),
        ] {
            let summary = Summary {
                todos: vec![Todo {
                    content: "work".into(),
                    status: status.into(),
                }],
                ..Summary::default()
            };
            let ctx = Ctx::new(
                &input,
                &registry,
                100,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            )
            .with_transcript(TranscriptSource::Fixed(Some(summary)));
            assert_eq!(
                reading(&ctx, "activity.todos")
                    .map(|_| rendered(&ctx, "activity.todos"))
                    .as_deref(),
                expected
            );
        }
        let summary = Summary {
            skills: Box::new(Skills {
                count: 1,
                done: vec!["review".into()],
                running: vec![],
            }),
            ..Summary::default()
        };
        let ctx = Ctx::new(
            &input,
            &registry,
            100,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_transcript(TranscriptSource::Fixed(Some(summary)));
        assert_eq!(rendered(&ctx, "activity.skills"), "✓ review");
    }

    #[test]
    fn model_versions_dates_and_optional_badges() {
        assert_eq!(short_model("claude-haiku-4-5-20251001"), "haiku-4.5");
        assert_eq!(short_model("claude-sonnet-5-5"), "sonnet-5.5");
        assert_eq!(short_model("claude-3-5-sonnet-20241022"), "sonnet-3.5");
        assert_eq!(short_model("haiku"), "haiku");
        let input = Input::default();
        let registry = Registry::default();
        let ctx = Ctx::new(
            &input,
            &registry,
            1,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        let mut agent = Agent {
            kind: "Explore".into(),
            ..Agent::default()
        };
        assert_eq!(agent_now(&ctx, &agent), "◐ Explore (1s)");
        agent.effort = Some("high".into());
        assert_eq!(agent_now(&ctx, &agent), "◐ Explore [high] (1s)");
        agent.effort = None;
        agent.model = Some("claude-sonnet-5-5".into());
        assert_eq!(agent_now(&ctx, &agent), "◐ Explore [sonnet-5.5] (1s)");
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
                "activity.tools",
                "activity.hooks",
                "mcp",
            ] {
                assert!(reading(&ctx, id).is_none(), "{id}");
            }
        }
    }
}
