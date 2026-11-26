//! The JSON Antigravity CLI (and Claude Code) writes to the status line command's stdin.
//!
//! Every field is optional and parsed leniently: a top-level field with an
//! unexpected shape is dropped on its own instead of failing the whole input,
//! so a schema change upstream degrades one widget rather than the line.

use crate::version::Version;
use chrono::DateTime;
use serde::{Deserialize, Deserializer};
use std::collections::BTreeMap;

fn lenient<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).ok())
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Input {
    #[serde(deserialize_with = "lenient")]
    pub session_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub conversation_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub session_name: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub conversation_title: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub transcript_path: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub cwd: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub prompt_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub version: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub product: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub terminal_width: Option<usize>,
    #[serde(deserialize_with = "lenient")]
    pub model: Option<Model>,
    #[serde(deserialize_with = "lenient")]
    pub workspace: Option<Workspace>,
    #[serde(deserialize_with = "lenient")]
    pub output_style: Option<OutputStyle>,
    #[serde(deserialize_with = "lenient")]
    pub cost: Option<Cost>,
    #[serde(deserialize_with = "lenient")]
    pub context_window: Option<ContextWindow>,
    #[serde(deserialize_with = "lenient")]
    pub exceeds_200k_tokens: Option<bool>,
    #[serde(deserialize_with = "lenient")]
    pub effort: Option<Effort>,
    #[serde(deserialize_with = "lenient")]
    pub thinking: Option<Thinking>,
    #[serde(deserialize_with = "lenient")]
    pub fast_mode: Option<bool>,
    #[serde(deserialize_with = "lenient")]
    pub rate_limits: Option<RateLimits>,
    #[serde(deserialize_with = "lenient")]
    pub quota: Option<BTreeMap<String, Option<QuotaBucket>>>,
    #[serde(deserialize_with = "lenient")]
    pub prompt_cache: Option<PromptCache>,
    #[serde(deserialize_with = "lenient")]
    pub vim: Option<Vim>,
    #[serde(deserialize_with = "lenient")]
    pub agent: Option<Agent>,
    #[serde(deserialize_with = "lenient")]
    pub agent_state: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub cycle_mode: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub sandbox: Option<Sandbox>,
    #[serde(deserialize_with = "lenient")]
    pub tool_confirmation_pending: Option<bool>,
    #[serde(deserialize_with = "lenient")]
    pub pending_input_count: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub task_count: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub artifact_count: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub plan_tier: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub email: Option<String>,
    #[serde(deserialize_with = "lenient_list")]
    pub subagents: Vec<StdinSubagent>,
    #[serde(deserialize_with = "lenient")]
    pub vcs: Option<Vcs>,
    #[serde(deserialize_with = "lenient")]
    pub remote: Option<Remote>,
    #[serde(deserialize_with = "lenient")]
    pub pr: Option<PullRequest>,
    #[serde(deserialize_with = "lenient")]
    pub worktree: Option<Worktree>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Model {
    pub id: Option<String>,
    pub display_name: Option<String>,
    pub effort: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Workspace {
    pub current_dir: Option<String>,
    pub project_dir: Option<String>,
    pub added_dirs: Vec<String>,
    pub git_worktree: Option<String>,
    pub repo: Option<Repo>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Repo {
    pub host: Option<String>,
    pub owner: Option<String>,
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct OutputStyle {
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Cost {
    pub total_cost_usd: Option<f64>,
    pub total_usd: Option<f64>,
    pub subagent_usd: Option<f64>,
    pub estimated: Option<bool>,
    pub total_duration_ms: Option<f64>,
    pub total_api_duration_ms: Option<f64>,
    pub total_lines_added: Option<u64>,
    pub total_lines_removed: Option<u64>,
}

impl Cost {
    /// Reported session cost in USD (`total_usd` for agy, `total_cost_usd` for CC).
    pub fn usd(&self) -> Option<f64> {
        self.total_usd
            .or(self.total_cost_usd)
            .filter(|v| v.is_finite() && *v >= 0.0)
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct ContextWindow {
    pub total_input_tokens: Option<u64>,
    pub total_output_tokens: Option<u64>,
    pub context_window_size: Option<u64>,
    pub current_usage: Option<Usage>,
    pub used_percentage: Option<f64>,
    pub remaining_percentage: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
    pub cache_creation_input_tokens: Option<u64>,
    pub cache_read_input_tokens: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Effort {
    pub level: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Thinking {
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct RateLimits {
    pub five_hour: Option<Window>,
    pub seven_day: Option<Window>,
    pub spend_limit: Option<SpendLimit>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Window {
    pub used_percentage: Option<f64>,
    /// Unix epoch seconds.
    pub resets_at: Option<f64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SpendLimit {
    pub used_percentage: Option<f64>,
    pub resets_at: Option<f64>,
    pub used_usd: Option<f64>,
    pub limit_usd: Option<f64>,
    pub period: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct QuotaBucket {
    pub remaining_fraction: Option<f64>,
    pub remaining_amount: Option<f64>,
    pub reset_time: Option<String>,
    pub reset_in_seconds: Option<f64>,
    pub disabled: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuotaKind {
    FiveHour,
    Weekly,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedQuota {
    /// Canonical group name (`"gemini"`, `"3p"`, or `""` for bare/rate_limits).
    pub pool: String,
    pub used_percentage: f64,
    pub resets_at: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Sandbox {
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct StdinSubagent {
    pub name: Option<String>,
    pub role: Option<String>,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Vcs {
    #[serde(rename = "type")]
    pub kind: Option<String>,
    pub branch: Option<String>,
    pub dirty: Option<bool>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PromptCache {
    pub warm: Option<bool>,
    pub caching_observed: Option<bool>,
    pub ttl: Option<String>,
    /// Unix epoch seconds; null when the last response reported no cache tokens.
    pub expires_at: Option<f64>,
    pub requests: Option<u64>,
    pub misses: Option<u64>,
    pub expected_rebuilds: Option<u64>,
    pub hit_ratio: Option<f64>,
    pub cache_write_tokens: Option<u64>,
    pub miss_recache_tokens: Option<u64>,
    pub last_miss_at: Option<f64>,
    pub last_miss_cause: Option<MissCause>,
    pub miss_causes: Option<BTreeMap<String, u64>>,
    pub recache_tokens_if_cold: Option<u64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct MissCause {
    pub causes: Vec<String>,
    pub tools_added: Option<u64>,
    pub tools_removed: Option<u64>,
    pub system_char_delta: Option<i64>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Vim {
    pub mode: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Agent {
    pub name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Remote {
    pub session_id: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct PullRequest {
    pub number: Option<u64>,
    pub url: Option<String>,
    pub review_state: Option<String>,
    /// `mr` for a GitLab merge request; absent for GitHub.
    pub kind: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Worktree {
    pub name: Option<String>,
    pub path: Option<String>,
    pub branch: Option<String>,
    pub original_cwd: Option<String>,
    pub original_branch: Option<String>,
}

/// A list whose malformed entries are dropped one by one.
fn lenient_list<'de, D, T>(d: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned,
{
    let value = serde_json::Value::deserialize(d)?;
    Ok(match value {
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|v| serde_json::from_value(v).ok())
            .collect(),
        _ => Vec::new(),
    })
}

/// The JSON written to the `subagentStatusLine` command: the
/// session and the background tasks (subagents, shells, workflows) listed
/// under the prompt, one line each.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct SubagentInput {
    #[serde(deserialize_with = "lenient")]
    pub session_id: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub cwd: Option<String>,
    /// Columns left for each task's line after the host prefix.
    #[serde(deserialize_with = "lenient")]
    pub columns: Option<usize>,
    #[serde(deserialize_with = "lenient_list")]
    pub tasks: Vec<Task>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct Task {
    pub id: String,
    /// Name the agent was given, if any.
    #[serde(deserialize_with = "lenient")]
    pub name: Option<String>,
    /// `local_agent`, `local_bash`, `local_workflow`, `remote_agent`,
    /// `in_process_teammate`.
    #[serde(rename = "type", deserialize_with = "lenient")]
    pub kind: Option<String>,
    /// The subagent type, e.g. `Explore`.
    #[serde(deserialize_with = "lenient")]
    pub agent_type: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub status: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub description: Option<String>,
    /// What it is doing now.
    #[serde(deserialize_with = "lenient")]
    pub label: Option<String>,
    /// Unix epoch milliseconds.
    #[serde(deserialize_with = "lenient")]
    pub start_time: Option<f64>,
    #[serde(deserialize_with = "lenient")]
    pub model: Option<String>,
    #[serde(deserialize_with = "lenient")]
    pub effort: Option<serde_json::Value>,
    #[serde(deserialize_with = "lenient")]
    pub context_window_size: Option<u64>,
    #[serde(deserialize_with = "lenient")]
    pub token_count: Option<u64>,
    /// Recent token counts, oldest first (up to 16).
    #[serde(deserialize_with = "lenient_list")]
    pub token_samples: Vec<f64>,
    #[serde(deserialize_with = "lenient")]
    pub cwd: Option<String>,
}

impl SubagentInput {
    /// Parse `json` if it is a subagent payload (it has a `tasks` list).
    pub fn detect(json: &str) -> Option<SubagentInput> {
        if !json.contains("\"tasks\"") {
            return None;
        }
        let value: serde_json::Value = serde_json::from_str(json).ok()?;
        value.get("tasks")?.as_array()?;
        serde_json::from_value(value).ok()
    }
}

impl Task {
    /// The task as a status line input, so session metrics (model,
    /// context, effort, dir, git) can describe it.
    pub fn input(&self, parent: &SubagentInput) -> Input {
        let cwd = self.cwd.clone().or_else(|| parent.cwd.clone());
        let effort = match &self.effort {
            Some(serde_json::Value::String(level)) => Some(level.clone()),
            Some(v) => v.get("level").and_then(|l| l.as_str()).map(str::to_string),
            None => None,
        };
        Input {
            session_id: parent.session_id.clone(),
            cwd: cwd.clone(),
            model: self.model.clone().map(|id| Model {
                id: Some(id),
                display_name: None,
                effort: None,
            }),
            workspace: Some(Workspace {
                current_dir: cwd,
                project_dir: parent.cwd.clone(),
                ..Workspace::default()
            }),
            context_window: Some(ContextWindow {
                total_input_tokens: self.token_count,
                context_window_size: self.context_window_size,
                ..ContextWindow::default()
            }),
            effort: effort.map(|level| Effort { level: Some(level) }),
            ..Input::default()
        }
    }
}

/// Largest stdin payload accepted; real payloads are a few KiB.
pub const MAX_INPUT_BYTES: usize = 1024 * 1024;

fn parse_bucket_reset(bucket: &QuotaBucket, now: i64) -> Option<i64> {
    if let Some(raw) = bucket.reset_time.as_deref().map(str::trim)
        && !raw.is_empty()
        && let Ok(dt) = DateTime::parse_from_rfc3339(raw)
    {
        let ts = dt.timestamp();
        // `1970-01-01T00:00:00Z` (epoch 0) means no active timer, not expired.
        return (ts > 0).then_some(ts);
    }
    bucket
        .reset_in_seconds
        .filter(|s| s.is_finite() && *s > 0.0 && *s <= (86_400.0 * 36_500.0))
        .map(|s| now.saturating_add(s.round() as i64))
}

fn bucket_to_resolved(pool: &str, bucket: &QuotaBucket, now: i64) -> Option<ResolvedQuota> {
    if bucket.disabled == Some(true) {
        return None;
    }
    let frac = bucket.remaining_fraction.filter(|f| f.is_finite())?;
    let used_percentage = (1.0 - frac.clamp(0.0, 1.0)) * 100.0;
    Some(ResolvedQuota {
        pool: pool.to_string(),
        used_percentage,
        resets_at: parse_bucket_reset(bucket, now),
    })
}

fn match_pool_bucket<'a>(
    quota: &'a BTreeMap<String, Option<QuotaBucket>>,
    pool: &str,
    kind: QuotaKind,
) -> Option<(&'static str, &'a QuotaBucket)> {
    let candidates: &[&str] = match (pool, kind) {
        ("gemini", QuotaKind::FiveHour) => &["gemini-5h", "gemini-five-hour", "gemini_5h"],
        ("gemini", QuotaKind::Weekly) => &["gemini-weekly", "gemini-7d", "gemini-seven-day"],
        ("3p", QuotaKind::FiveHour) => &["3p-5h", "3p-five-hour", "3p_5h"],
        ("3p", QuotaKind::Weekly) => &["3p-weekly", "3p-7d", "3p-seven-day"],
        ("bare", QuotaKind::FiveHour) => &["5h", "five_hour", "five-hour", "session"],
        ("bare", QuotaKind::Weekly) => &["weekly", "7d", "seven_day", "seven-day"],
        _ => &[],
    };
    let label = if pool == "bare" {
        ""
    } else if pool == "gemini" {
        "gemini"
    } else {
        "3p"
    };
    for want in candidates {
        for (k, v) in quota {
            if k.eq_ignore_ascii_case(want)
                && let Some(bucket) = v.as_ref()
                && bucket.disabled != Some(true)
                && bucket.remaining_fraction.is_some_and(f64::is_finite)
            {
                return Some((label, bucket));
            }
        }
    }
    None
}

fn prefers_gemini_pool(input: &Input) -> bool {
    let model = input.model.as_ref();
    let id = model.and_then(|m| m.id.as_deref()).unwrap_or("");
    let disp = model.and_then(|m| m.display_name.as_deref()).unwrap_or("");
    if id.is_empty() && disp.is_empty() {
        return true;
    }
    let combined = format!("{id} {disp}").to_lowercase();
    combined.contains("gemini") || combined.contains("orchestrator")
}

impl Input {
    pub fn parse(json: &str) -> serde_json::Result<Input> {
        serde_json::from_str(json)
    }

    pub fn cc_version(&self) -> Option<Version> {
        self.version.as_deref().and_then(Version::parse)
    }

    /// Conversation or session id (`conversation_id` preferred for agy).
    pub fn conv_id(&self) -> Option<&str> {
        self.conversation_id
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| self.session_id.as_deref().filter(|s| !s.trim().is_empty()))
    }

    /// Session title (`conversation_title` preferred for agy).
    pub fn title(&self) -> Option<&str> {
        self.conversation_title
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .or_else(|| {
                self.session_name
                    .as_deref()
                    .filter(|s| !s.trim().is_empty())
            })
    }

    /// Resolve a quota window from `quota` (Antigravity CLI) or `rate_limits` (Claude Code).
    /// `pool` is `"auto"`, `"gemini"`, `"3p"`, or `"bare"`.
    pub fn quota_window(&self, pool: &str, kind: QuotaKind, now: i64) -> Option<ResolvedQuota> {
        let pool_norm = pool.trim().to_ascii_lowercase();
        if let Some(quota) = self.quota.as_ref() {
            let order: &[&str] = match pool_norm.as_str() {
                "gemini" => &["gemini"],
                "3p" => &["3p"],
                "bare" => &["bare"],
                _ => {
                    if prefers_gemini_pool(self) {
                        &["gemini", "bare", "3p"]
                    } else {
                        &["3p", "bare", "gemini"]
                    }
                }
            };
            for candidate in order {
                if let Some((label, bucket)) = match_pool_bucket(quota, candidate, kind)
                    && let Some(resolved) = bucket_to_resolved(label, bucket, now)
                {
                    return Some(resolved);
                }
            }
            // Also accept single-pool custom names in `auto` mode (e.g. `session-5h`, `team-weekly`).
            if pool_norm == "auto" {
                for (k, v) in quota {
                    let kl = k.to_ascii_lowercase();
                    let matches_kind = match kind {
                        QuotaKind::FiveHour => {
                            kl.contains("5h") || kl.contains("5-hour") || kl.contains("session")
                        }
                        QuotaKind::Weekly => {
                            kl.contains("weekly") || kl.contains("7d") || kl.contains("week")
                        }
                    };
                    if matches_kind
                        && let Some(bucket) = v.as_ref()
                        && let Some(resolved) = bucket_to_resolved("", bucket, now)
                    {
                        return Some(resolved);
                    }
                }
            }
            return None;
        }
        if matches!(pool_norm.as_str(), "auto" | "bare" | "")
            && let Some(rl) = self.rate_limits.as_ref()
        {
            let w = match kind {
                QuotaKind::FiveHour => rl.five_hour.as_ref()?,
                QuotaKind::Weekly => rl.seven_day.as_ref()?,
            };
            let used_percentage = w.used_percentage.filter(|p| p.is_finite())?;
            let resets_at = w
                .resets_at
                .filter(|t| t.is_finite() && *t > 0.0)
                .map(|t| t as i64);
            return Some(ResolvedQuota {
                pool: String::new(),
                used_percentage,
                resets_at,
            });
        }
        None
    }

    /// The working directory, preferring `workspace.current_dir`.
    pub fn current_dir(&self) -> Option<&str> {
        let non_empty = |d: &&str| !d.is_empty();
        self.workspace
            .as_ref()
            .and_then(|w| w.current_dir.as_deref())
            .filter(non_empty)
            .or(self.cwd.as_deref().filter(non_empty))
    }

    pub fn model_id(&self) -> Option<&str> {
        self.model
            .as_ref()
            .and_then(|m| m.id.as_deref().or(m.display_name.as_deref()))
    }

    /// The project root for project-scoped configuration, falling back to cwd.
    pub fn project_dir(&self) -> Option<&str> {
        self.workspace
            .as_ref()
            .and_then(|w| w.project_dir.as_deref())
            .filter(|d| !d.is_empty())
            .or(self.cwd.as_deref().filter(|d| !d.is_empty()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAPTURED: &str = r#"{
        "session_id": "s1", "transcript_path": "/t.jsonl", "cwd": "/w/app", "prompt_id": "p1",
        "model": {"id": "claude-opus-5-5[1m]", "display_name": "Opus 5.5 (1M context)"},
        "workspace": {"current_dir": "/w/app", "project_dir": "/w/app", "added_dirs": [],
            "repo": {"host": "github.com", "owner": "acme", "name": "app"}},
        "version": "2.1.285", "output_style": {"name": "default"},
        "cost": {"total_cost_usd": 7.74, "total_duration_ms": 61000, "total_api_duration_ms": 9000,
            "total_lines_added": 12, "total_lines_removed": 3},
        "context_window": {"total_input_tokens": 270000, "total_output_tokens": 900,
            "context_window_size": 1000000, "used_percentage": 27, "remaining_percentage": 73,
            "current_usage": {"input_tokens": 3, "output_tokens": 900,
                "cache_creation_input_tokens": 1000, "cache_read_input_tokens": 268997}},
        "exceeds_200k_tokens": true, "effort": {"level": "xhigh"}, "thinking": {"enabled": true},
        "fast_mode": false,
        "rate_limits": {"five_hour": {"used_percentage": 4, "resets_at": 1790820000},
            "seven_day": {"used_percentage": 21.5, "resets_at": 1791200000}},
        "prompt_cache": {"warm": true, "caching_observed": true, "ttl": "1h", "expires_at": 1790812000,
            "requests": 40, "misses": 0, "expected_rebuilds": 1, "hit_ratio": 0.97,
            "cache_write_tokens": 90000, "miss_recache_tokens": 0, "last_miss_at": null,
            "last_miss_cause": null, "miss_causes": {}, "recache_tokens_if_cold": 270000}
    }"#;

    #[test]
    fn parses_captured_payload() {
        let input = Input::parse(CAPTURED).unwrap();
        assert_eq!(input.cc_version(), Some(Version::new(2, 1, 285)));
        assert_eq!(input.model_id(), Some("claude-opus-5-5[1m]"));
        assert_eq!(input.current_dir(), Some("/w/app"));
        let cw = input.context_window.unwrap();
        assert_eq!(cw.context_window_size, Some(1_000_000));
        assert_eq!(
            cw.current_usage.unwrap().cache_read_input_tokens,
            Some(268_997)
        );
        let rl = input.rate_limits.unwrap();
        assert_eq!(rl.seven_day.unwrap().used_percentage, Some(21.5));
        let pc = input.prompt_cache.unwrap();
        assert_eq!(pc.expires_at, Some(1_790_812_000.0));
        assert!(pc.last_miss_cause.is_none());
    }

    #[test]
    fn parses_antigravity_four_pool_quota_with_epoch_zero_resets() {
        let json = r#"{
            "conversation_id": "c1",
            "conversation_title": "Test Agy",
            "version": "1.5.17",
            "terminal_width": 131,
            "model": {"id": "Orchestrator (Neon)", "display_name": "Orchestrator (Neon)"},
            "agent_state": "working",
            "cycle_mode": "auto",
            "plan_tier": "Google AI Ultra",
            "email": "user@example.com",
            "quota": {
                "3p-5h": {"remaining_fraction": 0.967, "reset_time": "1970-01-01T00:00:00Z", "reset_in_seconds": 0},
                "3p-weekly": {"remaining_fraction": 0.614, "reset_time": "1970-01-01T00:00:00Z", "reset_in_seconds": 0},
                "gemini-5h": {"remaining_fraction": 0.974, "reset_time": "2026-11-26T12:04:55Z", "reset_in_seconds": 16855},
                "gemini-weekly": {"remaining_fraction": 0.993, "reset_time": "2026-12-03T07:04:55Z", "reset_in_seconds": 603655}
            }
        }"#;
        let input = Input::parse(json).unwrap();
        assert_eq!(input.conv_id(), Some("c1"));
        assert_eq!(input.title(), Some("Test Agy"));
        let g5 = input
            .quota_window("auto", QuotaKind::FiveHour, 1_795_677_839)
            .unwrap();
        assert_eq!(g5.pool, "gemini");
        assert!(g5.resets_at.is_some());
        let tp_w = input
            .quota_window("3p", QuotaKind::Weekly, 1_795_677_839)
            .unwrap();
        assert_eq!(tp_w.pool, "3p");
        assert_eq!(tp_w.resets_at, None);
        assert!((tp_w.used_percentage - 38.6).abs() < 0.01);
    }

    #[test]
    fn drops_only_malformed_fields() {
        let input = Input::parse(
            r#"{"model": "not-an-object", "cost": {"total_cost_usd": 1.5}, "pr": {"number": "x"}}"#,
        )
        .unwrap();
        assert!(input.model.is_none());
        assert!(input.pr.is_none());
        assert_eq!(input.cost.unwrap().total_cost_usd, Some(1.5));
    }

    #[test]
    fn accepts_empty_and_unknown_fields() {
        let input = Input::parse(r#"{"future_field": {"a": 1}}"#).unwrap();
        assert!(input.model.is_none());
        assert!(input.current_dir().is_none());
    }

    #[test]
    fn reads_subagent_payloads() {
        let json = r#"{"session_id": "s1", "cwd": "/w/app", "columns": 120, "tasks": [
            {"id": "a1", "type": "local_agent", "status": "running", "label": "Reading files",
             "startTime": 1790812000000, "model": "claude-opus-5-5[1m]", "effort": "high",
             "contextWindowSize": 1000000, "tokenCount": 211265, "tokenSamples": [206927, 211265],
             "cwd": "/w/app"},
            {"id": "bad", "tokenCount": "many"},
            {"id": "b1", "type": "local_bash", "status": "completed"}
        ]}"#;
        let sub = SubagentInput::detect(json).unwrap();
        assert_eq!(sub.columns, Some(120));
        let ids: Vec<&str> = sub.tasks.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["a1", "bad", "b1"]);
        assert_eq!(sub.tasks[1].token_count, None);
        let input = sub.tasks[0].input(&sub);
        assert_eq!(input.model_id(), Some("claude-opus-5-5[1m]"));
        assert_eq!(input.current_dir(), Some("/w/app"));
        assert_eq!(input.effort.unwrap().level.as_deref(), Some("high"));
        assert!(SubagentInput::detect(CAPTURED).is_none());
    }

    #[test]
    fn falls_back_to_cwd() {
        let input = Input::parse(r#"{"cwd": "/a", "workspace": {"current_dir": ""}}"#).unwrap();
        assert_eq!(input.current_dir(), Some("/a"));
    }
}
