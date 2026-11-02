//! Working directory, repository, branch, pull request and line changes.

use super::{Ctx, Metric, MetricMeta, OptKind, OptSpec, Options, Reading};
use crate::style::{Category, GlyphTier, IconKey, Role, Severity};
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
                priority: 4,
                options: &[
                    OptSpec {
                        key: "style",
                        kind: OptKind::Choice(&[
                            "name", "parent", "tilde", "short", "full", "depth",
                        ]),
                        default: "name",
                        summary: "How much of the path to show",
                    },
                    OptSpec {
                        key: "depth",
                        kind: OptKind::Number,
                        default: "3",
                        summary: "Last path components for depth style, at least 1",
                    },
                ],
                ..meta(
                    "dir",
                    Category::Workspace,
                    "Directory",
                    "Working directory",
                    IconKey::Dir,
                    ["{path}", "{path}", "{name}"],
                    &["path", "name"],
                )
            },
            compute: dir,
        },
        Metric {
            meta: MetricMeta {
                priority: 4,
                options: &[
                    OptSpec {
                        key: "sha",
                        kind: OptKind::Flag,
                        default: "false",
                        summary: "Add the short commit id",
                    },
                    OptSpec {
                        key: "branch_prefixes",
                        kind: OptKind::Text,
                        default: "",
                        summary: "Comma-separated prefix=replacement pairs for the displayed branch",
                    },
                ],
                ..meta(
                    "git",
                    Category::Vcs,
                    "Git",
                    "Branch, status and ahead/behind",
                    IconKey::Git,
                    [
                        "{branch}[ {status}][ {ahead}][ {behind}][ {sha}]",
                        "{branch}[{dirty}]",
                        "{branch}",
                    ],
                    &[
                        "branch",
                        "status",
                        "dirty",
                        "ahead",
                        "behind",
                        "sha",
                        "staged",
                        "modified",
                        "untracked",
                        "conflicts",
                    ],
                )
            },
            compute: git,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "repo",
                    Category::Vcs,
                    "Repository",
                    "owner/name from the origin remote",
                    IconKey::Repo,
                    ["{full}", "{name}", "{name}"],
                    &["full", "owner", "name", "host"],
                )
            },
            compute: repo,
        },
        Metric {
            meta: MetricMeta {
                priority: 3,
                ..meta(
                    "pr",
                    Category::Vcs,
                    "Pull request",
                    "Open PR or MR for the branch, with review state",
                    IconKey::Pr,
                    ["{num}[ {state}]", "{num}", "{num}"],
                    &["num", "state"],
                )
            },
            compute: pr,
        },
        Metric {
            meta: meta(
                "worktree",
                Category::Vcs,
                "Worktree",
                "Linked git worktree or --worktree session",
                IconKey::Worktree,
                ["{name}", "{name}", "{name}"],
                &["name", "branch"],
            ),
            compute: worktree,
        },
        Metric {
            meta: MetricMeta {
                priority: 2,
                ..meta(
                    "lines",
                    Category::Vcs,
                    "Lines changed",
                    "Lines added and removed this session",
                    IconKey::Lines,
                    [
                        "[{added}][ {removed}]",
                        "[{added}][ {removed}]",
                        "[{added}][ {removed}]",
                    ],
                    &["added", "removed"],
                )
            },
            compute: lines,
        },
    ]
}

fn dir(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let path = ctx.input.current_dir()?;
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let name = parts.last().copied().unwrap_or("/");
    let home = dirs::home_dir().map(|h| h.to_string_lossy().into_owned());
    let tilde = || match &home {
        Some(h) if !h.is_empty() && path.starts_with(h.as_str()) => {
            format!("~{}", &path[h.len()..])
        }
        _ => path.to_string(),
    };
    let shown = match opts.get("style").and_then(|v| v.as_str()).unwrap_or("name") {
        "full" => path.to_string(),
        "tilde" => tilde(),
        "parent" => parts[parts.len().saturating_sub(2)..].join("/"),
        "depth" => {
            let depth = opts
                .get("depth")
                .and_then(|v| v.as_integer())
                .unwrap_or(3)
                .max(1) as usize;
            let tail = parts[parts.len().saturating_sub(depth)..].join("/");
            if parts.len() > depth {
                format!("…/{tail}")
            } else if parts.is_empty() {
                name.to_string()
            } else {
                tail
            }
        }
        "short" => {
            let t = tilde();
            let segs: Vec<&str> = t.split('/').collect();
            segs.iter()
                .enumerate()
                .map(|(i, s)| {
                    if i + 1 == segs.len() || s.is_empty() || *s == "~" {
                        s.to_string()
                    } else {
                        s.chars()
                            .take(if s.starts_with('.') { 2 } else { 1 })
                            .collect()
                    }
                })
                .collect::<Vec<_>>()
                .join("/")
        }
        _ => name.to_string(),
    };
    Some(Reading::default().set("path", shown).set("name", name))
}

fn git(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let info = ctx.git()?;
    let ascii = ctx.glyphs == GlyphTier::Ascii;
    let branch = match (&info.branch, &info.oid) {
        (Some(b), _) => b.clone(),
        (None, Some(oid)) => format!("@{}", &oid[..oid.len().min(7)]),
        (None, None) => return None,
    };
    let (clean, dirty, conflict) = if ascii {
        ("ok", "*", "!")
    } else {
        ("✓", "●", "⚠")
    };
    let status = if info.conflicts > 0 {
        conflict
    } else if info.dirty() {
        dirty
    } else {
        clean
    };
    let (up, down) = if ascii { ("+", "-") } else { ("↑", "↓") };
    let mut r = Reading::default()
        .set("branch", branch_name(&branch, opts))
        .set("status", status)
        .set("dirty", if info.dirty() { "*" } else { "" })
        .link(info.branch.as_deref().and_then(|b| branch_link(ctx, b)));
    if info.ahead > 0 {
        r = r.set("ahead", format!("{up}{}", info.ahead));
    }
    if info.behind > 0 {
        r = r.set("behind", format!("{down}{}", info.behind));
    }
    if opts.get("sha").and_then(|v| v.as_bool()).unwrap_or(false)
        && let Some(oid) = &info.oid
    {
        r = r.set_role("sha", &oid[..oid.len().min(7)], Role::Muted);
    }
    for (name, n) in [
        ("staged", info.staged),
        ("modified", info.modified),
        ("untracked", info.untracked),
        ("conflicts", info.conflicts),
    ] {
        if n > 0 {
            r = r.set(name, n.to_string());
        }
    }
    if info.conflicts > 0 {
        r = r.severity(Severity::Crit);
    }
    Some(r)
}

fn branch_name(branch: &str, opts: &Options) -> String {
    opts.get("branch_prefixes")
        .and_then(|v| v.as_str())
        .into_iter()
        .flat_map(|s| s.split(','))
        .filter_map(|s| s.trim().split_once('='))
        .filter(|(prefix, _)| !prefix.is_empty() && branch.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map_or_else(
            || branch.to_string(),
            |(prefix, replacement)| format!("{replacement}{}", &branch[prefix.len()..]),
        )
}

fn branch_link(ctx: &Ctx, branch: &str) -> Option<String> {
    let repo = ctx.input.workspace.as_ref()?.repo.as_ref()?;
    let host = repo.host.as_deref().unwrap_or("github.com");
    let route = if host.eq_ignore_ascii_case("github.com") {
        "tree"
    } else if host
        .split('.')
        .any(|part| part.eq_ignore_ascii_case("gitlab"))
    {
        "-/tree"
    } else {
        return None;
    };
    let owner = repo.owner.as_deref()?;
    let name = repo.name.as_deref()?;
    let owner = owner
        .split('/')
        .map(url_segment)
        .collect::<Vec<_>>()
        .join("/");
    let branch = branch
        .split('/')
        .map(url_segment)
        .collect::<Vec<_>>()
        .join("/");
    Some(format!(
        "https://{host}/{owner}/{}/{route}/{branch}",
        url_segment(name)
    ))
}

fn url_segment(text: &str) -> String {
    text.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                char::from(b).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn repo(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let repo = ctx.input.workspace.as_ref()?.repo.as_ref()?;
    let (owner, name) = (repo.owner.as_deref()?, repo.name.as_deref()?);
    let host = repo.host.as_deref().unwrap_or("github.com");
    Some(
        Reading::default()
            .set("full", format!("{owner}/{name}"))
            .set("owner", owner)
            .set("name", name)
            .set("host", host)
            .link(Some(format!("https://{host}/{owner}/{name}"))),
    )
}

fn pr(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let pr = ctx.input.pr.as_ref()?;
    let number = pr.number?;
    let num = if pr.kind.as_deref() == Some("mr") {
        format!("!{number}")
    } else {
        format!("#{number}")
    };
    let (state, severity) = match pr.review_state.as_deref() {
        Some("approved") => ("approved", Severity::Good),
        Some("changes_requested") => ("changes", Severity::Crit),
        Some("pending") => ("pending", Severity::Normal),
        Some("draft") => ("draft", Severity::Normal),
        _ => ("", Severity::Normal),
    };
    Some(
        Reading::default()
            .set("num", num)
            .set("state", state)
            .severity(severity)
            .link(pr.url.clone()),
    )
}

fn worktree(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let session = ctx.input.worktree.as_ref();
    let name = session
        .and_then(|w| w.name.as_deref())
        .or_else(|| ctx.input.workspace.as_ref()?.git_worktree.as_deref())?;
    Some(
        Reading::default().set("name", name).set(
            "branch",
            session
                .and_then(|w| w.branch.as_deref())
                .unwrap_or_default(),
        ),
    )
}

fn lines(ctx: &Ctx, _: &Options) -> Option<Reading> {
    let c = ctx.input.cost.as_ref()?;
    let (added, removed) = (
        c.total_lines_added.unwrap_or(0),
        c.total_lines_removed.unwrap_or(0),
    );
    if added == 0 && removed == 0 {
        return None;
    }
    let mut r = Reading::default();
    if added > 0 {
        r = r.set_role("added", format!("+{added}"), Role::Good);
    }
    if removed > 0 {
        r = r.set_role("removed", format!("-{removed}"), Role::Bad);
    }
    Some(r)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{git::GitInfo, input::Input, metrics::GitSource, models::Registry};
    use chrono::FixedOffset;

    #[test]
    fn directory_depth_handles_both_separators_and_clamps_to_one() {
        let registry = Registry::default();
        for (path, depth, expected) in [
            ("/one/two/three/four", None, "…/two/three/four"),
            (r"C:\one\two\three", Some(2), "…/two/three"),
            ("/one/two", Some(3), "one/two"),
            ("/one/two/", Some(0), "…/two"),
            ("/one/two", Some(-2), "…/two"),
            ("/", Some(1), "/"),
        ] {
            let input = Input {
                cwd: Some(path.into()),
                ..Input::default()
            };
            let ctx = Ctx::new(
                &input,
                &registry,
                0,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            );
            let mut opts: Options = toml::from_str("style = 'depth'").unwrap();
            if let Some(depth) = depth {
                opts.insert("depth".into(), toml::Value::Integer(depth));
            }
            assert_eq!(
                dir(&ctx, &opts).unwrap().get("path").unwrap().text,
                expected
            );
        }
        let metric = metrics().into_iter().find(|m| m.meta.id == "dir").unwrap();
        assert!(
            metric
                .meta
                .options
                .iter()
                .any(|o| o.key == "depth" && o.kind == OptKind::Number)
        );
    }

    #[test]
    fn branch_prefix_uses_longest_match_and_skips_malformed_entries() {
        let opts = toml::from_str(
            "branch_prefixes = 'broken,=bad,feature/=✨,feature/ui/=UI-,fix/=🐛,hyy/='",
        )
        .unwrap();
        for (branch, expected) in [
            ("feature/ui/button", "UI-button"),
            ("feature/button", "✨button"),
            ("fix/bug", "🐛bug"),
            ("hyy/topic", "topic"),
            ("main", "main"),
        ] {
            assert_eq!(branch_name(branch, &opts), expected);
        }
        assert_eq!(branch_name("feature/test", &Options::new()), "feature/test");
        assert!(
            metrics()
                .into_iter()
                .find(|m| m.meta.id == "git")
                .unwrap()
                .meta
                .options
                .iter()
                .any(|o| o.key == "branch_prefixes")
        );
    }

    #[test]
    fn branch_links_preserve_path_separators() {
        let registry = Registry::default();
        for (host, route) in [("github.com", "tree"), ("gitlab.com", "-/tree")] {
            let input = Input::parse(&format!(
                r#"{{"workspace":{{"repo":{{"host":"{host}","owner":"acme","name":"app"}}}}}}"#
            ))
            .unwrap();
            let ctx = Ctx::new(
                &input,
                &registry,
                0,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            );
            assert_eq!(
                branch_link(&ctx, "feature/auth#%?/你好"),
                Some(format!(
                    "https://{host}/acme/app/{route}/feature/auth%23%25%3F/%E4%BD%A0%E5%A5%BD"
                ))
            );
        }
    }

    #[test]
    fn git_links_use_the_real_encoded_branch_on_supported_hosts() {
        let registry = Registry::default();
        for (host, owner, route) in [
            ("github.com", "acme", Some("tree")),
            ("gitlab.com", "acme/team", Some("-/tree")),
            ("gitlab.example.com", "acme", Some("-/tree")),
            ("example.com", "acme", None),
        ] {
            let input = Input::parse(&format!(
                r#"{{"workspace":{{"repo":{{"host":"{host}","owner":"{owner}","name":"app"}}}}}}"#
            ))
            .unwrap();
            let ctx = Ctx::new(
                &input,
                &registry,
                0,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            )
            .with_git(GitSource::Fixed(Some(GitInfo {
                branch: Some("feature/你好#%?".into()),
                ..GitInfo::default()
            })));
            let opts = toml::from_str("branch_prefixes = 'feature/=✨'").unwrap();
            let reading = git(&ctx, &opts).unwrap();
            assert_eq!(reading.get("branch").unwrap().text, "✨你好#%?");
            assert_eq!(
                reading.link,
                route.map(|route| format!(
                    "https://{host}/{owner}/app/{route}/feature/%E4%BD%A0%E5%A5%BD%23%25%3F"
                ))
            );
            let detached = Ctx::new(
                &input,
                &registry,
                0,
                FixedOffset::east_opt(0).unwrap(),
                GlyphTier::Unicode,
            )
            .with_git(GitSource::Fixed(Some(GitInfo {
                oid: Some("123456789".into()),
                ..GitInfo::default()
            })));
            assert!(git(&detached, &Options::new()).unwrap().link.is_none());
        }
    }
}
