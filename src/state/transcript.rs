//! Read-only session summaries from a 64 KiB head and 512 KiB tail budget.
//! A byte from the head budget identifies the first complete tail line.

use chrono::DateTime;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

const HEAD_BYTES: u64 = 64 * 1024;
const TAIL_BYTES: u64 = 512 * 1024;
const CACHE_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub start: Option<i64>,
    pub last_reply: Option<i64>,
    pub advisor: Option<String>,
    pub skills: Skills,
    pub hooks: Hooks,
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

#[derive(Deserialize, Serialize, PartialEq, Eq)]
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
        .take(limit + 1)
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

/// The first timestamp is read from the head; activity is limited to the tail.
pub fn read(path: &Path) -> Option<Summary> {
    cached(path, &super::dir().join("transcripts-v1"), summarize)
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

fn first_timestamp(file: &mut File) -> Option<i64> {
    let mut reader = BufReader::new(file.take(HEAD_BYTES - 1));
    let mut line = Vec::new();
    while reader.read_until(b'\n', &mut line).ok()? > 0 {
        if line.last() == Some(&b'\n')
            && let Some(at) = record(&line).and_then(|r| timestamp(&r))
        {
            return Some(at);
        }
        line.clear();
    }
    None
}

fn summarize(file: &mut File, len: u64) -> Option<Summary> {
    let start = first_timestamp(file);
    let offset = len.saturating_sub(TAIL_BYTES);
    file.seek(SeekFrom::Start(offset.saturating_sub(1))).ok()?;
    let mut tail = Vec::new();
    file.take(TAIL_BYTES + u64::from(offset > 0))
        .read_to_end(&mut tail)
        .ok()?;
    let from = if offset == 0 {
        0
    } else {
        tail.iter()
            .position(|&b| b == b'\n')
            .map_or(tail.len(), |i| i + 1)
    };
    let mut scan = Scan::default();
    for line in tail[from..].split_inclusive(|&b| b == b'\n') {
        if line.last() == Some(&b'\n')
            && let Some(value) = record(line)
        {
            scan.push(&value);
        }
    }
    scan.summary.start = start;
    Some(scan.finish())
}

#[derive(Default)]
struct Scan {
    summary: Summary,
    pending: Vec<(String, String)>,
}

impl Scan {
    fn push(&mut self, record: &Value) {
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
                    self.skill(block);
                }
            }
            Some("user") => self.user(&record["message"]["content"]),
            Some("attachment") => self.hook(&record["attachment"]),
            _ => {}
        }
    }

    fn skill(&mut self, block: &Value) {
        if block["type"] != "tool_use" || block["name"] != "Skill" {
            return;
        }
        let (Some(id), Some(name)) = (block["id"].as_str(), block["input"]["skill"].as_str())
        else {
            return;
        };
        self.summary.skills.count += 1;
        self.pending.push((id.into(), name.into()));
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
                    .position(|(id, _)| block["tool_use_id"] == *id)
            {
                self.summary.skills.done.push(self.pending.remove(i).1);
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

    fn finish(mut self) -> Summary {
        self.summary.skills.running = self
            .pending
            .into_iter()
            .take(3)
            .map(|(_, name)| name)
            .collect();
        let done = &mut self.summary.skills.done;
        done.drain(..done.len().saturating_sub(4));
        self.summary
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
            summarize(&mut File::open(&path).unwrap(), text.len() as u64).unwrap()
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
        let skills = Fixture::new().summary(&lines(&records)).skills;
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
    fn bounds_head_and_tail_and_ignores_the_middle() {
        let fixture = Fixture::new();
        let path = fixture.0.join("large.jsonl");
        let mut file = File::create(&path).unwrap();
        file.write_all(b"{\"type\":\"user\",\"timestamp\":\"1970-01-01T00:01:00Z\"}\n")
            .unwrap();
        file.seek(SeekFrom::Start(HEAD_BYTES + 100)).unwrap();
        file.write_all(b"\n{\"type\":\"assistant\",\"advisorModel\":\"outside-window\"}\n")
            .unwrap();
        file.seek(SeekFrom::Start(2 * 1024 * 1024)).unwrap();
        file.write_all(b"\n{\"type\":\"assistant\",\"timestamp\":\"1970-01-01T00:03:00Z\"}\n")
            .unwrap();
        let len = file.metadata().unwrap().len();
        let summary = summarize(&mut File::open(&path).unwrap(), len).unwrap();
        assert_eq!(summary.start, Some(60));
        assert_eq!(summary.last_reply, Some(180));
        assert!(summary.advisor.is_none());
        let text = "x".repeat(HEAD_BYTES as usize + 1)
            + "\n{\"type\":\"user\",\"timestamp\":\"1970-01-01T00:01:00Z\"}\n";
        assert!(fixture.summary(&text).start.is_none());
    }

    #[test]
    fn drops_partial_first_tail_line_and_handles_no_complete_tail_lines() {
        let fixture = Fixture::new();
        let prefix = "x".repeat(TAIL_BYTES as usize);
        let text = prefix.clone()
            + "{\"type\":\"assistant\",\"advisorModel\":\"partial\"}\n"
            + "{\"type\":\"assistant\",\"advisorModel\":\"complete\"}\n";
        assert_eq!(fixture.summary(&text).advisor.as_deref(), Some("complete"));
        assert_eq!(fixture.summary(&(prefix + "x")), Summary::default());
    }

    #[test]
    fn retains_a_complete_line_at_the_exact_tail_boundary() {
        let fixture = Fixture::new();
        let assistant = "{\"type\":\"assistant\",\"advisorModel\":\"aligned\"}\n";
        let text = "x".repeat(2 * HEAD_BYTES as usize)
            + "\n"
            + assistant
            + &"x".repeat(TAIL_BYTES as usize - assistant.len());
        assert_eq!(fixture.summary(&text).advisor.as_deref(), Some("aligned"));
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
