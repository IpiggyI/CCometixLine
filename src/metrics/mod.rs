//! Metrics: named data sources. Each one reads the input (and later state or
//! the mod bridge) and produces a `Reading` — text fields plus optional
//! numbers for graphs — without knowing how it will be displayed.

mod code;
mod custom;
pub(crate) mod environment;
pub mod fmt;
mod identity;
mod live;
mod mcp;
mod projection;
mod task;
mod transcript;
mod usage;

use crate::env::AutoCompact;
use crate::git::GitInfo;
use crate::input::{Input, Task};
use crate::models::{Registry, Resolved};
use crate::state::StateView;
use crate::style::{Category, GlyphTier, IconKey, Role, Severity};
use crate::version::Version;
use crate::widgets::WidgetKind;
use chrono::FixedOffset;
use std::cell::OnceCell;
use std::path::Path;

/// Per-item metric options from the config (e.g. `dir`'s `style`).
pub type Options = toml::Table;

#[derive(Clone, Debug, PartialEq)]
pub struct Field {
    pub text: String,
    pub role: Option<Role>,
}

/// What a metric produced for this render.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Values for text templates, e.g. `pct` → `62`.
    pub fields: Vec<(&'static str, Field)>,
    /// Fill level for bars and gauges, 0..1 (may exceed 1).
    pub ratio: Option<f64>,
    /// Share of a time window already passed, 0..1, for pace bars.
    pub elapsed: Option<f64>,
    /// History for sparklines, oldest first.
    pub series: Option<Vec<f64>>,
    /// Severity decided by the metric itself; otherwise thresholds on `ratio`.
    pub severity: Option<Severity>,
    /// The ratio counts down, like time left: thresholds read how much of it
    /// is used up (`1 - ratio`), so the low end is the bad one.
    pub draining: bool,
    /// Replaces the metric's default icon.
    pub icon: Option<IconKey>,
    /// Short group label rendered after the icon (e.g. `gemini` / `3p`).
    pub prefix: Option<String>,
    pub link: Option<String>,
}

impl Reading {
    /// Set a text field; empty values are left unset so optional template
    /// groups that use them disappear.
    pub fn set(self, name: &'static str, text: impl Into<String>) -> Self {
        self.set_with(name, text, None)
    }

    pub fn set_role(self, name: &'static str, text: impl Into<String>, role: Role) -> Self {
        self.set_with(name, text, Some(role))
    }

    fn set_with(mut self, name: &'static str, text: impl Into<String>, role: Option<Role>) -> Self {
        let text = text.into();
        if !text.is_empty() {
            self.fields.retain(|(n, _)| *n != name);
            self.fields.push((name, Field { text, role }));
        }
        self
    }

    pub fn get(&self, name: &str) -> Option<&Field> {
        self.fields.iter().find(|(n, _)| *n == name).map(|(_, f)| f)
    }

    pub fn ratio(mut self, ratio: f64) -> Self {
        self.ratio = Some(ratio);
        self
    }

    pub fn severity(mut self, severity: Severity) -> Self {
        self.severity = Some(severity);
        self
    }

    pub fn draining(mut self) -> Self {
        self.draining = true;
        self
    }

    pub fn prefix(mut self, prefix: impl Into<String>) -> Self {
        let p = prefix.into();
        if !p.is_empty() {
            self.prefix = Some(p);
        }
        self
    }

    pub fn link(mut self, link: Option<String>) -> Self {
        self.link = link;
        self
    }
}

/// Default thresholds on a ratio, in percent.
pub const LEVELS: &[(f64, Severity)] = &[(50.0, Severity::Warn), (80.0, Severity::Crit)];

/// How a metric option is edited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptKind {
    Text,
    Number,
    Flag,
    Choice(&'static [&'static str]),
}

/// An option a metric reads from its item's `options` table.
#[derive(Debug)]
pub struct OptSpec {
    pub key: &'static str,
    pub kind: OptKind,
    /// The value used when unset, as shown to the user.
    pub default: &'static str,
    pub summary: &'static str,
}

pub struct MetricMeta {
    pub id: &'static str,
    pub category: Category,
    pub name: &'static str,
    pub summary: &'static str,
    pub icon: IconKey,
    pub widget: WidgetKind,
    /// Default text templates for the full, compact and minimal levels.
    pub templates: [&'static str; 3],
    /// Fields the templates may use.
    pub fields: &'static [&'static str],
    pub levels: &'static [(f64, Severity)],
    /// Oldest Claude Code that provides the data.
    pub min_cc: Option<Version>,
    pub needs_mod: bool,
    /// Options read from the item's `options` table.
    pub options: &'static [OptSpec],
    pub stateful: bool,
    pub priority: u8,
}

pub struct Metric {
    pub meta: MetricMeta,
    pub compute: fn(&Ctx, &Options) -> Option<Reading>,
}

/// Where git status comes from.
#[derive(Clone)]
pub enum GitSource {
    /// Run git in the working directory, once per render.
    Live,
    /// Run git, reusing a cached result younger than this many milliseconds.
    Cached(u64),
    /// Fixed data, for previews and tests.
    Fixed(Option<GitInfo>),
}

/// Where session transcript data comes from.
#[derive(Clone)]
#[allow(clippy::large_enum_variant)]
pub enum TranscriptSource {
    Live,
    Fixed(Option<crate::state::transcript::Summary>),
}

/// Where enabled MCP server names come from.
#[derive(Clone)]
pub enum McpSource {
    Live,
    Fixed(Option<Vec<String>>),
}

/// How `cmd` items get their output.
#[derive(Clone, Debug, Default)]
pub enum Commands {
    /// Run them (through the cache), passing this payload on stdin.
    Run { stdin: String },
    /// Show the command line instead of running it, for previews and tests.
    #[default]
    Show,
}

/// Everything metrics may read during one render.
pub struct Ctx<'a> {
    pub input: &'a Input,
    pub registry: &'a Registry,
    /// Unix epoch seconds.
    pub now: i64,
    pub tz: FixedOffset,
    pub glyphs: GlyphTier,
    pub cc: Option<Version>,
    /// History and cross-session data; empty when state is unavailable.
    pub state: StateView,
    pub compact: AutoCompact,
    pub commands: Commands,
    /// The background task a subagent line describes.
    pub task: Option<&'a Task>,
    git: GitSource,
    git_cache: OnceCell<Option<GitInfo>>,
    model_cache: OnceCell<Option<Resolved>>,
    orchestrator_cache: OnceCell<Option<String>>,
    transcript: TranscriptSource,
    transcript_cache: OnceCell<Option<crate::state::transcript::Summary>>,
    mcp: McpSource,
    mcp_cache: OnceCell<Option<Vec<String>>>,
    environment_cache: OnceCell<environment::Environment>,
}

impl<'a> Ctx<'a> {
    pub fn new(
        input: &'a Input,
        registry: &'a Registry,
        now: i64,
        tz: FixedOffset,
        glyphs: GlyphTier,
    ) -> Self {
        Ctx {
            input,
            registry,
            now,
            tz,
            glyphs,
            cc: input.cc_version(),
            state: StateView::default(),
            compact: AutoCompact::default(),
            commands: Commands::Show,
            task: None,
            git: GitSource::Live,
            git_cache: OnceCell::new(),
            model_cache: OnceCell::new(),
            orchestrator_cache: OnceCell::new(),
            transcript: TranscriptSource::Live,
            transcript_cache: OnceCell::new(),
            mcp: McpSource::Live,
            mcp_cache: OnceCell::new(),
            environment_cache: OnceCell::new(),
        }
    }

    pub fn with_orchestrator_planner(mut self, planner: Option<String>) -> Self {
        let cell = OnceCell::new();
        let _ = cell.set(planner);
        self.orchestrator_cache = cell;
        self
    }

    pub fn with_git(mut self, git: GitSource) -> Self {
        self.git = git;
        self
    }

    pub fn with_transcript(mut self, source: TranscriptSource) -> Self {
        self.transcript = source;
        self
    }

    pub fn with_mcp(mut self, source: McpSource) -> Self {
        self.mcp = source;
        self
    }

    pub fn transcript(&self) -> Option<&crate::state::transcript::Summary> {
        self.transcript_cache
            .get_or_init(|| match &self.transcript {
                TranscriptSource::Live => self.state.transcript.clone().or_else(|| {
                    crate::state::transcript::resolve_path(self.input)
                        .and_then(|p| crate::state::transcript::read(&p))
                }),
                TranscriptSource::Fixed(summary) => summary.clone(),
            })
            .as_ref()
    }

    pub fn mcp(&self) -> Option<&[String]> {
        self.mcp_cache
            .get_or_init(|| match &self.mcp {
                McpSource::Live => mcp::servers(self.input),
                McpSource::Fixed(names) => names.clone(),
            })
            .as_deref()
    }

    fn environment(&self) -> &environment::Environment {
        self.environment_cache.get_or_init(|| match self.git {
            GitSource::Fixed(_) => self.state.environment.clone().unwrap_or_default(),
            _ => environment::collect(self.input.project_dir().map(Path::new)),
        })
    }

    pub fn with_state(mut self, state: StateView) -> Self {
        self.state = state;
        self
    }

    pub fn with_compact(mut self, compact: AutoCompact) -> Self {
        self.compact = compact;
        self
    }

    pub fn with_commands(mut self, commands: Commands) -> Self {
        self.commands = commands;
        self
    }

    pub fn with_task(mut self, task: &'a Task) -> Self {
        self.task = Some(task);
        self
    }

    pub fn git(&self) -> Option<&GitInfo> {
        self.git_cache
            .get_or_init(|| {
                let dir = || self.input.current_dir().map(Path::new);
                match &self.git {
                    GitSource::Fixed(info) => info.clone(),
                    GitSource::Live => crate::git::status(dir()?),
                    GitSource::Cached(ttl) => crate::state::git_cache::status(dir()?, *ttl),
                }
            })
            .as_ref()
    }

    /// Configured planner model when `input.model` is an `Orchestrator (...)` wrapper.
    pub fn orchestrator_planner(&self) -> Option<&str> {
        let is_orchestrator = self
            .input
            .model_id()
            .is_some_and(|id| id.to_ascii_lowercase().starts_with("orchestrator"));
        if !is_orchestrator {
            return None;
        }
        self.orchestrator_cache
            .get_or_init(|| {
                if matches!(self.transcript, TranscriptSource::Live) {
                    let path = crate::paths::agy_cli_dir().join("settings.json");
                    if let Ok(raw) = std::fs::read_to_string(path)
                        && let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw)
                        && let Some(planner) = val
                            .get("orchestrator")
                            .and_then(|o| o.get("planner"))
                            .and_then(|p| p.as_str())
                            .map(str::trim)
                            .filter(|s| !s.is_empty())
                    {
                        return Some(planner.to_string());
                    }
                }
                self.transcript()
                    .and_then(|t| t.selected_model.as_deref())
                    .map(str::trim)
                    .filter(|s| {
                        !s.is_empty() && !s.to_ascii_lowercase().starts_with("orchestrator")
                    })
                    .map(str::to_string)
            })
            .as_deref()
    }

    /// The model as the registry resolves it. When `input.model` is absent,
    /// fall back to the active backend model from the SQLite conversation DB or
    /// session transcript. When `input.model` is an `Orchestrator (...)` wrapper,
    /// preserve the Orchestrator prefix and refine its family from the configured
    /// planner when known.
    pub fn model(&self) -> Option<&Resolved> {
        self.model_cache
            .get_or_init(|| {
                let window = self
                    .input
                    .context_window
                    .as_ref()
                    .and_then(|c| c.context_window_size);
                let stdin_model = self.input.model.as_ref();
                let stdin_id =
                    stdin_model.and_then(|m| m.id.as_deref().or(m.display_name.as_deref()));
                if stdin_model.is_none() {
                    if let Some(db) = self.state.db.as_ref()
                        && (db.api_model.is_some() || db.display_model.is_some())
                    {
                        let id = db.api_model.as_deref().or(db.display_model.as_deref())?;
                        return Some(self.registry.resolve(
                            id,
                            db.display_model.as_deref(),
                            window,
                        ));
                    }
                    if let Some(t) = self.transcript()
                        && (t.selected_model.is_some() || t.last_model.is_some())
                    {
                        let id = t.last_model.as_deref().or(t.selected_model.as_deref())?;
                        return Some(self.registry.resolve(
                            id,
                            t.selected_model.as_deref(),
                            window,
                        ));
                    }
                }
                let model = stdin_model?;
                let id = stdin_id?;
                let mut resolved = self
                    .registry
                    .resolve(id, model.display_name.as_deref(), window);
                if id.to_ascii_lowercase().starts_with("orchestrator")
                    && let Some(planner) = self.orchestrator_planner()
                    && let Some(fam) = orchestrator_family(planner, self.registry)
                {
                    resolved.name = format!("Orchestrator ({fam})");
                }
                Some(resolved)
            })
            .as_ref()
    }

    /// Whether the running Claude Code is at least `min`; unknown versions
    /// are assumed new enough and left to field presence.
    pub fn supports(&self, min: Version) -> bool {
        self.cc.is_none_or(|v| v >= min)
    }
}

fn orchestrator_family(planner: &str, registry: &Registry) -> Option<String> {
    let res = registry.resolve(planner, Some(planner), None);
    let name = res.name.trim();
    if name.is_empty() || name.to_ascii_lowercase().starts_with("orchestrator") {
        return None;
    }
    let lower = name.to_ascii_lowercase();
    for prefix in [
        "gemini 4 ",
        "gemini 3.8 ",
        "gemini 3.6 ",
        "gemini 3.5 ",
        "gemini 3.1 ",
        "gemini 3 ",
        "gemini 2.5 ",
        "claude ",
    ] {
        if lower.starts_with(prefix) {
            let rest = name[prefix.len()..].trim();
            if !rest.is_empty() {
                return Some(rest.to_string());
            }
        }
    }
    Some(name.to_string())
}

pub fn catalog() -> &'static [Metric] {
    static CATALOG: std::sync::OnceLock<Vec<Metric>> = std::sync::OnceLock::new();
    CATALOG.get_or_init(|| {
        let mut all = Vec::new();
        all.extend(identity::metrics());
        all.extend(usage::metrics());
        all.extend(projection::metrics());
        all.extend(code::metrics());
        all.extend(custom::metrics());
        all.extend(live::metrics());
        all.extend(task::metrics());
        all.extend(transcript::metrics());
        all.push(mcp::metric());
        all.push(environment::metric());
        all
    })
}

pub fn find(id: &str) -> Option<&'static Metric> {
    catalog().iter().find(|m| m.meta.id == id)
}

/// Compute `metric` if the running Claude Code supports it.
pub fn read(metric: &Metric, ctx: &Ctx, opts: &Options) -> Option<Reading> {
    if metric.meta.min_cc.is_some_and(|min| !ctx.supports(min)) {
        return None;
    }
    (metric.compute)(ctx, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_templates_use_declared_fields() {
        let mut seen = std::collections::HashSet::new();
        for m in catalog() {
            assert!(seen.insert(m.meta.id), "duplicate {}", m.meta.id);
            for o in m.meta.options {
                if let OptKind::Choice(values) = o.kind {
                    assert!(values.contains(&o.default), "{}.{}", m.meta.id, o.key);
                }
            }
            for t in m.meta.templates {
                let mut rest = t;
                while let Some(start) = rest.find('{') {
                    let end = rest[start..].find('}').expect("closed field") + start;
                    let name = &rest[start + 1..end];
                    assert!(
                        m.meta.fields.contains(&name),
                        "{}: {name} not declared",
                        m.meta.id
                    );
                    rest = &rest[end + 1..];
                }
            }
        }
    }

    #[test]
    fn gates_on_claude_code_version() {
        let input = Input::parse(r#"{"version": "2.1.200"}"#).unwrap();
        let reg = Registry::default();
        let ctx = Ctx::new(
            &input,
            &reg,
            0,
            FixedOffset::east_opt(0).unwrap(),
            GlyphTier::Unicode,
        );
        let breakdown = find("context.breakdown").unwrap();
        assert!(read(breakdown, &ctx, &Options::new()).is_none());
    }
}
