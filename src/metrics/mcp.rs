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
    approval: Approval,
}

#[derive(Default, Deserialize, Serialize)]
struct Project {
    servers: Vec<String>,
    approval: Approval,
    disabled_mcp: Vec<String>,
}

#[derive(Clone, Default, Deserialize, Serialize)]
struct Approval {
    enabled: Vec<String>,
    disabled: Vec<String>,
    all: bool,
}

impl Approval {
    fn parse(value: &Value) -> Self {
        Self {
            enabled: strings(&value["enabledMcpjsonServers"]),
            disabled: strings(&value["disabledMcpjsonServers"]),
            all: value["enableAllProjectMcpServers"]
                .as_bool()
                .unwrap_or(false),
        }
    }

    fn merge(&mut self, other: Self) {
        self.enabled.extend(other.enabled);
        self.disabled.extend(other.disabled);
        self.all |= other.all;
    }

    fn load(
        project: Option<&Project>,
        settings: Option<PathBuf>,
        dir: Option<&Path>,
        cache: &Path,
    ) -> Self {
        let mut approval = project.map(|p| p.approval.clone()).unwrap_or_default();
        let paths = settings.into_iter().chain(dir.into_iter().flat_map(|d| {
            [
                d.join(".claude/settings.json"),
                d.join(".claude/settings.local.json"),
            ]
        }));
        for path in paths {
            if let Some(settings) = load(&path, cache) {
                approval.merge(settings.approval);
            }
        }
        approval
    }
}

fn project_key(path: &str) -> String {
    let path = path.replace('\\', "/");
    let mut key = path.trim_end_matches('/').to_owned();
    if key.is_empty() && path.starts_with('/') {
        key.push('/');
    }
    if key.as_bytes().get(1) == Some(&b':') && key.as_bytes()[0].is_ascii_alphabetic() {
        key.replace_range(..1, &key[..1].to_ascii_lowercase());
        if key.len() == 2 && path.ends_with('/') {
            key.push('/');
        }
    }
    key
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
                project_key(dir),
                Project {
                    servers: names(&p["mcpServers"]),
                    approval: Approval::parse(p),
                    disabled_mcp: strings(&p["disabledMcpServers"]),
                },
            )
        })
        .collect();
    Some(Config {
        servers: names(&value["mcpServers"]),
        projects,
        approval: Approval::parse(value),
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
        &crate::state::dir().join("mcp-v2"),
    )
}

fn servers_at(config: &Path, dir: Option<&Path>, cache: &Path) -> Option<Vec<String>> {
    let settings = config.parent().map(|p| p.join("settings.json"));
    let config = load(config, cache).unwrap_or_default();
    let project = dir.and_then(|d| config.projects.get(&project_key(d.to_str()?)));
    let mut enabled: BTreeSet<_> = config.servers.into_iter().collect();
    let approval = Approval::load(project, settings, dir, cache);
    if let Some(project) = project {
        enabled.extend(project.servers.iter().cloned());
    }
    if let Some(local) = dir.and_then(|d| load(&d.join(".mcp.json"), cache)) {
        enabled.extend(local.servers.into_iter().filter(|s| {
            (approval.all || approval.enabled.contains(s)) && !approval.disabled.contains(s)
        }));
    }
    if let Some(project) = project {
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
    fn project_settings_approve_mcp_json_without_a_project_entry() {
        let f = Fixture::new("settings");
        std::fs::create_dir_all(f.0.join("a/.claude")).unwrap();
        f.write("a/.mcp.json", json!({"mcpServers":{"idea":{}}}));
        f.write(
            "a/.claude/settings.local.json",
            json!({
                "enabledMcpjsonServers":["idea"], "enableAllProjectMcpServers":true
            }),
        );
        assert_eq!(f.servers("a"), Some(vec!["idea".into()]));
    }

    #[test]
    fn normalizes_project_separators_trailing_slashes_and_drive_letters() {
        let f = Fixture::new("paths");
        f.write(
            ".claude.json",
            json!({"projects":{
                "D:/Tellhow/Projects/Idea/gxdd/jiaohe/": {"mcpServers":{"idea":{}}}
            }}),
        );
        assert_eq!(
            servers_at(
                &f.0.join(".claude.json"),
                Some(Path::new(r"d:\Tellhow\Projects\Idea\gxdd\jiaohe\")),
                &f.0.join("cache")
            ),
            Some(vec!["idea".into()])
        );
        assert_eq!(project_key("/"), "/");
        assert_eq!(project_key(r"C:\"), "c:/");
        assert_eq!(project_key("/Project/"), "/Project");
        assert_ne!(project_key("/Project"), project_key("/project"));
    }

    #[test]
    fn merges_all_approval_sources_and_any_disable_wins() {
        let f = Fixture::new("approvals");
        std::fs::create_dir_all(f.0.join("a/.claude")).unwrap();
        let key = format!("{}\\", f.0.join("a").to_string_lossy().replace('/', "\\"));
        f.write(
            ".claude.json",
            json!({"projects":{key:{
                "enabledMcpjsonServers":["entry", "blocked-settings", "blocked-local"],
                "disabledMcpjsonServers":["blocked-entry"], "disabledMcpServers":["off"]
            }}}),
        );
        f.write(
            "settings.json",
            json!({"enabledMcpjsonServers":["user", "blocked-entry"],
            "disabledMcpjsonServers":["blocked-settings"]}),
        );
        f.write(
            "a/.claude/settings.json",
            json!({"enabledMcpjsonServers":["shared"],
            "disabledMcpjsonServers":["blocked-shared"]}),
        );
        f.write(
            "a/.claude/settings.local.json",
            json!({"enabledMcpjsonServers":["local", "off", "blocked-shared"],
            "disabledMcpjsonServers":["blocked-local"]}),
        );
        f.write(
            "a/.mcp.json",
            json!({"mcpServers":{
                "entry":{}, "user":{}, "shared":{}, "local":{}, "unapproved":{}, "off":{},
                "blocked-entry":{}, "blocked-settings":{}, "blocked-shared":{}, "blocked-local":{}
            }}),
        );
        assert_eq!(
            f.servers("a").unwrap(),
            ["entry", "local", "shared", "user"]
        );
        for source in [
            "settings.json",
            "a/.claude/settings.json",
            "a/.claude/settings.local.json",
        ] {
            f.write(
                source,
                json!({"enableAllProjectMcpServers":true, "disabledMcpjsonServers":["unapproved"]}),
            );
            let servers = f.servers("a").unwrap();
            assert!(servers.contains(&"shared".into()));
            assert!(!servers.contains(&"unapproved".into()));
            assert!(!servers.contains(&"off".into()));
            assert!(!servers.contains(&"blocked-entry".into()));
            f.write(source, json!({}));
        }
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
