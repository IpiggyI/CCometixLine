//! Bounded, stamp-cached environment names; source files are never modified.

use super::{Ctx, Metric, MetricMeta, Options, Reading};
use crate::state::transcript::{cached, clean, command_file, short_event};
use crate::style::{Category, IconKey};
use crate::widgets::WidgetKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use std::io::Read;
use std::path::Path;

const LIMIT: usize = 128;
const CONFIG_BYTES: u64 = 1024 * 1024;
const TEMPLATE: &str =
    "[CLAUDE.md: {claude_md}][ | rules: {rules}][ | MCPs: {mcp}][ | hooks: {hooks}]";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct Environment {
    pub claude_md: Vec<String>,
    pub rules: Vec<String>,
    pub hooks: Vec<String>,
}

pub(super) fn metric() -> Metric {
    Metric {
        meta: MetricMeta {
            id: "env",
            category: Category::Activity,
            name: "Environment",
            summary: "Claude instructions, rules, MCP servers and configured hooks",
            icon: IconKey::Repo,
            widget: WidgetKind::Text,
            templates: [TEMPLATE, TEMPLATE, "{claude_md}"],
            fields: &["claude_md", "rules", "mcp", "hooks"],
            levels: &[],
            min_cc: None,
            needs_mod: false,
            options: &[],
            stateful: true,
            priority: 2,
        },
        compute,
    }
}

fn compute(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let env = ctx.environment();
    let mcp = ctx
        .mcp()
        .map(|names| {
            names
                .iter()
                .take(LIMIT)
                .map(|s| clean(s, 128))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let reading = Reading::default()
        .set("claude_md", env.claude_md.join(", "))
        .set("rules", env.rules.join(", "))
        .set("mcp", mcp)
        .set("hooks", env.hooks.join(", "));
    (!reading.fields.is_empty()).then_some(reading)
}

pub(super) fn collect(project: Option<&Path>) -> Environment {
    let home = dirs::home_dir();
    let config = crate::paths::claude_dir();
    collect_at(
        Some(&config),
        project,
        home.as_deref(),
        &crate::state::dir().join("env-v1"),
    )
}

fn display(path: &Path, project: Option<&Path>, home: Option<&Path>) -> String {
    if let Some(relative) = project.and_then(|root| path.strip_prefix(root).ok()) {
        clean(&relative.to_string_lossy(), 256)
    } else if let Some(relative) = home.and_then(|root| path.strip_prefix(root).ok()) {
        format!("~/{}", clean(&relative.to_string_lossy(), 256))
    } else {
        clean(&path.to_string_lossy(), 256)
    }
}

fn collect_at(
    config: Option<&Path>,
    project: Option<&Path>,
    home: Option<&Path>,
    cache: &Path,
) -> Environment {
    let mut env = Environment::default();
    let paths =
        config
            .map(|d| d.join("CLAUDE.md"))
            .into_iter()
            .chain(project.into_iter().flat_map(|d| {
                [
                    d.join("CLAUDE.md"),
                    d.join(".claude/CLAUDE.md"),
                    d.join("CLAUDE.local.md"),
                ]
            }));
    for path in paths.filter(|p| p.is_file()) {
        let name = display(&path, project, home);
        if !env.claude_md.contains(&name) {
            env.claude_md.push(name);
        }
    }
    for dir in project
        .map(|d| d.join(".claude/rules"))
        .into_iter()
        .chain(config.map(|d| d.join("rules")))
    {
        env.rules.extend(rule_names(&dir, cache));
    }
    env.rules.sort();
    env.rules.dedup();
    env.rules.truncate(LIMIT);
    let settings =
        config
            .map(|d| d.join("settings.json"))
            .into_iter()
            .chain(project.into_iter().flat_map(|d| {
                [
                    d.join(".claude/settings.json"),
                    d.join(".claude/settings.local.json"),
                ]
            }));
    for path in settings {
        env.hooks.extend(hooks(&path, cache));
    }
    env.hooks.sort();
    env.hooks.dedup();
    env.hooks.truncate(LIMIT);
    env
}

#[derive(Deserialize, Serialize)]
struct Rules {
    modified: std::time::SystemTime,
    names: Vec<String>,
}

fn rule_names(dir: &Path, cache: &Path) -> Vec<String> {
    let Ok(meta) = dir.metadata() else {
        return Vec::new();
    };
    let Ok(modified) = meta.modified() else {
        return Vec::new();
    };
    let target = cache.join(format!(
        "rules-{}.json",
        crate::state::file_key(&dir.to_string_lossy())
    ));
    if let Some(saved) = crate::state::transcript::bounded(&target, CONFIG_BYTES)
        .and_then(|b| serde_json::from_slice::<Rules>(&b).ok())
        && saved.modified == modified
    {
        return saved.names;
    }
    let names = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .take(LIMIT * 4)
        .filter_map(Result::ok)
        .filter(|e| {
            e.path().extension().is_some_and(|e| e == "md")
                && e.file_type().is_ok_and(|t| t.is_file())
        })
        .filter_map(|e| {
            e.path()
                .file_stem()
                .map(|s| clean(&s.to_string_lossy(), 128))
        })
        .take(LIMIT)
        .collect();
    let saved = Rules { modified, names };
    let _ = crate::state::write_json(&target, &saved);
    saved.names
}

fn hooks(path: &Path, cache: &Path) -> Vec<String> {
    cached(path, cache, |file, len| {
        if len > CONFIG_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(CONFIG_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > CONFIG_BYTES {
            return None;
        }
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        Some(hook_names(&value))
    })
    .unwrap_or_default()
}

fn hook_names(value: &Value) -> Vec<String> {
    let mut names = Vec::new();
    for (event, groups) in value["hooks"].as_object().into_iter().flatten().take(LIMIT) {
        for hook in groups
            .as_array()
            .into_iter()
            .flatten()
            .take(LIMIT)
            .flat_map(|group| group["hooks"].as_array().into_iter().flatten().take(LIMIT))
        {
            if let Some(command) = hook["command"].as_str() {
                let name = command_file(command);
                if !name.is_empty() {
                    names.push(format!("{}: {name}", short_event(event)));
                }
                if names.len() == LIMIT {
                    return names;
                }
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn collects_names_and_invalidates_file_and_directory_stamps() {
        let root = std::env::temp_dir().join(format!("ccline-env-{}", std::process::id()));
        let home = root.join("home");
        let config = home.join(".claude");
        let project = home.join("app");
        let cache = root.join("cache");
        for dir in [config.join("rules"), project.join(".claude/rules")] {
            std::fs::create_dir_all(dir).unwrap();
        }
        for path in [
            config.join("CLAUDE.md"),
            project.join("CLAUDE.md"),
            project.join(".claude/CLAUDE.md"),
            project.join("CLAUDE.local.md"),
            config.join("rules/review.md"),
            project.join(".claude/rules/style.md"),
        ] {
            std::fs::write(path, "read only").unwrap();
        }
        std::fs::write(
            config.join("settings.json"),
            json!({"hooks":{"PostToolUse":[{"hooks":[{"command":"/bin/format.sh"}]}]}}).to_string(),
        )
        .unwrap();
        std::fs::write(
            project.join(".claude/settings.local.json"),
            json!({"hooks":{"PreToolUse":[{"hooks":[{"command":"/bin/check.py"}]}]}}).to_string(),
        )
        .unwrap();
        let read = || collect_at(Some(&config), Some(&project), Some(&home), &cache);
        let env = read();
        assert_eq!(
            env.claude_md,
            [
                "~/.claude/CLAUDE.md",
                "CLAUDE.md",
                ".claude/CLAUDE.md",
                "CLAUDE.local.md"
            ]
        );
        assert_eq!(env.rules, ["review", "style"]);
        assert_eq!(env.hooks, ["Post: format.sh", "Pre: check.py"]);
        std::fs::write(config.join("settings.json"), "{}").unwrap();
        std::fs::remove_file(project.join(".claude/rules/style.md")).unwrap();
        assert_eq!(read().hooks, ["Pre: check.py"]);
        assert_eq!(read().rules, ["review"]);
        assert_eq!(
            std::fs::read_to_string(project.join("CLAUDE.md")).unwrap(),
            "read only"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
