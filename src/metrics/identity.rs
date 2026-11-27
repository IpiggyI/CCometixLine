use super::{Ctx, Metric, MetricMeta, OptKind, OptSpec, Options, Reading};
use crate::style::{Category, Color, GlyphTier, IconKey, Role};
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
                options: &[OptSpec {
                    key: "effort",
                    kind: OptKind::Flag,
                    default: "true",
                    summary: "Add the reasoning effort while thinking is on",
                }],
                ..meta(
                    "model",
                    Category::Identity,
                    "Model",
                    "Model name, corrected by the model registry, with its reasoning effort",
                    IconKey::Model,
                    [
                        "{name}[ · {effort}][ · {fast}]",
                        "{name}[ · {effort}][ · {fast}]",
                        "{family}[ · {fast}]",
                    ],
                    &[
                        "name", "family", "id", "effort", "fast", "orb", "ver", "short",
                    ],
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
                    "A newer ccline is published; links to its release notes",
                    IconKey::Update,
                    ["{version} available", "{version}", "{version}"],
                    &["version", "current"],
                )
            },
            compute: update,
        },
    ]
}

fn model(ctx: &Ctx, opts: &Options) -> Option<Reading> {
    let resolved = ctx.model()?;
    let family = resolved.name.split_whitespace().next().unwrap_or_default();
    let id = ctx.input.model_id().unwrap_or_default();
    let mut r = Reading::default()
        .set("name", &resolved.name)
        .set("family", family)
        .set("id", id);
    r = match Celestial::parse(id) {
        Some((body, ver)) => {
            let r = match body.color() {
                Some(c) => r.set_role("short", body.name(), Role::Hue(c)),
                None => r.set("short", body.name()),
            }
            .set("ver", ver);
            if ctx.glyphs == GlyphTier::Ascii {
                r
            } else {
                r.set_parts("orb", body.orb())
            }
        }
        None => r.set("short", &resolved.name),
    };
    let with_effort = opts.get("effort").and_then(|v| v.as_bool()).unwrap_or(true);
    if with_effort && let Some(level) = effort(ctx) {
        r = r.set("effort", level);
    }
    if ctx.input.fast_mode == Some(true) {
        r = r.set("fast", "fast");
    }
    Some(r)
}

/// The celestial body a Codex model is named after: `gpt-6.2-sol` is the
/// sun, version `6.2`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Celestial {
    Luna,
    Terra,
    Sol,
    Astra,
}

impl Celestial {
    /// The body and version of an id shaped `gpt-<version>-<body>`.
    fn parse(id: &str) -> Option<(Celestial, &str)> {
        let rest = id.strip_prefix("gpt-")?;
        let (ver, body) = rest.split_once('-')?;
        if !ver.starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        let body = match body.to_ascii_lowercase().as_str() {
            "luna" => Celestial::Luna,
            "terra" => Celestial::Terra,
            "sol" => Celestial::Sol,
            "astra" => Celestial::Astra,
            _ => return None,
        };
        Some((body, ver))
    }

    fn name(self) -> &'static str {
        match self {
            Celestial::Luna => "luna",
            Celestial::Terra => "terra",
            Celestial::Sol => "sol",
            Celestial::Astra => "astra",
        }
    }

    /// The body in its colors, with what goes around it: a star beside the
    /// moon, the earth's moon, the sun's inner planets, a star's trail.
    fn orb(self) -> Vec<(String, Role)> {
        let rgb = |hex: u32| Color::Rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8);
        let parts: &[(&str, Option<u32>)] = match self {
            Celestial::Luna => &[
                ("˙", Some(0x565f89)),
                ("☾", Some(0xc0caf5)),
                (" ", None),
                ("·", Some(0x737aa2)),
            ],
            Celestial::Terra => &[("🌏", None), (" ", None), ("·", Some(0xa9b1d6))],
            Celestial::Sol => &[
                ("☀", Some(0xe0af68)),
                (" ", None),
                ("·", Some(0x9aa5ce)),
                ("•", Some(0xe6c384)),
                ("•", Some(0x7aa2f7)),
            ],
            Celestial::Astra => &[
                ("─", Some(0x3b4261)),
                ("─", Some(0x737aa2)),
                ("✦", Some(0xe0e6ff)),
            ],
        };
        parts
            .iter()
            .map(|(t, c)| (t.to_string(), c.map_or(Role::Text, |c| Role::Hue(rgb(c)))))
            .collect()
    }

    /// The name's own color; `None` leaves it the item's text color.
    fn color(self) -> Option<Color> {
        match self {
            Celestial::Terra => Some(Color::Rgb(0x7a, 0xa2, 0xf7)),
            Celestial::Sol => Some(Color::Rgb(0xe0, 0xaf, 0x68)),
            Celestial::Luna | Celestial::Astra => None,
        }
    }
}

/// The reasoning effort as `Low`, `Med`, `High`, `xHigh` or `Max`, for
/// models that have one. Thinking and effort go together: with thinking off
/// there is none to show.
fn effort(ctx: &Ctx) -> Option<String> {
    if ctx.input.thinking.as_ref().and_then(|t| t.enabled) == Some(false) {
        return None;
    }
    let level = ctx.input.effort.as_ref()?.level.as_deref()?.to_lowercase();
    Some(match level.as_str() {
        "low" => "Low".into(),
        "medium" => "Med".into(),
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
    let name = ctx.input.session_name.as_deref()?;
    let max = opts
        .get("max_len")
        .and_then(|v| v.as_integer())
        .unwrap_or(30)
        .max(4) as usize;
    Some(Reading::default().set("name", crate::render::width::truncate(name, max)))
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

    /// The model item through `template`, as each piece's text and role.
    fn celestial(id: &str, tier: GlyphTier, template: &str) -> Vec<(String, Role)> {
        let json = format!(r#"{{"model": {{"id": "{id}", "display_name": "GPT X"}}}}"#);
        let input = Input::parse(&json).unwrap();
        let reg = Registry::default();
        let ctx = Ctx::new(&input, &reg, 0, FixedOffset::east_opt(0).unwrap(), tier);
        let r = read(find("model").unwrap(), &ctx, &Options::new()).unwrap();
        crate::widgets::template::render(template, &r)
            .into_iter()
            .map(|p| (p.text, p.role))
            .collect()
    }

    const BADGE: &str = "[{orb} ][{ver} ]{short}";

    #[test]
    fn celestial_models_get_their_body_version_and_color() {
        let text = |id, tier| -> String {
            celestial(id, tier, BADGE)
                .into_iter()
                .map(|p| p.0)
                .collect()
        };
        assert_eq!(text("gpt-6-luna", GlyphTier::Nerd), "˙☾ · 6 luna");
        assert_eq!(text("gpt-6.1-terra", GlyphTier::Unicode), "🌏 · 6.1 terra");
        assert_eq!(text("gpt-6.2-sol", GlyphTier::Nerd), "☀ ·•• 6.2 sol");
        assert_eq!(text("gpt-6.2-astra", GlyphTier::Nerd), "──✦ 6.2 astra");
        assert_eq!(text("gpt-6.2-sol", GlyphTier::Ascii), "6.2 sol");

        let hue = |id, field: &str, piece: &str| {
            celestial(id, GlyphTier::Nerd, field)
                .into_iter()
                .find(|p| p.0 == piece)
                .map(|p| p.1)
        };
        let amber = Some(Role::Hue(Color::Rgb(0xe0, 0xaf, 0x68)));
        // The sun and its name share the amber; its three planets each have
        // a color of their own; a star's trail brightens toward the star.
        assert_eq!(hue("gpt-6.2-sol", "{orb}", "☀"), amber);
        assert_eq!(hue("gpt-6.2-sol", "{short}", "sol"), amber);
        let planets: Vec<Role> = celestial("gpt-6.2-sol", GlyphTier::Nerd, "{orb}")
            .into_iter()
            .skip(2)
            .map(|p| p.1)
            .collect();
        assert_eq!(planets.len(), 3);
        assert!(planets[0] != planets[1] && planets[1] != planets[2] && planets[0] != planets[2]);
        let trail: Vec<(u8, u8, u8)> = celestial("gpt-6.2-astra", GlyphTier::Nerd, "{orb}")
            .into_iter()
            .map(|p| match p.1 {
                Role::Hue(c) => c.to_rgb(),
                other => panic!("{other:?}"),
            })
            .collect();
        assert!(trail.windows(2).all(|w| w[0].0 < w[1].0), "{trail:?}");
        // Luna's and astra's names keep the item's text color.
        assert_eq!(hue("gpt-6-luna", "{short}", "luna"), Some(Role::Text));
        assert_eq!(
            hue("gpt-6.1-terra", "{short}", "terra").map(|r| matches!(r, Role::Hue(_))),
            Some(true)
        );
    }

    #[test]
    fn other_models_show_their_name_without_a_body() {
        for id in [
            "gpt-reserve",
            "gpt-6.2-nova",
            "claude-opus-4-8",
            "gpt-x-sol",
        ] {
            let pieces = celestial(id, GlyphTier::Nerd, BADGE);
            let text: String = pieces.iter().map(|p| p.0.as_str()).collect();
            assert_eq!(text, "GPT X", "{id}");
        }
    }
}
