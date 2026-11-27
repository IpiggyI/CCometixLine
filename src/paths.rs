use std::path::PathBuf;

/// codexline's data directory: `$CODEXLINE_HOME` if set, else
/// `~/.codex/codexline`.
///
/// Kept apart from Claude Code's `~/.claude/ccline`, so the two status lines
/// have their own config and state.
pub fn home() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEXLINE_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir()
        .unwrap_or_default()
        .join(".codex")
        .join("codexline")
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

/// Claude Code's own config directory, honoring `CLAUDE_CONFIG_DIR`.
pub fn claude_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_default().join(".claude")
}

/// Codex's own home, honoring `CODEX_HOME`; its threads are under `sessions`.
pub fn codex_home() -> PathBuf {
    if let Some(dir) = std::env::var_os("CODEX_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    dirs::home_dir().unwrap_or_default().join(".codex")
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
