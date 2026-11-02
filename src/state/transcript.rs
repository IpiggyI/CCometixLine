//! Read-only session summaries accumulated across bounded incremental scans.

use chrono::DateTime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const SCAN_BYTES: u64 = 8 * 1024 * 1024;
const IDENTITY_BYTES: u64 = 4096;
const CACHE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub start: Option<i64>,
    pub last_reply: Option<i64>,
    pub advisor: Option<String>,
    pub skills: Skills,
    pub hooks: Hooks,
    pub tool_calls: Box<ToolCalls>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolCalls {
    pub counts: BTreeMap<String, usize>,
    pub count: usize,
    pub last: Option<(String, bool)>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Skills {
    pub running: Vec<String>,
    pub done: Vec<String>,
    pub count: usize,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Hooks {
    pub last: Option<String>,
    pub last_failed: bool,
    pub counts: BTreeMap<String, usize>,
    pub failed: usize,
}

#[derive(Clone, Deserialize, Serialize, PartialEq, Eq)]
struct Stamp {
    path: PathBuf,
    len: u64,
    modified: Duration,
}

impl Stamp {
    fn at(path: &Path) -> Option<Self> {
        let meta = path.metadata().ok()?;
        meta.is_file().then_some(())?;
        Some(Self {
            path: path.to_owned(),
            len: meta.len(),
            modified: meta
                .modified()
                .ok()?
                .duration_since(SystemTime::UNIX_EPOCH)
                .ok()?,
        })
    }
}

#[derive(Deserialize, Serialize)]
struct Cached<T> {
    stamp: Stamp,
    data: T,
}

/// Read with a byte cap; oversized files yield no data.
pub(crate) fn bounded(path: &Path, limit: u64) -> Option<Vec<u8>> {
    if path.metadata().ok()?.len() > limit {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= limit).then_some(bytes)
}

/// Cache derived data only, with exact modification time and length checks.
pub(crate) fn cached<T: Serialize + DeserializeOwned>(
    path: &Path,
    cache_dir: &Path,
    read: impl FnOnce(&mut File, u64) -> Option<T>,
) -> Option<T> {
    let stamp = Stamp::at(path)?;
    let cache = cache_dir.join(format!("{}.json", super::file_key(&path.to_string_lossy())));
    if let Some(saved) =
        bounded(&cache, CACHE_BYTES).and_then(|b| serde_json::from_slice::<Cached<T>>(&b).ok())
        && saved.stamp == stamp
    {
        return Some(saved.data);
    }
    let data = read(&mut File::open(path).ok()?, stamp.len)?;
    if Stamp::at(path)? != stamp {
        return None;
    }
    let saved = Cached { stamp, data };
    let _ = super::write_json(&cache, &saved);
    super::sometimes(100, || {
        super::sweep(cache_dir, Duration::from_secs(7 * 86_400), 200);
    });
    Some(saved.data)
}

#[derive(Deserialize, Serialize)]
struct TranscriptCache {
    stamp: Stamp,
    head: Fingerprint,
    offset: u64,
    scanned: u64,
    fragment: Fragment,
    scan: Scan,
}

#[derive(Default, Deserialize, Serialize)]
struct Fragment {
    text: String,
    utf8_tail: Vec<u8>,
    invalid: bool,
}

impl Fragment {
    fn append(&mut self, bytes: &[u8]) {
        if self.invalid {
            return;
        }
        let mut joined = std::mem::take(&mut self.utf8_tail);
        joined.extend_from_slice(bytes);
        match std::str::from_utf8(&joined) {
            Ok(text) => self.text.push_str(text),
            Err(error) => {
                if error.error_len().is_some() {
                    self.invalid = true;
                    self.text.clear();
                } else {
                    if let Ok(text) = std::str::from_utf8(&joined[..error.valid_up_to()]) {
                        self.text.push_str(text);
                    }
                    self.utf8_tail
                        .extend_from_slice(&joined[error.valid_up_to()..]);
                }
            }
        }
    }
}

#[derive(Deserialize, Serialize, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    hash: u64,
}

fn fingerprint(file: &mut File, len: u64) -> Option<Fingerprint> {
    file.seek(SeekFrom::Start(0)).ok()?;
    let mut bytes = Vec::new();
    file.take(len.min(IDENTITY_BYTES))
        .read_to_end(&mut bytes)
        .ok()?;
    Some(Fingerprint {
        len: bytes.len() as u64,
        hash: super::fnv1a(&bytes),
    })
}

/// Resume at the last complete line; a changed identity starts a new summary.
pub fn read(path: &Path) -> Option<Summary> {
    read_at(path, &super::dir().join("transcripts-v2"))
}

fn read_at(path: &Path, cache_dir: &Path) -> Option<Summary> {
    let stamp = Stamp::at(path)?;
    let mut file = File::open(path).ok()?;
    let cache = cache_dir.join(format!("{}.json", super::file_key(&path.to_string_lossy())));
    let mut saved = TranscriptCache::load(&cache, &mut file, &stamp)?;
    if saved.stamp == stamp && saved.scanned == stamp.len {
        return Some(saved.scan.finish());
    }
    saved.head = fingerprint(&mut file, stamp.len)?;
    saved.advance(&mut file, stamp.len)?;
    if Stamp::at(path)? != stamp {
        return None;
    }
    saved.stamp = stamp;
    let _ = super::write_json(&cache, &saved);
    super::sometimes(100, || {
        super::sweep(cache_dir, Duration::from_secs(7 * 86_400), 200);
    });
    Some(saved.scan.finish())
}

fn timestamp(record: &Value) -> Option<i64> {
    DateTime::parse_from_rfc3339(record.get("timestamp")?.as_str()?)
        .ok()
        .map(|t| t.timestamp())
}

fn record(line: &[u8]) -> Option<Value> {
    let value: Value = serde_json::from_slice(line).ok()?;
    (value["type"].is_string() && value["isSidechain"] != true).then_some(value)
}

impl TranscriptCache {
    fn load(cache: &Path, file: &mut File, stamp: &Stamp) -> Option<Self> {
        // Pending names and a JSON-escaped unfinished line can exceed the fixed config cap.
        let limit = stamp.len.saturating_mul(8).saturating_add(CACHE_BYTES);
        let saved = bounded(cache, limit).and_then(|b| serde_json::from_slice::<Self>(&b).ok());
        if let Some(saved) = saved
            && saved.stamp.path == stamp.path
            && saved.stamp.len <= stamp.len
            && saved.offset <= saved.stamp.len
            && saved.offset <= saved.scanned
            && saved.scanned <= saved.stamp.len
            && (saved.stamp.len < stamp.len || saved.stamp == *stamp)
            && fingerprint(file, saved.head.len)? == saved.head
        {
            return Some(saved);
        }
        Some(Self {
            head: fingerprint(file, stamp.len)?,
            stamp: stamp.clone(),
            offset: 0,
            scanned: 0,
            fragment: Fragment::default(),
            scan: Scan::default(),
        })
    }

    fn advance(&mut self, file: &mut File, len: u64) -> Option<()> {
        file.seek(SeekFrom::Start(self.scanned)).ok()?;
        let mut bytes = Vec::new();
        file.take(SCAN_BYTES.min(len - self.scanned))
            .read_to_end(&mut bytes)
            .ok()?;
        for piece in bytes.split_inclusive(|&b| b == b'\n') {
            self.scanned += piece.len() as u64;
            self.fragment.append(piece);
            if piece.last() == Some(&b'\n') {
                if !self.fragment.invalid
                    && let Some(value) = record(self.fragment.text.as_bytes())
                {
                    self.scan.push(&value);
                }
                self.offset = self.scanned;
                self.fragment = Fragment::default();
            }
        }
        Some(())
    }
}

#[derive(Default, Deserialize, Serialize)]
struct Scan {
    summary: Summary,
    pending: Vec<Pending>,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    id: String,
    name: String,
    skill: Option<String>,
}

impl Scan {
    fn push(&mut self, record: &Value) {
        if self.summary.start.is_none() {
            self.summary.start = timestamp(record);
        }
        let content = record["message"]["content"].as_array();
        match record["type"].as_str() {
            Some("assistant") => {
                if let Some(at) = timestamp(record) {
                    self.summary.last_reply = Some(at);
                }
                if let Some(id) = record["advisorModel"].as_str().filter(|s| !s.is_empty()) {
                    self.summary.advisor = Some(id.into());
                }
                for block in content.into_iter().flatten() {
                    self.tool(block);
                }
            }
            Some("user") => self.user(&record["message"]["content"]),
            Some("attachment") => self.hook(&record["attachment"]),
            _ => {}
        }
    }

    fn tool(&mut self, block: &Value) {
        if block["type"] != "tool_use" {
            return;
        }
        let Some(name) = block["name"].as_str().filter(|s| !s.is_empty()) else {
            return;
        };
        self.summary.tool_calls.count += 1;
        *self
            .summary
            .tool_calls
            .counts
            .entry(name.into())
            .or_default() += 1;
        if let Some(id) = block["id"].as_str() {
            let skill = (name == "Skill")
                .then(|| block["input"]["skill"].as_str().map(str::to_owned))
                .flatten();
            self.summary.skills.count += usize::from(skill.is_some());
            self.pending.push(Pending {
                id: id.into(),
                name: name.into(),
                skill,
            });
        }
    }

    fn user(&mut self, content: &Value) {
        let prompt = content.as_str().is_some_and(|s| !s.is_empty())
            || content
                .as_array()
                .is_some_and(|a| a.iter().any(|b| b["type"] != "tool_result"));
        if prompt {
            self.summary.hooks = Hooks::default();
        }
        for block in content.as_array().into_iter().flatten() {
            if block["type"] == "tool_result"
                && let Some(i) = self
                    .pending
                    .iter()
                    .position(|p| block["tool_use_id"] == p.id)
            {
                let pending = self.pending.remove(i);
                self.summary.tool_calls.last = Some((pending.name, block["is_error"] == true));
                if let Some(skill) = pending.skill {
                    let done = &mut self.summary.skills.done;
                    done.push(skill);
                    done.drain(..done.len().saturating_sub(4));
                }
            }
        }
    }

    fn hook(&mut self, attachment: &Value) {
        let failed = match attachment["type"].as_str() {
            Some("hook_success") => false,
            Some("hook_non_blocking_error" | "hook_blocking_error") => true,
            _ => return,
        };
        let (Some(name), Some(event)) = (
            attachment["hookName"].as_str(),
            attachment["hookEvent"].as_str(),
        ) else {
            return;
        };
        let hooks = &mut self.summary.hooks;
        hooks.last = Some(name.into());
        hooks.last_failed = failed;
        hooks.failed += usize::from(failed);
        *hooks.counts.entry(event.into()).or_default() += 1;
    }

    fn finish(&self) -> Summary {
        let mut summary = self.summary.clone();
        summary.skills.running = self
            .pending
            .iter()
            .filter_map(|p| p.skill.clone())
            .take(3)
            .collect();
        summary
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::io::Write;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "ccline-transcript-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn summary(&self, text: &str) -> Summary {
            let path = self.0.join("session.jsonl");
            std::fs::write(&path, text).unwrap();
            read_at(&path, &self.0.join("cache")).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn lines(records: &[Value]) -> String {
        records.iter().map(|r| format!("{r}\n")).collect()
    }

    #[test]
    fn skips_headers_malformed_sidechains_and_partial_last_line() {
        let text = concat!(
            "{\"type\":\"last-prompt\"}\n{\"type\":\"mode\"}\nbroken\n",
            "{\"type\":\"assistant\",\"isSidechain\":true,\"timestamp\":\"1970-01-01T00:00:10Z\"}\n",
            "{\"type\":\"user\",\"timestamp\":\"1970-01-01T00:01:00Z\"}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"invalid\"}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:01:30Z\",\"advisorModel\":\"claude-opus-4-8\"}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:02:00Z\",\"advisorModel\":\"claude-opus-5-5\"}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:03:00Z\"}\n",
            "{\"type\":\"assistant\",\"isSidechain\":true,\"timestamp\":\"1970-01-01T00:04:00Z\",\"advisorModel\":\"wrong\"}\n",
            "{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:05:00Z\"}"
        );
        let summary = Fixture::new().summary(text);
        assert_eq!(summary.start, Some(60));
        assert_eq!(summary.last_reply, Some(180));
        assert_eq!(summary.advisor.as_deref(), Some("claude-opus-5-5"));
    }

    #[test]
    fn pairs_skill_results_and_keeps_bounded_recent_lists() {
        let mut records = Vec::new();
        for i in 0..10 {
            records.push(json!({"type":"assistant", "message":{"content":[
                {"type":"tool_use", "id":format!("id{i}"), "name":"Skill", "input":{"skill":format!("skill{i}")}}
            ]}}));
        }
        records.push(
            json!({"type":"user", "isSidechain":true, "message":{"content":[
                {"type":"tool_result", "tool_use_id":"id6"}
            ]}}),
        );
        for i in (0..6).rev() {
            records.push(json!({"type":"user", "message":{"content":[
                {"type":"tool_result", "tool_use_id":format!("id{i}")}
            ]}}));
        }
        records.push(
            json!({"type":"attachment", "attachment":{"type":"invoked_skills", "skills":["old"]}}),
        );
        records.push(
            json!({"type":"assistant", "isSidechain":true, "message":{"content":[
                {"type":"tool_use", "id":"side", "name":"Skill", "input":{"skill":"ignored"}}
            ]}}),
        );
        let summary = Fixture::new().summary(&lines(&records));
        assert_eq!(summary.tool_calls.count, 10);
        assert_eq!(summary.tool_calls.counts["Skill"], 10);
        let skills = summary.skills;
        assert_eq!(skills.count, 10);
        assert_eq!(skills.running, ["skill6", "skill7", "skill8"]);
        assert_eq!(skills.done, ["skill3", "skill2", "skill1", "skill0"]);
    }

    fn hook(kind: &str, event: &str) -> Value {
        json!({"type":"attachment", "attachment":{"type":kind, "hookName":format!("{event}:Bash"), "hookEvent":event}})
    }

    #[test]
    fn hooks_reset_on_prompts_but_not_tool_results() {
        let records = [
            hook("hook_blocking_error", "Old"),
            json!({"type":"user", "message":{"content":"New prompt"}}),
            hook("hook_success", "PreToolUse"),
            json!({"type":"user", "message":{"content":[{"type":"tool_result", "tool_use_id":"x"}]}}),
            hook("hook_non_blocking_error", "Stop"),
            hook("hook_blocking_error", "PreToolUse"),
            hook("hook_additional_context", "Ignored"),
            hook("hook_system_message", "Ignored"),
        ];
        let hooks = Fixture::new().summary(&lines(&records)).hooks;
        assert_eq!(hooks.last.as_deref(), Some("PreToolUse:Bash"));
        assert!(hooks.last_failed);
        assert_eq!(hooks.failed, 2);
        assert_eq!(
            hooks.counts,
            [("PreToolUse".into(), 2), ("Stop".into(), 1)].into()
        );
        let mut records = records.to_vec();
        records.push(json!({"type":"user", "message":{"content":[{"type":"tool_result"}, {"type":"text", "text":"Next"}]}}));
        assert_eq!(
            Fixture::new().summary(&lines(&records)).hooks,
            Hooks::default()
        );
    }

    #[test]
    fn counts_skills_in_the_middle_of_a_large_session() {
        let padding = "{\"type\":\"progress\"}\n".repeat(45_000);
        let text = padding.clone()
            + &lines(&[json!({"type":"assistant", "message":{"content":[
                {"type":"tool_use", "id":"middle", "name":"Skill", "input":{"skill":"review"}}
            ]}})])
            + &padding;
        assert_eq!(Fixture::new().summary(&text).skills.count, 1);
    }

    #[test]
    fn resumes_complete_lines_and_pairs_results_across_renders() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        let initial = lines(&[
            json!({"type":"user", "timestamp":"1970-01-01T00:01:00Z", "message":{"content":"prompt"}}),
            json!({"type":"assistant", "message":{"content":[
                {"type":"tool_use", "id":"skill", "name":"Skill", "input":{"skill":"review"}},
                {"type":"tool_use", "id":"bash", "name":"Bash"}
            ]}}),
            hook("hook_success", "PreToolUse"),
        ]);
        std::fs::write(&path, initial + "{\"type\":\"user\",\"message\":").unwrap();
        let first = read_at(&path, &cache).unwrap();
        assert_eq!(first.skills.running, ["review"]);
        assert_eq!(first.tool_calls.count, 2);
        assert_eq!(first.tool_calls.last, None);
        assert_eq!(read_at(&path, &cache), Some(first));
        let mut file = File::options().append(true).open(&path).unwrap();
        writeln!(file, r#"{{"content":[{{"type":"tool_result","tool_use_id":"skill"}},{{"type":"tool_result","tool_use_id":"bash","is_error":true}}]}}}}"#).unwrap();
        let second = read_at(&path, &cache).unwrap();
        assert_eq!(second.start, Some(60));
        assert!(second.skills.running.is_empty());
        assert_eq!(second.skills.done, ["review"]);
        assert_eq!(second.skills.count, 1);
        assert_eq!(second.tool_calls.count, 2);
        assert_eq!(second.tool_calls.last, Some(("Bash".into(), true)));
        assert_eq!(second.hooks.counts["PreToolUse"], 1);
        assert_eq!(read_at(&path, &cache), Some(second));
    }

    #[test]
    fn bounds_each_scan_and_resumes_even_without_new_appends() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        let padding = format!(
            "{{\"type\":\"progress\",\"text\":\"{}\"}}\n",
            "x".repeat(1000)
        );
        let text = padding.repeat(SCAN_BYTES as usize / padding.len() + 1)
            + &lines(&[
                json!({"type":"assistant", "advisorModel":"last", "message":{"content":[
                    {"type":"tool_use", "id":"late", "name":"Edit"}
                ]}}),
            ]);
        std::fs::write(&path, text).unwrap();
        assert_eq!(read_at(&path, &cache).unwrap().tool_calls.count, 0);
        let cache_path = cache.join(format!(
            "{}.json",
            super::super::file_key(&path.to_string_lossy())
        ));
        let saved: TranscriptCache =
            serde_json::from_slice(&bounded(&cache_path, CACHE_BYTES).unwrap()).unwrap();
        assert!(saved.offset > SCAN_BYTES - padding.len() as u64);
        assert!(saved.offset <= SCAN_BYTES);
        let summary = read_at(&path, &cache).unwrap();
        assert_eq!(summary.advisor.as_deref(), Some("last"));
        assert_eq!(summary.tool_calls.count, 1);
        assert_eq!(read_at(&path, &cache), Some(summary));
    }

    #[test]
    fn invalidates_on_truncation_rewrite_and_corrupt_cache() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        f.summary(&lines(&[json!({"type":"assistant", "advisorModel":"old"})]));
        let old_time = path.metadata().unwrap().modified().unwrap();
        std::fs::write(
            &path,
            lines(&[json!({"type":"assistant", "advisorModel":"new"})]),
        )
        .unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old_time)
            .unwrap();
        assert_eq!(
            read_at(&path, &cache).unwrap().advisor.as_deref(),
            Some("new")
        );
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_at(&path, &cache), Some(Summary::default()));
        f.summary(&lines(&[
            json!({"type":"assistant", "advisorModel":"regrown"}),
        ]));
        assert_eq!(
            read_at(&path, &cache).unwrap().advisor.as_deref(),
            Some("regrown")
        );
        for entry in std::fs::read_dir(&cache).unwrap() {
            std::fs::write(entry.unwrap().path(), "broken").unwrap();
        }
        assert_eq!(
            read_at(&path, &cache).unwrap().advisor.as_deref(),
            Some("regrown")
        );
        assert!(read_at(&f.0.join("missing"), &cache).is_none());
    }

    #[test]
    fn resumes_a_valid_record_larger_than_the_scan_budget() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        let prefix = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"large","name":"Skill","input":{"skill":"review","padding":""#;
        let text = prefix.to_owned()
            + &"x".repeat(SCAN_BYTES as usize - prefix.len() - 1)
            + "é"
            + &"x".repeat(100)
            + "\"}}]}}\n";
        std::fs::write(&path, text).unwrap();
        let first = read_at(&path, &cache).unwrap();
        assert_eq!(first.tool_calls.count, 0);
        let cache_path = cache.join(format!(
            "{}.json",
            super::super::file_key(&path.to_string_lossy())
        ));
        let saved = TranscriptCache::load(
            &cache_path,
            &mut File::open(&path).unwrap(),
            &Stamp::at(&path).unwrap(),
        )
        .unwrap();
        assert_eq!(saved.scanned, SCAN_BYTES);
        assert_eq!(saved.offset, 0);
        assert_eq!(saved.fragment.utf8_tail, "é".as_bytes()[..1]);
        let second = read_at(&path, &cache).unwrap();
        assert_eq!(second.tool_calls.count, 1);
        assert_eq!(second.skills.running, ["review"]);
        assert_eq!(read_at(&path, &cache), Some(second));
        let mut file = File::options().append(true).open(&path).unwrap();
        file.write_all(
            lines(&[json!({"type":"user", "message":{"content":[
                {"type":"tool_result", "tool_use_id":"large", "is_error":true}
            ]}})])
            .as_bytes(),
        )
        .unwrap();
        let last = read_at(&path, &cache).unwrap();
        assert_eq!(last.tool_calls.count, 1);
        assert_eq!(last.tool_calls.last, Some(("Skill".into(), true)));
        assert!(last.skills.running.is_empty());
        assert_eq!(last.skills.done, ["review"]);
    }

    #[test]
    fn resumes_when_pending_calls_make_the_cache_larger_than_eight_mib() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        let calls: Vec<_> = (0..2200).map(|i| json!({
            "type":"tool_use", "id":format!("id{i}"), "name":format!("tool{i}{}", "x".repeat(2048))
        })).collect();
        let mut text = lines(&[json!({"type":"assistant", "message":{"content":calls}})]);
        let padding = format!(
            "{{\"type\":\"progress\",\"text\":\"{}\"}}\n",
            "x".repeat(1000)
        );
        while text.len() <= SCAN_BYTES as usize {
            text.push_str(&padding);
        }
        text.push_str(&lines(&[json!({"type":"user", "message":{"content":[
            {"type":"tool_result", "tool_use_id":"id0"}
        ]}})]));
        std::fs::write(&path, text).unwrap();
        assert_eq!(read_at(&path, &cache).unwrap().tool_calls.count, 2200);
        let cache_path = cache.join(format!(
            "{}.json",
            super::super::file_key(&path.to_string_lossy())
        ));
        assert!(cache_path.metadata().unwrap().len() > CACHE_BYTES);
        let second = read_at(&path, &cache).unwrap();
        assert_eq!(second.tool_calls.count, 2200);
        assert_eq!(
            second.tool_calls.last,
            Some((format!("tool0{}", "x".repeat(2048)), false))
        );
        assert_eq!(read_at(&path, &cache), Some(second));
    }

    #[test]
    fn skips_invalid_utf8_across_chunks_and_recovers_at_the_next_line() {
        let mut fragment = Fragment::default();
        fragment.append(&[0xc3]);
        fragment.append(b"invalid");
        assert!(fragment.invalid);
        assert!(fragment.text.is_empty());
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        std::fs::write(
            &path,
            [
                b"\xff\n".as_slice(),
                b"{\"type\":\"assistant\",\"advisorModel\":\"valid\"}\n",
            ]
            .concat(),
        )
        .unwrap();
        assert_eq!(
            read_at(&path, &f.0.join("cache"))
                .unwrap()
                .advisor
                .as_deref(),
            Some("valid")
        );
    }

    #[test]
    fn cache_reuses_unchanged_data_and_invalidates_on_length_mtime_and_path() {
        let fixture = Fixture::new();
        let path = fixture.0.join("source");
        let cache = fixture.0.join("cache");
        std::fs::write(&path, "a").unwrap();
        assert_eq!(cached(&path, &cache, |_, _| Some(1)), Some(1));
        assert_eq!(
            cached::<i32>(&path, &cache, |_, _| panic!("unchanged source read")),
            Some(1)
        );
        std::fs::write(&path, "longer").unwrap();
        assert_eq!(cached(&path, &cache, |_, _| Some(2)), Some(2));
        let old = path.metadata().unwrap().modified().unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(old + Duration::from_secs(1))
            .unwrap();
        assert_eq!(cached(&path, &cache, |_, _| Some(3)), Some(3));
        let other = fixture.0.join("other");
        std::fs::write(&other, "longer").unwrap();
        assert_eq!(cached(&other, &cache, |_, _| Some(4)), Some(4));
        assert_eq!(
            cached::<i32>(&fixture.0.join("missing"), &cache, |_, _| panic!()),
            None
        );
    }

    #[test]
    fn missing_malformed_and_oversized_cache_are_lenient() {
        let fixture = Fixture::new();
        assert_eq!(fixture.summary("bad\n[]\nnull\n{}\n"), Summary::default());
        let path = fixture.0.join("oversized");
        File::create(&path)
            .unwrap()
            .set_len(CACHE_BYTES + 1)
            .unwrap();
        assert!(bounded(&path, CACHE_BYTES).is_none());
        assert!(bounded(&fixture.0.join("missing"), CACHE_BYTES).is_none());
    }
}
