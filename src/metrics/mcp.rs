//! Enabled servers from Claude's user and project MCP configuration.
//! Plugin-provided MCP servers are outside this metric's scope.
//! Each configuration read is capped at 8 MiB; caches store names and flags only.

use super::{Ctx, Metric, MetricMeta, Options, Reading};
use crate::input::Input;
use crate::state::transcript::cached;
use crate::style::{Category, IconKey};
use crate::widgets::WidgetKind;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::path::{Path, PathBuf};

const CONFIG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Default, Deserialize, Serialize)]
struct Config {
    servers: Vec<String>,
    projects: BTreeMap<String, Project>,
}

#[derive(Default, Deserialize, Serialize)]
struct Project {
    servers: Vec<String>,
    enabled: Vec<String>,
    disabled: Vec<String>,
    disabled_mcp: Vec<String>,
    all: bool,
}

fn names(value: &Value) -> Vec<String> {
    value
        .as_object()
        .map(|o| o.keys().cloned().collect())
        .unwrap_or_default()
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn parse(value: &Value) -> Option<Config> {
    value.as_object()?;
    let projects = value["projects"]
        .as_object()
        .into_iter()
        .flatten()
        .map(|(dir, p)| {
            (
                dir.clone(),
                Project {
                    servers: names(&p["mcpServers"]),
                    enabled: strings(&p["enabledMcpjsonServers"]),
                    disabled: strings(&p["disabledMcpjsonServers"]),
                    disabled_mcp: strings(&p["disabledMcpServers"]),
                    all: p["enableAllProjectMcpServers"].as_bool().unwrap_or(false),
                },
            )
        })
        .collect();
    Some(Config {
        servers: names(&value["mcpServers"]),
        projects,
    })
}

fn load(path: &Path, cache_dir: &Path) -> Option<Config> {
    cached(path, cache_dir, |file, len| {
        if len > CONFIG_BYTES {
            return None;
        }
        let mut bytes = Vec::new();
        file.take(CONFIG_BYTES + 1).read_to_end(&mut bytes).ok()?;
        if bytes.len() as u64 > CONFIG_BYTES {
            return None;
        }
        parse(&serde_json::from_slice::<Value>(&bytes).ok()?)
    })
}

fn config_path(config_dir: Option<std::ffi::OsString>, home: Option<PathBuf>) -> Option<PathBuf> {
    let root = config_dir
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .or(home)?;
    Some(root.join(".claude.json"))
}

pub(super) fn servers(input: &Input) -> Option<Vec<String>> {
    servers_at(
        &config_path(std::env::var_os("CLAUDE_CONFIG_DIR"), dirs::home_dir())?,
        input.project_dir().map(Path::new),
        &crate::state::dir().join("mcp-v1"),
    )
}

fn servers_at(config: &Path, dir: Option<&Path>, cache: &Path) -> Option<Vec<String>> {
    let config = load(config, cache).unwrap_or_default();
    let project = dir.and_then(|d| config.projects.get(d.to_str()?));
    let mut enabled: BTreeSet<_> = config.servers.iter().cloned().collect();
    if let Some(project) = project {
        enabled.extend(project.servers.iter().cloned());
        if let Some(local) = dir.and_then(|d| load(&d.join(".mcp.json"), cache)) {
            enabled.extend(local.servers.into_iter().filter(|s| {
                (project.all || project.enabled.contains(s)) && !project.disabled.contains(s)
            }));
        }
        enabled.retain(|s| !project.disabled_mcp.contains(s));
    }
    (!enabled.is_empty()).then(|| enabled.into_iter().collect())
}

pub(super) fn metric() -> Metric {
    Metric {
        meta: MetricMeta {
            id: "mcp",
            category: Category::Activity,
            name: "MCP servers",
            summary: "Enabled MCP servers from user and project configuration",
            icon: IconKey::Mcp,
            widget: WidgetKind::Text,
            templates: ["{names}", "{names}", "{count}"],
            fields: &["names", "count"],
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
    let names: BTreeSet<_> = ctx.mcp()?.iter().collect();
    (!names.is_empty()).then(|| {
        Reading::default()
            .set("count", names.len().to_string())
            .set(
                "names",
                names.into_iter().cloned().collect::<Vec<_>>().join(", "),
            )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new(name: &str) -> Self {
            let path =
                std::env::temp_dir().join(format!("ccline-mcp-{}-{name}", std::process::id()));
            std::fs::create_dir_all(path.join("a")).unwrap();
            std::fs::create_dir_all(path.join("b")).unwrap();
            Self(path)
        }

        fn write(&self, name: &str, value: Value) {
            std::fs::write(self.0.join(name), value.to_string()).unwrap();
        }

        fn servers(&self, project: &str) -> Option<Vec<String>> {
            servers_at(
                &self.0.join(".claude.json"),
                Some(&self.0.join(project)),
                &self.0.join("cache"),
            )
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn merges_sources_with_project_specific_opt_in_and_disable_precedence() {
        let f = Fixture::new("merge");
        let a = f.0.join("a").to_string_lossy().into_owned();
        let b = f.0.join("b").to_string_lossy().into_owned();
        f.write(".claude.json", json!({"mcpServers":{"global":{}, "off":{}}, "projects":{
            a: {"mcpServers":{"project":{}, "off":{}}, "enabledMcpjsonServers":["local", "blocked", "off"],
                "disabledMcpjsonServers":["blocked"], "disabledMcpServers":["off"]},
            b: {"mcpServers":{"other":{}}, "enableAllProjectMcpServers":true, "disabledMcpjsonServers":["blocked"]}
        }}));
        for project in ["a", "b"] {
            f.write(
                &format!("{project}/.mcp.json"),
                json!({"mcpServers":{"local":{}, "blocked":{}, "unlisted":{}, "off":{}}}),
            );
        }
        assert_eq!(f.servers("a").unwrap(), ["global", "local", "project"]);
        assert_eq!(
            f.servers("b").unwrap(),
            ["global", "local", "off", "other", "unlisted"]
        );
        f.write("a/.mcp.json", json!({"mcpServers":{"unlisted":{}}}));
        assert_eq!(f.servers("a").unwrap(), ["global", "project"]);
    }

    #[test]
    fn absent_unapproved_malformed_and_oversized_sources_yield_no_servers() {
        let f = Fixture::new("missing");
        assert!(f.servers("a").is_none());
        f.write("a/.mcp.json", json!({"mcpServers":{"unapproved":{}}}));
        assert!(f.servers("a").is_none());
        std::fs::write(f.0.join(".claude.json"), "malformed").unwrap();
        assert!(f.servers("a").is_none());
        std::fs::File::create(f.0.join(".claude.json"))
            .unwrap()
            .set_len(CONFIG_BYTES + 1)
            .unwrap();
        assert!(f.servers("a").is_none());
    }

    #[test]
    fn caches_names_without_server_commands_and_refreshes_config() {
        let f = Fixture::new("cache");
        f.write(
            ".claude.json",
            json!({"mcpServers":{"one":{"command":"DO_NOT_CACHE_COMMAND"}}}),
        );
        assert_eq!(f.servers("a").unwrap(), ["one"]);
        for entry in std::fs::read_dir(f.0.join("cache")).unwrap() {
            let text = std::fs::read_to_string(entry.unwrap().path()).unwrap();
            assert!(!text.contains("DO_NOT_CACHE_COMMAND"));
        }
        f.write(".claude.json", json!({"mcpServers":{"second":{}}}));
        assert_eq!(f.servers("a").unwrap(), ["second"]);
    }

    #[test]
    fn project_root_prefers_workspace_project_dir_then_cwd() {
        let input = Input::parse(
            r#"{"cwd":"/cwd", "workspace":{"project_dir":"/project", "current_dir":"/nested"}}"#,
        )
        .unwrap();
        assert_eq!(input.project_dir(), Some("/project"));
        let input =
            Input::parse(r#"{"cwd":"/cwd", "workspace":{"current_dir":"/nested"}}"#).unwrap();
        assert_eq!(input.project_dir(), Some("/cwd"));
    }

    #[test]
    fn config_lives_in_home_unless_config_dir_is_set() {
        let home = PathBuf::from("home");
        assert_eq!(
            config_path(None, Some(home.clone())),
            Some(home.join(".claude.json"))
        );
        assert_eq!(
            config_path(Some("custom".into()), Some(home)),
            Some(PathBuf::from("custom").join(".claude.json"))
        );
    }
}
