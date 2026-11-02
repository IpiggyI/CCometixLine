//! Persistent state under `~/.claude/ccline/state/`: per-session history,
//! cached git status, model ids seen, the daily cost ledger and the update
//! check. Every write is atomic; files shared between sessions are
//! read-modify-write with the last writer winning, which the next render
//! corrects.

pub mod bridge;
pub mod cmd_cache;
pub mod git_cache;
pub mod ledger;
pub mod models_seen;
pub mod session;
pub mod swr;
pub mod transcript;
pub mod update;

use crate::input::Input;
use crate::models::Registry;
use chrono::{DateTime, FixedOffset, Timelike};
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

pub use session::{Sample, SessionState};

pub fn dir() -> PathBuf {
    crate::paths::state_dir()
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

pub fn write_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let text = serde_json::to_string(value).map_err(std::io::Error::other)?;
    crate::paths::write_atomic(path, &text)
}

fn fnv1a(s: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// A stable file name for `s` (FNV-1a, hex).
pub fn file_key(s: &str) -> String {
    format!("{:016x}", fnv1a(s))
}

/// A short stable key for a prompt id; samples only compare them.
pub fn prompt_key(id: &str) -> u32 {
    fnv1a(id) as u32
}

/// Session ids become file names; keep only safe characters.
pub fn safe_name(id: &str) -> Option<String> {
    let name: String = id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(80)
        .collect();
    (!name.is_empty()).then_some(name)
}

/// Delete files in `dir` older than `max_age`, then the oldest beyond
/// `max_files`. Errors are ignored.
pub fn sweep(dir: &Path, max_age: Duration, max_files: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = SystemTime::now();
    let mut files: Vec<(SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|e| {
            let modified = e.metadata().ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .collect();
    files.retain(|(modified, path)| {
        let old = now.duration_since(*modified).is_ok_and(|age| age > max_age);
        if old {
            let _ = std::fs::remove_file(path);
        }
        !old
    });
    if files.len() > max_files {
        files.sort();
        for (_, path) in &files[..files.len() - max_files] {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// Take the lock file at `path` for work that should run once at a time; a
/// lock older than `stale` is taken to be abandoned. Returns whether the
/// lock is now held.
pub fn try_lock(path: &Path, stale: Duration) -> bool {
    let create = || {
        std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    match create() {
        Ok(_) => true,
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
            let abandoned = std::fs::metadata(path)
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > stale));
            abandoned && std::fs::remove_file(path).is_ok() && create().is_ok()
        }
        Err(_) => false,
    }
}

/// Start `ccline <args>` in the background, detached from Claude Code's
/// pipes and process group so it outlives the render and nothing waits for
/// it. Returns whether it started.
pub fn spawn_detached(args: &[&std::ffi::OsStr]) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    }
    cmd.spawn().is_ok()
}

/// Run `f` about once in `every` calls, keyed on the clock so no RNG is needed.
pub fn sometimes(every: u32, f: impl FnOnce()) {
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    if nanos.is_multiple_of(every) {
        f();
    }
}

/// What metrics may read from state during a render.
#[derive(Clone, Debug, Default)]
pub struct StateView {
    /// Fixed transcript and MCP data for previews; live renders read lazily.
    pub transcript: Option<transcript::Summary>,
    pub mcp: Option<Vec<String>>,
    /// Samples of this session, oldest first, ending with the current one.
    pub samples: Vec<Sample>,
    /// Spend across sessions today, from the ledger.
    pub today_cost: Option<f64>,
    /// Spend across local days in the current seven-day window.
    pub week_cost: Option<f64>,
    /// Opening timestamp of the counted seven-day window.
    pub week_since: Option<i64>,
    /// Spend across local days in the current calendar month.
    pub month_cost: Option<f64>,
    /// Newest ccline version known to be published.
    pub latest_version: Option<String>,
    /// What the companion mod reports for this session.
    pub bridge: Option<std::rc::Rc<bridge::Bridge>>,
}

/// Record this render: add a session sample, fold the session's cost into
/// today's ledger and note the model id. `raw` is the stdin payload, kept
/// for replay. Returns what metrics read back.
pub fn observe(
    input: &Input,
    raw: &str,
    registry: &Registry,
    now: i64,
    tz: FixedOffset,
) -> StateView {
    let current = Sample::from_input(input, now);
    let Some(id) = input
        .session_id
        .as_deref()
        .filter(|id| safe_name(id).is_some())
    else {
        return StateView {
            samples: vec![current],
            ..StateView::default()
        };
    };

    let mut session = SessionState::load(id);
    let added = session.record(current);
    session.last_input = serde_json::from_str(raw).ok();
    session.save_if_needed(id, now, added);

    let ledger = current.cost.and_then(|cost| {
        let local = DateTime::from_timestamp(now, 0)?.with_timezone(&tz);
        let midnight = now - local.num_seconds_from_midnight() as i64;
        let started_today = input
            .cost
            .as_ref()
            .and_then(|c| c.total_duration_ms)
            .is_some_and(|ms| now - (ms / 1000.0) as i64 >= midnight);
        let date = local.format("%Y-%m-%d").to_string();
        Some(ledger::update(id, cost, started_today, &date, now))
    });
    let costs = ledger
        .as_ref()
        .and_then(|l| period_costs(input, l, now, tz));

    if let Some(model) = &input.model
        && let Some(model_id) = model.id.as_deref()
    {
        let window = input
            .context_window
            .as_ref()
            .and_then(|c| c.context_window_size);
        let resolved = registry.resolve(model_id, model.display_name.as_deref(), window);
        models_seen::record(model_id, &resolved, window, now);
    }

    let mut samples = session.samples;
    if !added {
        // Unchanged values aren't stored, but rates still need "now".
        samples.push(current);
    }
    bridge::prune();
    StateView {
        transcript: None,
        mcp: None,
        samples,
        today_cost: ledger.as_ref().map(ledger::Ledger::today),
        week_cost: costs.map(|c| c.0),
        week_since: costs.map(|c| c.1),
        month_cost: costs.map(|c| c.2),
        latest_version: None,
        bridge: bridge::read(id, now * 1000).map(std::rc::Rc::new),
    }
}

/// Daily totals include the whole local day when a quota window opens mid-day.
fn period_costs(
    input: &Input,
    ledger: &ledger::Ledger,
    now: i64,
    tz: FixedOffset,
) -> Option<(f64, i64, f64)> {
    let local = DateTime::from_timestamp(now, 0)?.with_timezone(&tz);
    let midnight = now - local.num_seconds_from_midnight() as i64;
    let since = input
        .rate_limits
        .as_ref()
        .and_then(|r| r.seven_day.as_ref())
        .and_then(|w| w.resets_at)
        .filter(|t| t.is_finite())
        .map_or(midnight - 6 * 86_400, |reset| {
            (reset as i64).saturating_sub(7 * 86_400)
        });
    let week_date = DateTime::from_timestamp(since, 0)?
        .with_timezone(&tz)
        .format("%Y-%m-%d")
        .to_string();
    let month_date = local.format("%Y-%m-01").to_string();
    Some((ledger.since(&week_date), since, ledger.since(&month_date)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_totals_use_local_dates_and_quota_opening() {
        let tz = FixedOffset::east_opt(8 * 3600).unwrap();
        let now = DateTime::parse_from_rfc3339("2026-11-02T01:00:00+08:00")
            .unwrap()
            .timestamp();
        let mut ledger = ledger::Ledger::default();
        for (day, cost) in [
            ("2026-10-26", 1.0),
            ("2026-10-27", 3.0),
            ("2026-10-31", 7.0),
            ("2026-11-01", 15.0),
            ("2026-11-02", 31.0),
        ] {
            ledger.record(day, "a", cost, false, 0);
        }
        let mut input = Input::default();
        let (week, since, month) = period_costs(&input, &ledger, now, tz).unwrap();
        assert_eq!(week, 30.0);
        assert_eq!(month, 24.0);
        assert_eq!(
            since,
            DateTime::parse_from_rfc3339("2026-10-27T00:00:00+08:00")
                .unwrap()
                .timestamp()
        );
        let opening = DateTime::parse_from_rfc3339("2026-10-31T12:00:00+08:00")
            .unwrap()
            .timestamp();
        input.rate_limits = Some(crate::input::RateLimits {
            seven_day: Some(crate::input::Window {
                resets_at: Some((opening + 7 * 86_400) as f64),
                ..Default::default()
            }),
            ..Default::default()
        });
        assert_eq!(
            period_costs(&input, &ledger, now, tz),
            Some((28.0, opening, 24.0))
        );
        let december = DateTime::parse_from_rfc3339("2026-12-01T00:00:00+08:00")
            .unwrap()
            .timestamp();
        ledger.record("2026-12-01", "a", 32.0, false, 0);
        input.rate_limits = None;
        let (week, _, month) = period_costs(&input, &ledger, december, tz).unwrap();
        assert_eq!((week, month), (1.0, 1.0));
    }

    #[test]
    fn keys_are_stable_and_names_safe() {
        assert_eq!(file_key("/home/u/app"), file_key("/home/u/app"));
        assert_ne!(file_key("/a"), file_key("/b"));
        assert_eq!(
            safe_name("2ad4a42c-815f/../x").as_deref(),
            Some("2ad4a42c-815fx")
        );
        assert_eq!(safe_name("../"), None);
    }

    #[test]
    fn locks_once_until_released_or_abandoned() {
        let dir = std::env::temp_dir().join(format!("ccline-lock-{}", std::process::id()));
        let lock = dir.join("x.lock");
        assert!(try_lock(&lock, Duration::from_secs(60)));
        assert!(!try_lock(&lock, Duration::from_secs(60)));
        std::thread::sleep(Duration::from_millis(20));
        assert!(try_lock(&lock, Duration::from_millis(10)));
        std::fs::remove_file(&lock).unwrap();
        assert!(try_lock(&lock, Duration::from_secs(60)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sweeps_by_count() {
        let dir = std::env::temp_dir().join(format!("ccline-sweep-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for i in 0..5 {
            std::fs::write(dir.join(format!("{i}.json")), "{}").unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        sweep(&dir, Duration::from_secs(3600), 3);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["2.json", "3.json", "4.json"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
