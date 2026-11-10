//! Per-model weekly limits (such as Fable's) from a usage snapshot file.
//!
//! Claude Code's status line payload carries only the 5-hour, 7-day and
//! spend windows; the per-model weekly windows reach `/usage` alone. A
//! separate tool (cc-usage, or a script around Claude Code's `get_usage`
//! control request) writes them to `usage-snapshot.json` in Claude Code's
//! config directory, in the shape claude-hud's external snapshot uses:
//! `{updated_at, model_scoped: [{display_name, utilization, resets_at}]}`.
//! Renders only read it.

use serde_json::Value;
use std::path::Path;

const FILE: &str = "usage-snapshot.json";
/// The writer refreshes every 5 to 10 minutes; older data is left unshown.
const MAX_AGE: i64 = 15 * 60;
const MAX_WINDOWS: usize = 8;
const MAX_NAME: usize = 64;

/// One per-model weekly window.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScopedWindow {
    /// The server's model label, such as `Fable`.
    pub name: String,
    pub used_percentage: Option<f64>,
    /// Unix epoch seconds.
    pub resets_at: Option<f64>,
}

/// The windows in Claude Code's config directory, if the snapshot is fresh.
pub fn read(now: i64) -> Vec<ScopedWindow> {
    read_at(&crate::paths::claude_dir().join(FILE), now)
}

pub fn read_at(path: &Path, now: i64) -> Vec<ScopedWindow> {
    let Some(doc) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
    else {
        return Vec::new();
    };
    let fresh = doc
        .get("updated_at")
        .and_then(timestamp)
        .is_some_and(|t| now - t as i64 <= MAX_AGE);
    if !fresh {
        return Vec::new();
    }
    let Some(rows) = doc.get("model_scoped").and_then(Value::as_array) else {
        return Vec::new();
    };
    rows.iter()
        .filter_map(|row| {
            let name =
                crate::state::transcript::clean(row.get("display_name")?.as_str()?, MAX_NAME);
            (!name.is_empty()).then(|| ScopedWindow {
                name,
                used_percentage: row
                    .get("utilization")
                    .and_then(Value::as_f64)
                    .filter(|p| p.is_finite()),
                resets_at: row.get("resets_at").and_then(timestamp),
            })
        })
        .take(MAX_WINDOWS)
        .collect()
}

/// Epoch seconds from an ISO 8601 string or a number of seconds or
/// milliseconds.
fn timestamp(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => {
            let n = n.as_f64().filter(|n| n.is_finite() && *n > 0.0)?;
            Some(if n > 1e12 { n / 1000.0 } else { n })
        }
        Value::String(s) => chrono::DateTime::parse_from_rfc3339(s.trim())
            .ok()
            .map(|t| t.timestamp() as f64),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_794_279_600; // 2026-11-10T03:00:00Z

    /// A snapshot file under the temp directory, removed on drop.
    struct Snapshot(std::path::PathBuf);

    impl Drop for Snapshot {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn write(name: &str, body: &str) -> Snapshot {
        let path = std::env::temp_dir().join(format!(
            "ccline-usage-snapshot-{}-{name}.json",
            std::process::id()
        ));
        std::fs::write(&path, body).unwrap();
        Snapshot(path)
    }

    #[test]
    fn a_fresh_snapshot_yields_its_windows() {
        let file = write(
            "fresh",
            r#"{"updated_at": "2026-11-10T02:55:00.000Z",
                "model_scoped": [{"display_name": "Fable", "utilization": 37, "resets_at": "2026-11-16T02:00:00+00:00"}]}"#,
        );
        assert_eq!(
            read_at(&file.0, NOW),
            vec![ScopedWindow {
                name: "Fable".into(),
                used_percentage: Some(37.0),
                resets_at: Some(1_794_794_400.0),
            }]
        );
    }

    #[test]
    fn a_stale_or_undated_snapshot_yields_nothing() {
        let row =
            r#""model_scoped": [{"display_name": "Fable", "utilization": 37, "resets_at": null}]"#;
        let stale = write(
            "stale",
            &format!(r#"{{"updated_at": "2026-11-10T02:44:59Z", {row}}}"#),
        );
        let undated = write("undated", &format!("{{{row}}}"));
        assert!(read_at(&stale.0, NOW).is_empty());
        assert!(read_at(&undated.0, NOW).is_empty());
        assert!(read_at(Path::new("/nonexistent/usage-snapshot.json"), NOW).is_empty());
    }

    #[test]
    fn accepts_epoch_times_null_usage_and_cleans_labels() {
        let file = write(
            "epoch",
            &format!(
                r#"{{"updated_at": {ms},
                "model_scoped": [
                  {{"display_name": "Fa\u001b[31mble\nX", "utilization": null, "resets_at": {s}}},
                  {{"display_name": "  ", "utilization": 5}}
                ]}}"#,
                ms = NOW * 1000,
                s = NOW + 3600,
            ),
        );
        let windows = read_at(&file.0, NOW);
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].name, "Fa[31mble X");
        assert_eq!(windows[0].used_percentage, None);
        assert_eq!(windows[0].resets_at, Some((NOW + 3600) as f64));
    }
}
