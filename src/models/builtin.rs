//! Built-in model knowledge: names for Claude model ids in any provider's
//! spelling, and windows for a few third-party models (kept from v1).

use regex::Regex;
use std::sync::OnceLock;

pub struct BuiltinRule {
    pub pattern: &'static str,
    pub name: &'static str,
    pub window: u64,
}

/// Substring rules, checked in order (more specific patterns first).
pub const RULES: &[BuiltinRule] = &[
    BuiltinRule {
        pattern: "kimi-k2-turbo",
        name: "Kimi K2 Turbo",
        window: 128_000,
    },
    BuiltinRule {
        pattern: "kimi-k2",
        name: "Kimi K2",
        window: 128_000,
    },
    BuiltinRule {
        pattern: "glm-4.5",
        name: "GLM-4.5",
        window: 128_000,
    },
    BuiltinRule {
        pattern: "qwen3-coder",
        name: "Qwen Coder",
        window: 256_000,
    },
];

pub fn rule_for(id_lower: &str) -> Option<&'static BuiltinRule> {
    RULES.iter().find(|r| id_lower.contains(r.pattern))
}

/// A Claude model's family and version, e.g. `opus` 4.8.
#[derive(Clone, Debug, PartialEq)]
pub struct ClaudeModel {
    /// Lowercase: `opus`, `sonnet`, `haiku`, `fable` or `mythos`.
    pub family: String,
    pub major: u32,
    pub minor: Option<u32>,
}

/// Parse a Claude model id in any provider's spelling, e.g.
/// `claude-opus-4-8`, `claude-3-5-sonnet-20241022`,
/// `us.anthropic.claude-sonnet-4-5-20250929-v1:0`, `claude-sonnet-5@20250929`
/// or `anthropic/claude-opus-4.1`.
pub fn claude_model(id: &str) -> Option<ClaudeModel> {
    static FAMILY: OnceLock<Regex> = OnceLock::new();
    let re = FAMILY.get_or_init(|| {
        let kw = "opus|sonnet|haiku|fable|mythos";
        Regex::new(&format!(
            r"(?i)(?:(?P<pre_major>\d{{1,2}})(?:[.-](?P<pre_minor>\d{{1,2}}))?-(?P<pre_kw>{kw})|(?P<post_kw>{kw})-(?P<post_major>\d{{1,2}})(?:[.-](?P<post_minor>\d{{1,2}}))?)(?:-\d{{3,}}|-[a-z]|\[|@|:|$)"
        ))
        .expect("family regex compiles")
    });
    let caps = re.captures(id)?;
    let family = caps.name("post_kw").or_else(|| caps.name("pre_kw"))?;
    let major = caps.name("post_major").or_else(|| caps.name("pre_major"))?;
    let minor = caps.name("post_minor").or_else(|| caps.name("pre_minor"));
    Some(ClaudeModel {
        family: family.as_str().to_ascii_lowercase(),
        major: major.as_str().parse().ok()?,
        minor: minor.and_then(|m| m.as_str().parse().ok()),
    })
}

/// Display name of a Claude model id, e.g. `Opus 4.8`.
pub fn claude_name(id: &str) -> Option<String> {
    let m = claude_model(id)?;
    let mut chars = m.family.chars();
    let family: String = chars
        .next()
        .map(|c| c.to_ascii_uppercase())
        .into_iter()
        .chain(chars)
        .collect();
    Some(match m.minor {
        Some(minor) => format!("{family} {}.{minor}", m.major),
        None => format!("{family} {}", m.major),
    })
}

/// API prices in USD per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
    pub cache_read: f64,
}

/// Official pricing: https://platform.claude.com/docs/en/about-claude/pricing.
/// `prompt_tokens` includes input, cache reads and cache writes. US-only
/// `inference_geo` adds 1.1x, but is not reported by the input and is not applied.
pub fn price(id: &str, fast_mode: bool, prompt_tokens: u64) -> Option<Price> {
    let m = claude_model(id)?;
    let version = (m.major, m.minor.unwrap_or(0));
    let (mut input, mut output) = match (m.family.as_str(), version) {
        ("fable" | "mythos", (5, 0 | 1)) => (10.0, 50.0),
        ("opus", (5, 5)) => (4.0, 20.0),
        ("opus", (5, 0) | (4, 5..=8)) => (5.0, 25.0),
        ("opus", (4, 0 | 1)) => (15.0, 75.0),
        ("sonnet", (5, 0 | 5)) => (2.0, 10.0),
        ("sonnet", (4, 0 | 5 | 6)) => (3.0, 15.0),
        ("haiku", (5, 5)) if prompt_tokens > 100_000 => (0.5, 2.5),
        ("haiku", (5, 5)) => (0.1, 0.5),
        ("haiku", (4, 5)) => (1.0, 5.0),
        ("haiku", (3, 5)) => (0.8, 4.0),
        ("haiku", (3, 0)) => (0.25, 1.25),
        _ => return None,
    };
    if fast_mode && m.family == "opus" {
        match version {
            (5, 5) => (input, output) = (8.0, 40.0),
            (5, 0) | (4, 8) => (input, output) = (10.0, 50.0),
            _ => {}
        }
    }
    let read = match (m.family.as_str(), version) {
        ("fable" | "mythos", (5, 1)) => 0.025,
        ("opus" | "sonnet", (5, 5)) => 0.05,
        _ => 0.1,
    };
    Some(Price {
        input,
        output,
        cache_write_5m: input * 1.25,
        cache_write_1h: input * 2.0,
        cache_read: input * read,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_claude_ids_across_providers() {
        let cases = [
            ("claude-opus-4-8", "Opus 4.8"),
            ("claude-opus-5-5[1m]", "Opus 5.5"),
            ("claude-3-5-sonnet-20241022", "Sonnet 3.5"),
            ("claude-3-opus-20240229", "Opus 3"),
            ("claude-opus-4-20250514", "Opus 4"),
            ("claude-sonnet-4-5-20250929", "Sonnet 4.5"),
            ("us.anthropic.claude-sonnet-4-5-20250929-v1:0", "Sonnet 4.5"),
            ("claude-sonnet-5@20250929", "Sonnet 5"),
            ("anthropic/claude-opus-4.1", "Opus 4.1"),
            ("claude-haiku-4-5", "Haiku 4.5"),
            ("claude-fable-5", "Fable 5"),
            ("claude-sonnet-4-thinking", "Sonnet 4"),
        ];
        for (id, want) in cases {
            assert_eq!(claude_name(id).as_deref(), Some(want), "{id}");
        }
    }

    #[test]
    fn prices_known_models_only() {
        let cases = [
            ("fable-5-1", 10.0, 50.0, 0.025),
            ("mythos-5-1", 10.0, 50.0, 0.025),
            ("fable-5", 10.0, 50.0, 0.1),
            ("mythos-5", 10.0, 50.0, 0.1),
            ("opus-5-5", 4.0, 20.0, 0.05),
            ("opus-5", 5.0, 25.0, 0.1),
            ("opus-4-8", 5.0, 25.0, 0.1),
            ("opus-4-7", 5.0, 25.0, 0.1),
            ("opus-4-6", 5.0, 25.0, 0.1),
            ("opus-4-5", 5.0, 25.0, 0.1),
            ("opus-4-1", 15.0, 75.0, 0.1),
            ("opus-4", 15.0, 75.0, 0.1),
            ("sonnet-5-5", 2.0, 10.0, 0.05),
            ("sonnet-5", 2.0, 10.0, 0.1),
            ("sonnet-4-6", 3.0, 15.0, 0.1),
            ("sonnet-4-5", 3.0, 15.0, 0.1),
            ("sonnet-4", 3.0, 15.0, 0.1),
            ("haiku-5-5", 0.1, 0.5, 0.1),
            ("haiku-4-5", 1.0, 5.0, 0.1),
            ("haiku-3-5", 0.8, 4.0, 0.1),
            ("haiku-3", 0.25, 1.25, 0.1),
        ];
        for (model, input, output, read) in cases {
            let p = price(&format!("claude-{model}"), false, 100_000).unwrap();
            assert_eq!(
                p,
                Price {
                    input,
                    output,
                    cache_write_5m: input * 1.25,
                    cache_write_1h: input * 2.0,
                    cache_read: input * read,
                },
                "{model}"
            );
        }
        for model in [
            "claude-opus-6",
            "claude-3-opus-20240229",
            "claude-haiku-4",
            "gpt-5",
            "kimi-k2",
        ] {
            assert_eq!(price(model, false, 0), None, "{model}");
        }
    }

    #[test]
    fn applies_fast_rates_and_long_prompt_tier() {
        for (id, input, output) in [
            ("opus-5-5", 8.0, 40.0),
            ("opus-5", 10.0, 50.0),
            ("opus-4-8", 10.0, 50.0),
        ] {
            let p = price(&format!("claude-{id}"), true, 0).unwrap();
            assert_eq!(
                (p.input, p.output, p.cache_write_5m, p.cache_write_1h),
                (input, output, input * 1.25, input * 2.0)
            );
        }
        for id in ["claude-sonnet-5-5", "claude-opus-4-7", "claude-haiku-5-5"] {
            assert_eq!(price(id, true, 0), price(id, false, 0));
        }
        let p = price("claude-haiku-5-5", false, 100_001).unwrap();
        assert_eq!((p.input, p.output, p.cache_read), (0.5, 2.5, 0.05));
    }

    #[test]
    fn ignores_non_claude_ids() {
        assert_eq!(claude_name("kimi-k2-0905-preview"), None);
        assert_eq!(claude_name("gpt-5"), None);
    }

    #[test]
    fn prefers_specific_builtin_rules() {
        assert_eq!(
            rule_for("kimi-k2-turbo-preview").unwrap().name,
            "Kimi K2 Turbo"
        );
        assert_eq!(rule_for("moonshot/kimi-k2").unwrap().name, "Kimi K2");
        assert!(rule_for("deepseek-v4").is_none());
    }
}
