//! Transcript parsing utilities shared by usage-driven segments.
//!
//! Walks a Claude Code transcript file forward and produces a
//! `SessionUsageStats` record carrying cumulative session totals, last-turn
//! usage and the last assistant timestamp. Sidechain (sub-agent) entries are
//! excluded by default. The legacy `parse_last_assistant_usage` API is kept
//! as a thin compatibility wrapper for callers that only need the last turn.

use crate::config::{NormalizedUsage, SessionUsageStats, TranscriptEntry};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

/// Read session-level usage stats from a transcript path.
///
/// Falls back to the most recent `.jsonl` in the same project directory when
/// the given path does not exist (mirrors Claude Code's resume semantics).
pub fn parse_session_stats<P: AsRef<Path>>(transcript_path: P) -> Option<SessionUsageStats> {
    let path = resolve_active_transcript(transcript_path.as_ref())?;
    let file = fs::File::open(path).ok()?;
    let reader = BufReader::new(file);

    let mut stats = SessionUsageStats::default();
    let mut had_assistant_usage = false;

    for line in reader.lines().map_while(Result::ok) {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let entry = match serde_json::from_str::<TranscriptEntry>(line) {
            Ok(entry) => entry,
            Err(_) => continue,
        };

        if entry.is_sidechain.unwrap_or(false) {
            continue;
        }

        if entry.r#type.as_deref() != Some("assistant") {
            continue;
        }

        let message = match entry.message {
            Some(message) => message,
            None => continue,
        };
        let raw_usage = match message.usage {
            Some(usage) => usage,
            None => continue,
        };

        let normalized = raw_usage.normalize();
        accumulate_into(&mut stats.totals, &normalized);
        stats.last_turn = normalized;
        stats.assistant_count = stats.assistant_count.saturating_add(1);
        had_assistant_usage = true;

        if let Some(timestamp) = entry.timestamp.as_deref() {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
                stats.last_assistant_at = Some(dt.with_timezone(&chrono::Utc));
            }
        }
    }

    had_assistant_usage.then_some(stats)
}

fn accumulate_into(acc: &mut NormalizedUsage, turn: &NormalizedUsage) {
    acc.input_tokens = acc.input_tokens.saturating_add(turn.input_tokens);
    acc.output_tokens = acc.output_tokens.saturating_add(turn.output_tokens);
    acc.total_tokens = acc.total_tokens.saturating_add(turn.total_tokens);
    acc.cache_creation_input_tokens = acc
        .cache_creation_input_tokens
        .saturating_add(turn.cache_creation_input_tokens);
    acc.cache_read_input_tokens = acc
        .cache_read_input_tokens
        .saturating_add(turn.cache_read_input_tokens);
    acc.cache_creation_5m = acc.cache_creation_5m.saturating_add(turn.cache_creation_5m);
    acc.cache_creation_1h = acc.cache_creation_1h.saturating_add(turn.cache_creation_1h);
}

/// Read the latest assistant message usage from a transcript path.
///
/// Compatibility wrapper that forwards to [`parse_session_stats`] and returns
/// only the last-turn usage. Existing call sites (e.g. `context_window`) keep
/// working unchanged.
pub fn parse_last_assistant_usage<P: AsRef<Path>>(transcript_path: P) -> Option<NormalizedUsage> {
    parse_session_stats(transcript_path).map(|stats| stats.last_turn)
}

fn resolve_active_transcript(path: &Path) -> Option<PathBuf> {
    if path.exists() {
        return Some(path.to_path_buf());
    }

    newest_jsonl_in(path.parent()?)
}

// v1.1.2 used to iterate older `.jsonl` files when the newest one had no
// usable assistant entry. The active transcript path supplied by Claude Code
// almost always exists, so this v1 simplification keeps "newest only" — the
// rare edge case (newest file truly empty) returns `None` instead of falling
// back, which is acceptable given the resume semantics.
fn newest_jsonl_in(project_dir: &Path) -> Option<PathBuf> {
    fs::read_dir(project_dir)
        .ok()?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("jsonl"))
        .max_by_key(|path| {
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(std::time::UNIX_EPOCH)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_transcript(name: &str, content: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ccline-transcript-{}-{}-{}.jsonl",
            std::process::id(),
            name,
            unique
        ));
        fs::write(&path, content).unwrap();
        path
    }

    fn assistant_line(input: u32, output: u32, cache_read: u32) -> String {
        format!(
            r#"{{"type":"assistant","message":{{"usage":{{"input_tokens":{},"output_tokens":{},"cache_read_input_tokens":{}}}}}}}"#,
            input, output, cache_read
        )
    }

    #[test]
    fn empty_file_returns_none() {
        let path = write_transcript("empty", "");
        assert!(parse_session_stats(&path).is_none());
    }

    #[test]
    fn no_assistant_returns_none() {
        let path = write_transcript(
            "no-assistant",
            r#"{"type":"user","message":{"content":"hello"}}"#,
        );
        assert!(parse_session_stats(&path).is_none());
    }

    #[test]
    fn single_assistant_has_matching_last_and_total_token_values() {
        let path = write_transcript("single", &assistant_line(10, 5, 3));
        let stats = parse_session_stats(&path).unwrap();

        assert_eq!(stats.assistant_count, 1);
        assert_eq!(stats.last_turn.input_tokens, stats.totals.input_tokens);
        assert_eq!(stats.last_turn.output_tokens, stats.totals.output_tokens);
        assert_eq!(
            stats.last_turn.cache_read_input_tokens,
            stats.totals.cache_read_input_tokens
        );
    }

    #[test]
    fn three_assistants_accumulate_totals_and_keep_last_turn() {
        let lines = [
            assistant_line(10, 1, 0),
            assistant_line(20, 2, 3),
            assistant_line(30, 4, 5),
        ]
        .join("\n");
        let path = write_transcript("three", &lines);
        let stats = parse_session_stats(&path).unwrap();

        assert_eq!(stats.assistant_count, 3);
        assert_eq!(stats.totals.input_tokens, 60);
        assert_eq!(stats.totals.output_tokens, 7);
        assert_eq!(stats.totals.cache_read_input_tokens, 8);
        assert_eq!(stats.last_turn.input_tokens, 30);
        assert_eq!(stats.last_turn.output_tokens, 4);
    }

    #[test]
    fn sidechain_assistants_are_skipped() {
        let content = [
            r#"{"type":"assistant","isSidechain":true,"message":{"usage":{"input_tokens":99,"output_tokens":1}}}"#,
            &assistant_line(7, 2, 0),
        ]
        .join("\n");
        let path = write_transcript("sidechain", &content);
        let stats = parse_session_stats(&path).unwrap();

        assert_eq!(stats.assistant_count, 1);
        assert_eq!(stats.totals.input_tokens, 7);
        assert_eq!(stats.totals.output_tokens, 2);
    }

    #[test]
    fn malformed_json_lines_are_skipped() {
        let content = format!("not-json\n{}", assistant_line(8, 3, 0));
        let path = write_transcript("malformed", &content);
        let stats = parse_session_stats(&path).unwrap();

        assert_eq!(stats.assistant_count, 1);
        assert_eq!(stats.totals.input_tokens, 8);
        assert_eq!(stats.totals.output_tokens, 3);
    }

    #[test]
    fn valid_timestamp_is_parsed_as_utc() {
        let path = write_transcript(
            "timestamp-valid",
            r#"{"type":"assistant","timestamp":"2026-05-01T12:34:56.789Z","message":{"usage":{"input_tokens":1}}}"#,
        );
        let stats = parse_session_stats(&path).unwrap();
        let expected = chrono::DateTime::parse_from_rfc3339("2026-05-01T12:34:56.789Z")
            .unwrap()
            .with_timezone(&chrono::Utc);

        assert_eq!(stats.last_assistant_at, Some(expected));
    }

    #[test]
    fn invalid_timestamp_keeps_tokens_without_timestamp() {
        let path = write_transcript(
            "timestamp-invalid",
            r#"{"type":"assistant","timestamp":"invalid","message":{"usage":{"input_tokens":7,"output_tokens":2}}}"#,
        );
        let stats = parse_session_stats(&path).unwrap();

        assert_eq!(stats.totals.input_tokens, 7);
        assert_eq!(stats.totals.output_tokens, 2);
        assert!(stats.last_assistant_at.is_none());
    }

    #[test]
    fn accumulate_into_sums_every_numeric_field() {
        let mut acc = NormalizedUsage::default();
        let turn_a = NormalizedUsage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            cache_creation_input_tokens: 4,
            cache_read_input_tokens: 5,
            cache_creation_5m: 6,
            cache_creation_1h: 7,
            ..Default::default()
        };
        let turn_b = NormalizedUsage {
            input_tokens: 10,
            output_tokens: 20,
            total_tokens: 30,
            cache_creation_input_tokens: 40,
            cache_read_input_tokens: 50,
            cache_creation_5m: 60,
            cache_creation_1h: 70,
            ..Default::default()
        };

        accumulate_into(&mut acc, &turn_a);
        accumulate_into(&mut acc, &turn_b);

        assert_eq!(acc.input_tokens, 11);
        assert_eq!(acc.output_tokens, 22);
        assert_eq!(acc.total_tokens, 33);
        assert_eq!(acc.cache_creation_input_tokens, 44);
        assert_eq!(acc.cache_read_input_tokens, 55);
        assert_eq!(acc.cache_creation_5m, 66);
        assert_eq!(acc.cache_creation_1h, 77);
    }

    #[test]
    fn accumulate_into_saturates_on_overflow() {
        let mut acc = NormalizedUsage {
            input_tokens: u32::MAX - 1,
            ..Default::default()
        };
        let turn = NormalizedUsage {
            input_tokens: 100,
            ..Default::default()
        };

        accumulate_into(&mut acc, &turn);

        assert_eq!(acc.input_tokens, u32::MAX);
    }
}
