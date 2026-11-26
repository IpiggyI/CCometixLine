use super::{Ctx, Metric, MetricMeta, OptKind, OptSpec, Options, Reading};
use crate::style::{Category, IconKey, Role, Severity};
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
        widget: WidgetKind::Text,
        templates,
        fields,
        levels: &[],
        min_cc: None,
        needs_mod: false,
        options: &[],
        stateful: false,
        priority: 3,
    }
}

pub(super) fn metrics() -> Vec<Metric> {
    vec![
        Metric {
            meta: MetricMeta {
                priority: 5,
                options: &[
                    OptSpec {
                        key: "effort",
                        kind: OptKind::Flag,
                        default: "true",
                        summary: "Add the reasoning effort while thinking is on",
                    },
                    OptSpec {
                        key: "api_model",
                        kind: OptKind::Flag,
                        default: "true",
                        summary: "Show the backend model id from the session DB when it differs from the alias",
                    },
                ],
                ..meta(
                    "model",
                    Category::Identity,
                    "Model",
                    "Model name, corrected by the model registry, with its reasoning effort",
                    IconKey::Model,
                    [
                        "{name}[ · {effort}][ · {fast}][ · {api_model}]",
                        "{name}[ · {effort}][ · {fast}]",
                        "{family}[ · {fast}]",
                    ],
                    &["name", "family", "id", "effort", "fast", "api_model"],
                )
            },
            compute: model,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "output_style",
                    Category::Identity,
                    "Output style",
                    "Output style name; hidden for the default style unless show_default = true",
                    IconKey::OutputStyle,
                    ["{name}", "{name}", "{name}"],
                    &["name"],
                )
            },
            compute: output_style,
        },
        Metric {
            meta: MetricMeta {
                priority: 4,
                ..meta(
                    "vim",
                    Category::Identity,
                    "Vim mode",
                    "Vim editor mode while vim mode is on",
                    IconKey::Vim,
                    ["{mode}", "{mode}", "{short}"],
                    &["mode", "short"],
                )
            },
            compute: vim,
        },
        Metric {
            meta: meta(
                "agent",
                Category::Identity,
                "Agent",
                "Agent name when started with --agent",
                IconKey::Agent,
                ["{name}", "{name}", "{name}"],
                &["name"],
            ),
            compute: agent,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "session_name",
                    Category::Session,
                    "Session name",
                    "Name from /rename or the generated title; max_len (default 30)",
                    IconKey::SessionName,
                    ["{name}", "{name}", "{name}"],
                    &["name"],
                )
            },
            compute: session_name,
        },
        Metric {
            meta: MetricMeta {
                priority: 1,
                ..meta(
                    "cc_version",
                    Category::System,
                    "Claude Code version",
                    "Version of the running Claude Code",
                    IconKey::Version,
                    ["{version}", "{version}", "{version}"],
                    &["version"],
                )
            },
            compute: cc_version,
        },
        Metric {
            meta: MetricMeta {
                priority: 1,
                stateful: true,
                ..meta(
                    "update",
                    Category::System,
                    "Update",
                    "A newer agyline is published; links to its release notes",
                    IconKey::Update,
                    ["{version} available", "{version}", "{version}"],
                    &["version", "current"],
                )
            },
            compute: update,
        },
        Metric {
            meta: MetricMeta {
                priority: 1,
                ..meta(
                    "auth",
                    Category::System,
                    "Auth",
                    "Antigravity account plan tier and signed-in email",
                    IconKey::Auth,
                    ["{user}", "{user}", "{user}"],
                    &["user", "plan", "email", "name"],
                )
            },
            compute: auth,
        },
        Metric {
            meta: MetricMeta {
                priority: 3,
                ..meta(
                    "agy.state",
                    Category::Activity,
                    "Agent state",
                    "Live Antigravity agent execution state",
                    IconKey::State,
                    ["{state}", "{state}", "{short}"],
                    &["state", "short"],
                )
            },
            compute: agy_state,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "agy.mode",
                    Category::System,
                    "Cycle mode",
                    "Antigravity cycle mode and sandbox indicator",
                    IconKey::Mode,
                    ["{mode}[ · {sandbox}]", "{mode}", "{short}"],
                    &["mode", "short", "sandbox"],
                )
            },
            compute: agy_mode,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "agy.tasks",
                    Category::Activity,
                    "Tasks & artifacts",
                    "Count of active Antigravity tasks and artifacts",
                    IconKey::Todos,
                    ["{summary}", "{tasks}", "{total}"],
                    &["summary", "tasks", "artifacts", "total"],
                )
            },
            compute: agy_tasks,
        },
    ]
}

fn model(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let resolved = ctx.model()?;
    let family = resolved.name.split_whitespace().next().unwrap_or_default();
    let stdin_id = ctx.input.model_id().unwrap_or_default();
    let id = ctx
        .state
        .db
        .as_ref()
        .and_then(|d| d.api_model.as_deref())
        .or_else(|| ctx.input.model_id())
        .unwrap_or_default();
    let mut r = Reading::default()
        .set("name", &resolved.name)
        .set("family", family)
        .set("id", id);
    let with_effort = opts.get("effort").and_then(|v| v.as_bool()).unwrap_or(true);
    if with_effort && let Some(level) = effort(ctx) {
        r = r.set("effort", level);
    }
    if ctx.input.fast_mode == Some(true) {
        r = r.set("fast", "fast");
    }
    let with_api = opts
        .get("api_model")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let is_orchestrator = stdin_id.to_ascii_lowercase().starts_with("orchestrator");
    if with_api
        && is_orchestrator
        && let Some(api) = active_backend_model(ctx)
        && !api.eq_ignore_ascii_case(&resolved.name)
    {
        r = r.set_role("api_model", api, Role::Muted);
    }
    Some(r)
}

fn active_backend_model(ctx: &Ctx) -> Option<String> {
    if let Some(db) = ctx.state.db.as_ref()
        && let Some(api) = db.api_model.as_deref()
    {
        if id_effort_suffix(api).is_none()
            && !api.to_ascii_lowercase().ends_with("-thinking")
            && let Some(eff) = db.display_model.as_deref().and_then(display_effort_suffix)
        {
            return Some(format!("{api}-{}", eff.trim().to_ascii_lowercase()));
        }
        return Some(api.to_string());
    }
    ctx.transcript().and_then(|t| t.last_model.clone())
}

fn normalize_effort(raw: &str) -> Option<String> {
    let lower = raw.trim().to_lowercase();
    if lower.is_empty() {
        return None;
    }
    Some(match lower.as_str() {
        "low" => "Low".into(),
        "med" | "medium" => "Med".into(),
        "high" => "High".into(),
        "xhigh" => "xHigh".into(),
        "max" => "Max".into(),
        other => {
            let mut c = other.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect())
                .unwrap_or_default()
        }
    })
}

fn display_effort_suffix(display_name: &str) -> Option<&str> {
    let trimmed = display_name.trim_end();
    let inner = trimmed.strip_suffix(')')?.rsplit('(').next()?;
    let lower = inner.trim().to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "low" | "med" | "medium" | "high" | "xhigh" | "max"
    )
    .then_some(inner)
}

fn id_effort_suffix(id: &str) -> Option<&str> {
    let (_, tail) = id.trim().rsplit_once('-')?;
    let lower = tail.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "low" | "med" | "medium" | "high" | "xhigh" | "max"
    )
    .then_some(tail)
}

/// The reasoning effort as `Low`, `Med`, `High`, `xHigh` or `Max`, for
/// models that have one. Thinking and effort go together: with thinking off
/// there is none to show.
fn effort(ctx: &Ctx) -> Option<String> {
    if ctx.input.thinking.as_ref().and_then(|t| t.enabled) == Some(false) {
        return None;
    }
    let is_orchestrator = ctx
        .input
        .model_id()
        .is_some_and(|id| id.to_ascii_lowercase().starts_with("orchestrator"));
    let planner = is_orchestrator
        .then(|| ctx.orchestrator_planner())
        .flatten();
    let raw = ctx
        .input
        .effort
        .as_ref()
        .and_then(|e| e.level.as_deref())
        .or_else(|| ctx.input.model.as_ref().and_then(|m| m.effort.as_deref()))
        .or_else(|| {
            ctx.input
                .model
                .as_ref()
                .and_then(|m| m.display_name.as_deref())
                .and_then(display_effort_suffix)
        })
        .or_else(|| planner.and_then(display_effort_suffix))
        .or_else(|| planner.and_then(id_effort_suffix))
        .or_else(|| {
            ctx.state
                .db
                .as_ref()
                .and_then(|d| d.display_model.as_deref())
                .and_then(display_effort_suffix)
        })
        .or_else(|| {
            ctx.transcript()
                .and_then(|t| t.selected_model.as_deref())
                .and_then(display_effort_suffix)
        })
        .or_else(|| {
            ctx.state
                .db
                .as_ref()
                .and_then(|d| d.api_model.as_deref())
                .or_else(|| ctx.transcript().and_then(|t| t.last_model.as_deref()))
                .and_then(id_effort_suffix)
        })?;
    normalize_effort(raw)
}

fn output_style(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let name = ctx.input.output_style.as_ref()?.name.as_deref()?;
    let show_default = opts
        .get("show_default")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if name.eq_ignore_ascii_case("default") && !show_default {
        return None;
    }
    Some(Reading::default().set("name", name))
}

fn vim(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let mode = ctx.input.vim.as_ref()?.mode.as_deref().unwrap_or("INSERT");
    let short = match mode {
        "VISUAL LINE" => "VL".to_string(),
        m => m.chars().next().map(String::from).unwrap_or_default(),
    };
    Some(Reading::default().set("mode", mode).set("short", short))
}

fn agent(ctx: &Ctx, _: &Options) -> Option<Reading> {
    Some(Reading::default().set("name", ctx.input.agent.as_ref()?.name.as_deref()?))
}

fn session_name(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let owned;
    let name = match ctx.input.title() {
        Some(t) => t,
        None => {
            owned = ctx
                .input
                .conv_id()
                .and_then(crate::state::transcript::read_annotation_title)?;
            &owned
        }
    };
    let max = opts
        .get("max_len")
        .and_then(|v| v.as_integer())
        .unwrap_or(30)
        .max(4) as usize;
    Some(Reading::default().set("name", crate::render::width::truncate(name, max)))
}

fn auth(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let plan = ctx
        .input
        .plan_tier
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let email = ctx
        .input
        .email
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    if plan.is_none() && email.is_none() {
        return None;
    }
    let mut r = Reading::default();
    if let Some(p) = plan {
        r = r.set("plan", p);
    }
    if let Some(e) = email {
        r = r.set("email", e);
    }
    if let Some(user) = email
        .and_then(|e| e.split('@').next())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or(plan)
    {
        r = r.set("user", user);
    }
    let combined = match (plan, email) {
        (Some(p), Some(e)) => format!("{p} ({e})"),
        (Some(p), None) => p.to_string(),
        (None, Some(e)) => e.to_string(),
        (None, None) => unreachable!(),
    };
    Some(r.set("name", combined))
}

fn agy_state(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let raw = if ctx.input.tool_confirmation_pending == Some(true) {
        "approval"
    } else if ctx.input.pending_input_count.unwrap_or(0) > 0 {
        "input"
    } else {
        ctx.input
            .agent_state
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())?
    };
    let lower = raw.to_ascii_lowercase();
    let short = lower
        .chars()
        .next()
        .map(|c| c.to_ascii_uppercase().to_string())?;
    let mut r = Reading::default().set("state", &lower).set("short", short);
    match lower.as_str() {
        "approval" | "waiting_approval" | "error" | "failed" => {
            r = r
                .set_role("state", &lower, Role::Bad)
                .severity(Severity::Crit);
        }
        "running" | "thinking" | "working" | "input" => {
            r = r
                .set_role("state", &lower, Role::Fill(Severity::Warn))
                .severity(Severity::Warn);
        }
        "idle" | "done" => {
            r = r.set_role("state", &lower, Role::Muted);
        }
        _ => {}
    }
    Some(r)
}

fn agy_mode(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let mode = ctx
        .input
        .cycle_mode
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());
    let sandbox = ctx
        .input
        .sandbox
        .as_ref()
        .and_then(|s| s.enabled)
        .filter(|e| *e);
    if mode.is_none() && sandbox.is_none() {
        return None;
    }
    let mut r = Reading::default();
    if let Some(m) = mode {
        let short = m
            .chars()
            .next()
            .map(|c| c.to_ascii_uppercase().to_string())
            .unwrap_or_default();
        r = r.set("mode", m).set("short", short);
    }
    if sandbox == Some(true) {
        r = r.set("sandbox", "sandbox");
    }
    Some(r)
}

fn agy_tasks(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let tasks = ctx.input.task_count.unwrap_or(0);
    let artifacts = ctx.input.artifact_count.unwrap_or(0);
    if tasks == 0 && artifacts == 0 {
        return None;
    }
    let tasks_str = (tasks > 0).then(|| {
        if tasks == 1 {
            "1 task".to_string()
        } else {
            format!("{tasks} tasks")
        }
    });
    let artifacts_str = (artifacts > 0).then(|| {
        if artifacts == 1 {
            "1 artifact".to_string()
        } else {
            format!("{artifacts} artifacts")
        }
    });
    let mut parts = Vec::new();
    if let Some(ref s) = tasks_str {
        parts.push(s.as_str());
    }
    if let Some(ref s) = artifacts_str {
        parts.push(s.as_str());
    }
    let mut r = Reading::default()
        .set("summary", parts.join(" · "))
        .set("total", (tasks + artifacts).to_string());
    if let Some(s) = tasks_str {
        r = r.set("tasks", s);
    }
    if artifacts > 0 {
        r = r.set("artifacts", artifacts.to_string());
    }
    Some(r)
}

fn cc_version(ctx: &Ctx, _: &Options) -> Option<Reading> {
    Some(Reading::default().set("version", ctx.input.version.as_deref()?))
}

fn update(ctx: &Ctx, _: &Options) -> Option<Reading> {
    use crate::state::update::{CURRENT, RELEASES};
    let latest = ctx.state.latest_version.as_deref()?;
    Some(
        Reading::default()
            .set("version", format!("v{latest}"))
            .set("current", format!("v{CURRENT}"))
            .link(Some(format!("{RELEASES}/tag/v{latest}"))),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Input;
    use crate::metrics::{find, read};
    use crate::models::Registry;
    use crate::style::GlyphTier;
    use chrono::FixedOffset;

    /// The model item at `level` (0 full, 1 compact) for `json`.
    fn model_text(json: &str, opts: &str, level: usize) -> String {
        let input = Input::parse(json).unwrap();
        let reg = Registry::default();
        let ctx = Ctx::new(
            &input,
            &reg,
            0,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        let m = find("model").unwrap();
        let opts: Options = toml::from_str(opts).unwrap();
        let r = read(m, &ctx, &opts).unwrap();
        crate::widgets::template::render(m.meta.templates[level], &r)
            .iter()
            .map(|p| p.text.as_str())
            .collect()
    }

    const OPUS: &str = r#""model": {"id": "claude-opus-4-8", "display_name": "Opus 4.8"}"#;

    #[test]
    fn model_shows_fast_independently_of_effort() {
        let json = format!(r#"{{{OPUS}, "fast_mode": true, "effort": {{"level": "xhigh"}}}}"#);
        for level in [0, 1] {
            assert_eq!(model_text(&json, "", level), "Opus 4.8 · xHigh · fast");
            assert_eq!(
                model_text(&json, "effort = false", level),
                "Opus 4.8 · fast"
            );
        }
        assert_eq!(model_text(&json, "", 2), "Opus · fast");
        let off = format!(r#"{{{OPUS}, "fast_mode": false}}"#);
        assert_eq!(model_text(&off, "", 0), "Opus 4.8");
    }

    #[test]
    fn model_carries_the_effort_while_thinking() {
        let on = format!(
            r#"{{{OPUS}, "effort": {{"level": "high"}}, "thinking": {{"enabled": true}}}}"#
        );
        assert_eq!(model_text(&on, "", 0), "Opus 4.8 · High");
        assert_eq!(model_text(&on, "", 1), "Opus 4.8 · High");
        for (level, shown) in [
            ("low", "Low"),
            ("medium", "Med"),
            ("xhigh", "xHigh"),
            ("max", "Max"),
        ] {
            let json = format!(r#"{{{OPUS}, "effort": {{"level": "{level}"}}}}"#);
            assert_eq!(model_text(&json, "", 0), format!("Opus 4.8 · {shown}"));
        }
        assert_eq!(model_text(&on, "effort = false", 0), "Opus 4.8");
        let off = format!(
            r#"{{{OPUS}, "effort": {{"level": "high"}}, "thinking": {{"enabled": false}}}}"#
        );
        assert_eq!(model_text(&off, "", 0), "Opus 4.8");
        let none = format!(r#"{{{OPUS}}}"#);
        assert_eq!(model_text(&none, "", 0), "Opus 4.8");
    }

    #[test]
    fn orchestrator_shows_planner_family_and_active_backend_model() {
        let json =
            r#"{"model": {"id": "Orchestrator (Neon)", "display_name": "Orchestrator (Neon)"}}"#;
        let input = Input::parse(json).unwrap();
        let reg = Registry::default();
        let state = crate::state::StateView {
            db: Some(crate::state::agy_db::DbSummary {
                api_model: Some("gemini-4-hydrogen".into()),
                display_model: Some("Gemini 4 Hydrogen (High)".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx = Ctx::new(
            &input,
            &reg,
            0,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_state(state)
        .with_transcript(crate::metrics::TranscriptSource::Fixed(None))
        .with_orchestrator_planner(Some("Gemini 4 Carbon (High)".into()));
        let m = find("model").unwrap();
        let r = read(m, &ctx, &Options::default()).unwrap();
        let text: String = crate::widgets::template::render(m.meta.templates[0], &r)
            .iter()
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(
            text,
            "Orchestrator (Carbon) · High · gemini-4-hydrogen-high"
        );
    }

    #[test]
    fn non_orchestrator_omits_backend_api_model() {
        let json = r#"{"model": {"id": "Gemini 4 Carbon (High)", "display_name": "Gemini 4 Carbon (High)"}}"#;
        let input = Input::parse(json).unwrap();
        let reg = Registry::default();
        let state = crate::state::StateView {
            db: Some(crate::state::agy_db::DbSummary {
                api_model: Some("gemini-4-carbon".into()),
                display_model: Some("Gemini 4 Carbon (High)".into()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let ctx = Ctx::new(
            &input,
            &reg,
            0,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        )
        .with_state(state)
        .with_transcript(crate::metrics::TranscriptSource::Fixed(None));
        let m = find("model").unwrap();
        let r = read(m, &ctx, &Options::default()).unwrap();
        let text: String = crate::widgets::template::render(m.meta.templates[0], &r)
            .iter()
            .map(|p| p.text.as_str())
            .collect();
        assert_eq!(text, "Gemini 4 Carbon · High");
    }
}
