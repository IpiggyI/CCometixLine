use std::path::PathBuf;

/// Expand a leading `~` or `~/...` against `dirs::home_dir()`.
pub fn expand_tilde(raw: &str) -> PathBuf {
    let trimmed = raw.trim();
    if let Some(home) = dirs::home_dir() {
        if trimmed == "~" {
            return home;
        }
        if let Some(rest) = trimmed
            .strip_prefix("~/")
            .or_else(|| trimmed.strip_prefix("~\\"))
        {
            return home.join(rest);
        }
    }
    PathBuf::from(trimmed)
}

/// Antigravity CLI's own directory: `$AGY_CLI_DIR` if set, else `~/.gemini/antigravity-cli`.
pub fn agy_cli_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGY_CLI_DIR").filter(|d| !d.is_empty()) {
        return expand_tilde(&dir.to_string_lossy());
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".gemini")
        .join("antigravity-cli")
}

/// Antigravity's shared config directory: `$AGY_CONFIG_DIR` if set, else `~/.gemini/config`.
pub fn agy_config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGY_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return expand_tilde(&dir.to_string_lossy());
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".gemini")
        .join("config")
}

/// agyline's data directory: `$AGYLINE_HOME` if set, else `~/.gemini/antigravity-cli/agyline`.
///
/// Kept separate from `~/.claude/ccline` and `$CCLINE_HOME` so Claude Code's `ccline` stays isolated.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("AGYLINE_HOME").filter(|d| !d.is_empty()) {
        return expand_tilde(&dir.to_string_lossy());
    }
    agy_cli_dir().join("agyline")
}

pub fn config_file() -> PathBuf {
    home().join("config.toml")
}

pub fn models_file() -> PathBuf {
    home().join("models.toml")
}

/// The user's own presets, one file each.
pub fn presets_dir() -> PathBuf {
    home().join("presets")
}

pub fn state_dir() -> PathBuf {
    home().join("state")
}

/// Kept for compatibility with Claude config callers; resolves to `agy_cli_dir()`.
pub fn claude_dir() -> PathBuf {
    agy_cli_dir()
}

/// Write `contents` to `path` atomically: write a sibling temp file, then
/// rename it over the target.
pub fn write_atomic(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}
