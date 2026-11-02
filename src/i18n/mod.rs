//! The configuration TUI's language. English text is the key: `t` gives
//! the current language's text for it, and the English itself when the
//! table has none, so text without a translation still reads.

mod zh;

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::Cell;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;

/// A language the TUI is drawn in.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Lang {
    #[default]
    En,
    Zh,
}

/// The `language` setting.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Language {
    /// From the locale: `LC_ALL`, then `LC_MESSAGES`, then `LANG`.
    #[default]
    Auto,
    En,
    Zh,
}

impl Language {
    pub const ALL: [Language; 3] = [Language::Auto, Language::En, Language::Zh];

    pub fn name(self) -> &'static str {
        match self {
            Language::Auto => "auto",
            Language::En => "en",
            Language::Zh => "zh",
        }
    }

    /// The setting in its own language, for choosing it.
    pub fn label(self) -> &'static str {
        match self {
            Language::Auto => t("auto"),
            Language::En => "English",
            Language::Zh => "中文",
        }
    }

    /// The language to draw in; `auto` takes the locale's.
    pub fn resolve(self, locale: Lang) -> Lang {
        match self {
            Language::Auto => locale,
            Language::En => Lang::En,
            Language::Zh => Lang::Zh,
        }
    }
}

impl Serialize for Language {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.name())
    }
}

/// A language ccline doesn't know means `auto`, so a typo never costs the
/// rest of the config.
impl<'de> Deserialize<'de> for Language {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = String::deserialize(d)?;
        Ok(Language::ALL
            .into_iter()
            .find(|l| l.name() == name)
            .unwrap_or_default())
    }
}

/// The locale's language, from the environment.
pub fn locale() -> Lang {
    from_locale(|k| std::env::var(k).ok())
}

/// The language the first set locale variable names; English when none
/// is set (as on most Windows systems) or it isn't Chinese.
fn from_locale(var: impl Fn(&str) -> Option<String>) -> Lang {
    let locale = ["LC_ALL", "LC_MESSAGES", "LANG"]
        .into_iter()
        .filter_map(var)
        .find(|v| !v.is_empty());
    match locale {
        Some(l) if l.to_lowercase().starts_with("zh") => Lang::Zh,
        _ => Lang::En,
    }
}

thread_local! {
    // Per thread, so tests drawing in one language don't reach others.
    static LANG: Cell<Lang> = const { Cell::new(Lang::En) };
}

pub fn set(lang: Lang) {
    LANG.with(|l| l.set(lang));
}

pub fn lang() -> Lang {
    LANG.with(Cell::get)
}

fn table(lang: Lang) -> Option<&'static HashMap<&'static str, &'static str>> {
    static ZH: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    match lang {
        Lang::En => None,
        Lang::Zh => Some(ZH.get_or_init(|| zh::TEXT.iter().copied().collect())),
    }
}

/// `en` in the current language.
pub fn t(en: &str) -> &str {
    table(lang()).and_then(|t| t.get(en).copied()).unwrap_or(en)
}

/// `template` with each `{name}` replaced by its value; other braces stay.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = template.to_string();
    for (name, value) in args {
        out = out.replace(&format!("{{{name}}}"), &value.to_string());
    }
    out
}

/// `t` of a template, then its `{name}` placeholders filled:
/// `tf!("{n} more lines", n = count)`.
#[macro_export]
macro_rules! tf {
    ($en:literal $(, $name:ident = $value:expr)* $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::t($en),
            &[$((stringify!($name), &$value as &dyn ::std::fmt::Display)),*],
        )
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn falls_back_to_english() {
        set(Lang::Zh);
        assert_eq!(t("save"), "保存");
        assert_eq!(t("no such text"), "no such text");
        assert_eq!(tf!("{n} more lines", n = 3), "还有 3 行");
        set(Lang::En);
        assert_eq!(t("save"), "save");
        assert_eq!(tf!("{n} more lines", n = 3), "3 more lines");
    }

    #[test]
    fn reads_the_locale() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(name, _)| *name == k)
                    .map(|(_, v)| v.to_string())
            }
        };
        assert_eq!(from_locale(env(&[("LANG", "zh_CN.UTF-8")])), Lang::Zh);
        assert_eq!(from_locale(env(&[("LANG", "en_US.UTF-8")])), Lang::En);
        assert_eq!(
            from_locale(env(&[("LC_ALL", "C"), ("LANG", "zh_CN.UTF-8")])),
            Lang::En
        );
        assert_eq!(
            from_locale(env(&[("LC_ALL", ""), ("LC_MESSAGES", "zh_TW")])),
            Lang::Zh
        );
        assert_eq!(from_locale(env(&[])), Lang::En);
    }

    #[test]
    fn unknown_languages_mean_auto() {
        #[derive(Deserialize)]
        struct F {
            language: Language,
        }
        let parse = |s: &str| toml::from_str::<F>(s).unwrap().language;
        assert_eq!(parse("language = \"zh\""), Language::Zh);
        assert_eq!(parse("language = \"fr\""), Language::Auto);
    }

    #[test]
    fn every_catalog_text_has_chinese() {
        let keys: BTreeSet<&str> = zh::TEXT.iter().map(|(en, _)| *en).collect();
        let mut texts: Vec<&str> = Vec::new();
        for m in crate::metrics::catalog() {
            texts.extend([m.meta.name, m.meta.summary]);
            texts.extend(m.meta.options.iter().map(|o| o.summary));
        }
        for p in crate::presets::PRESETS {
            texts.push(p.summary);
            texts.extend(p.tags);
        }
        texts.extend(crate::scenarios::SCENARIOS.iter().map(|s| s.name));
        let missing: BTreeSet<&str> = texts.into_iter().filter(|en| !keys.contains(en)).collect();
        assert!(missing.is_empty(), "no Chinese for:\n{missing:#?}");
    }

    /// Every string literal in `src`, outside this module.
    fn sources() -> Vec<(String, String)> {
        fn walk(dir: &std::path::Path, out: &mut Vec<(String, String)>) {
            for entry in std::fs::read_dir(dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    if !path.ends_with("i18n") {
                        walk(&path, out);
                    }
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    out.push((path.display().to_string(), text));
                }
            }
        }
        let mut out = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut out,
        );
        out
    }

    /// The text of the string literal starting at `s` (after its quote).
    fn literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    '"' => out.push('"'),
                    '\\' => out.push('\\'),
                    '\'' => out.push('\''),
                    _ => return None,
                },
                c => out.push(c),
            }
        }
        None
    }

    #[test]
    fn every_text_has_chinese_and_every_chinese_a_text() {
        let keys: BTreeSet<&str> = zh::TEXT.iter().map(|(en, _)| *en).collect();
        assert_eq!(keys.len(), zh::TEXT.len(), "a text is in the table twice");
        let files = sources();
        let mut missing = BTreeSet::new();
        for (path, text) in &files {
            for opener in ["t(", "tf!("] {
                for (at, _) in text.match_indices(opener) {
                    let before = text[..at].chars().next_back();
                    if before.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                        continue;
                    }
                    // rustfmt may put the literal on the next line.
                    let rest = text[at + opener.len()..].trim_start();
                    if let Some(en) = rest.strip_prefix('"').and_then(literal)
                        && !keys.contains(en.as_str())
                    {
                        missing.insert(format!("{path}: {en:?}"));
                    }
                }
            }
        }
        assert!(missing.is_empty(), "no Chinese for:\n{missing:#?}");
        let all: String = files.iter().map(|(_, t)| t.as_str()).collect();
        let unused: Vec<_> = keys
            .iter()
            .filter(|en| {
                let quoted = format!("{:?}", en);
                !all.contains(&quoted)
            })
            .collect();
        assert!(
            unused.is_empty(),
            "Chinese for text no longer used: {unused:#?}"
        );
    }
}
