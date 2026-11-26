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
const RECENT: usize = 256;
const AGENTS: usize = 128;
const NAMES: usize = 128;
// Subagent transcripts open with a prompt snapshot of 100-200 KiB before the
// first assistant record that carries the effort.
const HEAD_BYTES: u64 = 1024 * 1024;

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct SessionTokens {
    pub input: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub output: u64,
}

impl SessionTokens {
    pub fn total(&self) -> u64 {
        self.input + self.cache_read + self.cache_write + self.output
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Summary {
    pub start: Option<i64>,
    pub last_reply: Option<i64>,
    pub advisor: Option<String>,
    pub skills: Box<Skills>,
    pub hooks: Hooks,
    pub tool_calls: Box<ToolCalls>,
    pub agents: Vec<Agent>,
    pub agent_count: usize,
    pub todos: Vec<Todo>,
    #[serde(default)]
    pub compactions: usize,
    #[serde(default)]
    pub context_tokens: Option<u64>,
    #[serde(default)]
    pub last_model: Option<String>,
    #[serde(default)]
    pub selected_model: Option<String>,
    #[serde(default)]
    pub session_tokens: SessionTokens,
    #[serde(default)]
    pub estimated_cost_micros: u64,
    #[serde(default)]
    pub mcp_servers: Vec<String>,
    #[serde(default)]
    pub mcp_errors: Vec<String>,
}

impl Summary {
    pub fn estimated_cost_usd(&self) -> Option<f64> {
        (self.estimated_cost_micros > 0).then(|| self.estimated_cost_micros as f64 / 1_000_000.0)
    }
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct ToolCalls {
    pub counts: BTreeMap<String, usize>,
    pub count: usize,
    pub last: Option<(String, bool)>,
    pub recent: Vec<Tool>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub enum Status {
    #[default]
    Running,
    Completed,
    Error,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Tool {
    pub id: String,
    pub name: String,
    pub target: String,
    pub status: Status,
    pub start: i64,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Agent {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub agent_id: Option<String>,
    pub start: i64,
    pub end: Option<i64>,
    pub background: bool,
    pub effort_read: bool,
    pub effort_stamp: Option<(u64, Duration)>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
pub struct Todo {
    pub content: String,
    pub status: String,
}

/// Keep untrusted transcript text on one terminal line, without control sequences.
pub fn clean(text: &str, limit: usize) -> String {
    text.split_whitespace()
        .map(|part| part.chars().filter(|c| !c.is_control()).collect::<String>())
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

pub fn command_file(command: &str) -> String {
    let command = command.trim();
    let token = if let Some(quote) = command.chars().next().filter(|c| matches!(c, '\'' | '"')) {
        command[1..].split(quote).next().unwrap_or_default()
    } else {
        command.split_whitespace().next().unwrap_or_default()
    };
    let path = token.replace('\\', "/");
    clean(path.rsplit('/').next().unwrap_or_default(), 128)
}

pub fn basename(command: &str) -> String {
    let file = command_file(command);
    clean(
        file.rsplit_once('.')
            .map_or(file.as_str(), |(stem, _)| stem),
        64,
    )
}

pub fn short_event(event: &str) -> String {
    clean(
        match event {
            "PreToolUse" => "Pre",
            "PostToolUse" => "Post",
            "PostToolUseFailure" => "PostFailure",
            other => other,
        },
        64,
    )
}

fn cap<T>(list: &mut Vec<T>, max: usize) {
    list.drain(..list.len().saturating_sub(max));
}

fn increment(counts: &mut BTreeMap<String, usize>, name: String) {
    if counts.contains_key(&name) || counts.len() < NAMES {
        *counts.entry(name).or_default() += 1;
    }
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
        if self.text.len().saturating_add(bytes.len()) > SCAN_BYTES as usize * 2 {
            self.invalid = true;
            self.text.clear();
        }
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

/// Resolve `transcript_path` with legacy Antigravity `.gemini/antigravity/brain/...`
/// remapping, canonical `brain/<id>/` fallbacks, and token-file protection.
pub fn resolve_path(input: &crate::input::Input) -> Option<PathBuf> {
    let cli_dir = crate::paths::agy_cli_dir();
    let token = cli_dir.join("antigravity-oauth-token");
    let token_real = std::fs::canonicalize(&token).ok();
    let allowed = |candidate: &Path| -> bool {
        if !candidate.is_absolute() {
            return false;
        }
        let name = candidate.file_name().and_then(|n| n.to_str());
        if name == Some("antigravity-oauth-token") {
            return false;
        }
        if let Ok(meta) = std::fs::symlink_metadata(candidate) {
            if meta.is_dir() {
                return false;
            }
            if let Ok(real) = std::fs::canonicalize(candidate) {
                if real == cli_dir || real == token || token_real.as_ref() == Some(&real) {
                    return false;
                }
                if real.is_dir() {
                    return false;
                }
            }
        }
        true
    };
    let exists_file = |p: &Path| allowed(p) && std::fs::metadata(p).is_ok_and(|m| m.is_file());

    let mut canonical = Vec::new();
    for id in [
        input.conversation_id.as_deref(),
        input.session_id.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(safe) = super::safe_name(id) {
            canonical.push(
                cli_dir
                    .join("brain")
                    .join(&safe)
                    .join(".system_generated")
                    .join("logs")
                    .join("transcript.jsonl"),
            );
            canonical.push(cli_dir.join("brain").join(&safe).join("transcript.jsonl"));
        }
    }

    if let Some(raw) = input
        .transcript_path
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let raw_path = PathBuf::from(raw);
        if exists_file(&raw_path) {
            return Some(raw_path);
        }
        if let Some(remapped) = remap_legacy_transcript(&cli_dir, raw)
            && exists_file(&remapped)
        {
            return Some(remapped);
        }
        for c in &canonical {
            if exists_file(c) {
                return Some(c.clone());
            }
        }
        if allowed(&raw_path) {
            return Some(remap_legacy_transcript(&cli_dir, raw).unwrap_or(raw_path));
        }
    }

    canonical.into_iter().find(|c| exists_file(c))
}

fn remap_legacy_transcript(cli_dir: &Path, raw: &str) -> Option<PathBuf> {
    let norm = raw.replace('\\', "/");
    let (_, rest) = norm.split_once("/.gemini/antigravity/brain/")?;
    let mut out = cli_dir.join("brain");
    for seg in rest
        .split('/')
        .filter(|s| !s.is_empty() && *s != "." && *s != "..")
    {
        out.push(seg);
    }
    Some(out)
}

/// Read the session title from `~/.gemini/antigravity-cli/annotations/<conv_id>.pbtxt`.
pub fn read_annotation_title(conv_id: &str) -> Option<String> {
    let safe = super::safe_name(conv_id)?;
    let path = crate::paths::agy_cli_dir()
        .join("annotations")
        .join(format!("{safe}.pbtxt"));
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if meta.file_type().is_symlink() || !meta.is_file() || meta.len() > 64 * 1024 {
        return None;
    }
    let text = std::fs::read_to_string(&path).ok()?;
    parse_pbtxt_title(&text)
}

fn parse_pbtxt_title(text: &str) -> Option<String> {
    let idx = text.find("title:")?;
    let after = text[idx + "title:".len()..].trim_start();
    let quote = after.chars().next().filter(|c| *c == '"' || *c == '\'')?;
    let mut raw = Vec::new();
    let mut bytes = after.as_bytes()[1..].iter().copied().peekable();
    while let Some(b) = bytes.next() {
        if b == quote as u8 {
            break;
        }
        if b == b'\\' {
            match bytes.next() {
                Some(b'n') => raw.push(b'\n'),
                Some(b'r') => raw.push(b'\r'),
                Some(b't') => raw.push(b'\t'),
                Some(b'\\') => raw.push(b'\\'),
                Some(b'"') => raw.push(b'"'),
                Some(b'\'') => raw.push(b'\''),
                Some(b'x') => {
                    let h1 = bytes.next();
                    let h2 = bytes.next();
                    if let (Some(a), Some(b)) = (h1, h2)
                        && let Ok(v) =
                            u8::from_str_radix(std::str::from_utf8(&[a, b]).unwrap_or_default(), 16)
                    {
                        raw.push(v);
                    }
                }
                Some(d) if (b'0'..=b'7').contains(&d) => {
                    let mut oct = vec![d];
                    for _ in 0..2 {
                        if let Some(&n) = bytes.peek()
                            && (b'0'..=b'7').contains(&n)
                        {
                            oct.push(bytes.next().unwrap());
                        }
                    }
                    if let Ok(v) = u8::from_str_radix(std::str::from_utf8(&oct).unwrap_or("0"), 8) {
                        raw.push(v);
                    }
                }
                Some(other) => raw.push(other),
                None => break,
            }
        } else {
            raw.push(b);
        }
    }
    let decoded = String::from_utf8_lossy(&raw);
    let cleaned = clean(&decoded, 120);
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Resume at the last complete line; a changed identity starts a new summary.
pub fn read(path: &Path) -> Option<Summary> {
    read_at(path, &super::dir().join("transcripts-v5"))
}

fn read_at(path: &Path, cache_dir: &Path) -> Option<Summary> {
    let stamp = Stamp::at(path)?;
    let mut file = File::open(path).ok()?;
    let cache = cache_dir.join(format!("{}.json", super::file_key(&path.to_string_lossy())));
    let mut saved = TranscriptCache::load(&cache, &mut file, &stamp)?;
    let advanced = saved.stamp != stamp || saved.scanned != stamp.len;
    if advanced {
        saved.head = fingerprint(&mut file, stamp.len)?;
        saved.advance(&mut file, stamp.len)?;
    }
    let enriched = saved.scan.efforts(path);
    if !advanced && !enriched {
        return Some(saved.scan.finish());
    }
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
    let raw = record
        .get("timestamp")
        .or_else(|| record.get("created_at"))
        .and_then(|v| v.as_str())?;
    DateTime::parse_from_rfc3339(raw)
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
        let limit = 32 * 1024 * 1024;
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
    task_index: BTreeMap<String, Option<usize>>,
    next_task: usize,
    #[serde(default)]
    next_seq: usize,
    #[serde(default)]
    pending_agy: Vec<AgyPending>,
    #[serde(default)]
    running_bg: BTreeMap<i64, AgyPending>,
}

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
enum AgyPendingKind {
    Tool,
    Agent,
    Skill,
}

#[derive(Clone, Deserialize, Serialize)]
struct AgyPending {
    kind: AgyPendingKind,
    id: String,
    mcp_server: Option<String>,
}

#[derive(Deserialize, Serialize)]
struct Pending {
    id: String,
    skill: Option<String>,
}

fn parse_selected_model(content: &str) -> Option<String> {
    let start = content.find("<USER_SETTINGS_CHANGE>")? + "<USER_SETTINGS_CHANGE>".len();
    let end = content[start..].find("</USER_SETTINGS_CHANGE>")?;
    let block = &content[start..start + end];
    let after_key = block.find("Model Selection")?;
    let line = block[after_key..].lines().next()?;
    let sentence = line.split_once(". ").map_or(line, |(head, _)| head);
    let after_from = sentence.split_once(" from ")?.1;
    let cleaned = after_from
        .split_once(" to ")?
        .1
        .trim()
        .trim_end_matches('.')
        .trim();
    (!cleaned.is_empty()).then(|| cleaned.to_string())
}

impl Scan {
    fn push(&mut self, record: &Value) {
        if self.summary.start.is_none() {
            self.summary.start = timestamp(record);
        }
        self.completion(record);
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
                    self.tool(block, timestamp(record).unwrap_or(0));
                }
            }
            Some("user") => self.user(record),
            Some("attachment") => self.hook(&record["attachment"]),
            Some("system") if record["subtype"] == "compact_boundary" => {
                self.summary.compactions += 1;
            }
            Some("PLANNER_RESPONSE") => self.agy_planner(record),
            Some("USER_INPUT") => {
                self.flush_agy(Status::Completed, timestamp(record).unwrap_or(0));
                if let Some(model) = record["content"].as_str().and_then(parse_selected_model) {
                    self.summary.selected_model = Some(clean(&model, 128));
                }
            }
            Some("CHECKPOINT") => {
                self.summary.compactions += 1;
                self.accumulate_agy_tokens(record, false);
            }
            Some("ERROR_MESSAGE") => {
                self.flush_agy(Status::Error, timestamp(record).unwrap_or(0));
            }
            Some("GENERIC") => self.agy_generic(record),
            Some("SYSTEM_MESSAGE") => self.agy_system_message(record),
            _ => {}
        }
    }

    fn tool(&mut self, block: &Value, at: i64) {
        if block["type"] != "tool_use" {
            return;
        }
        let Some(name) = block["name"].as_str().filter(|s| !s.is_empty()) else {
            return;
        };
        let Some(id) = block["id"].as_str().filter(|id| !id.is_empty()) else {
            return;
        };
        let id = clean(id, 128);
        if self.pending.iter().any(|p| p.id == id)
            || self.summary.tool_calls.recent.iter().any(|t| t.id == id)
            || self.summary.agents.iter().any(|a| a.id == id)
        {
            return;
        }
        let input = &block["input"];
        match name {
            "Agent" | "Task" => {
                self.summary.agent_count += 1;
                self.summary.agents.push(Agent {
                    id,
                    kind: clean(input["subagent_type"].as_str().unwrap_or("agent"), 64),
                    description: clean(input["description"].as_str().unwrap_or_default(), 256),
                    model: string(&input["model"], 128),
                    start: at,
                    background: input["run_in_background"] == true,
                    ..Agent::default()
                });
                cap(&mut self.summary.agents, AGENTS);
            }
            "TodoWrite" | "TaskCreate" | "TaskUpdate" => self.todo(name, &id, input),
            "TaskList" | "TaskGet" => {}
            "Skill" => {
                if let Some(skill) = string(&input["skill"], 64) {
                    self.summary.skills.count += 1;
                    self.pending.push(Pending {
                        id,
                        skill: Some(skill),
                    });
                    cap(&mut self.pending, RECENT);
                }
            }
            _ => {
                self.summary.tool_calls.count += 1;
                self.summary.tool_calls.recent.push(Tool {
                    id,
                    name: clean(name, 128),
                    target: target(name, input),
                    status: Status::Running,
                    start: at,
                });
                cap(&mut self.summary.tool_calls.recent, RECENT);
            }
        }
    }

    fn user(&mut self, record: &Value) {
        let content = &record["message"]["content"];
        let prompt = content.as_str().is_some_and(|s| !s.is_empty())
            || content
                .as_array()
                .is_some_and(|a| a.iter().any(|b| b["type"] != "tool_result"));
        if prompt {
            self.summary.hooks = Hooks::default();
        }
        for block in content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|b| b["type"] == "tool_result")
        {
            self.result(block, record);
        }
    }

    fn result(&mut self, block: &Value, record: &Value) {
        let Some(id) = block["tool_use_id"].as_str() else {
            return;
        };
        if let Some(i) = self
            .summary
            .tool_calls
            .recent
            .iter()
            .position(|t| t.id == id && t.status == Status::Running)
        {
            let mut tool = self.summary.tool_calls.recent.remove(i);
            let failed = block["is_error"] == true;
            tool.status = if failed {
                Status::Error
            } else {
                Status::Completed
            };
            self.summary.tool_calls.last = Some((tool.name.clone(), failed));
            increment(&mut self.summary.tool_calls.counts, tool.name.clone());
            self.summary.tool_calls.recent.push(tool);
        }
        if let Some(i) = self.pending.iter().position(|p| p.id == id) {
            let pending = self.pending.remove(i);
            if let Some(skill) = pending.skill {
                self.summary.skills.done.push(skill);
                cap(&mut self.summary.skills.done, 4);
            }
        }
        if let Some(agent) = self.summary.agents.iter_mut().find(|a| a.id == id) {
            let result = &record["toolUseResult"];
            if let Some(id) = string(&result["agentId"], 64) {
                agent.agent_id = Some(id);
            }
            if let Some(model) = string(&result["resolvedModel"], 128) {
                agent.model = Some(model);
            }
            agent.background |= result["isAsync"] == true || result["status"] == "async_launched";
            if !agent.background && agent.end.is_none() {
                agent.end = Some(timestamp(record).unwrap_or(0));
            }
        }
    }

    fn completion(&mut self, record: &Value) {
        let notice = match (
            record["type"].as_str(),
            record["attachment"]["type"].as_str(),
        ) {
            (Some("queue-operation"), _) if record["operation"] == "enqueue" => {
                record["content"].as_str()
            }
            (Some("attachment"), Some("queued_command")) => record["attachment"]["prompt"].as_str(),
            _ => None,
        };
        let Some(notice) = notice else { return };
        let Some(task) = tag(notice, "task-id") else {
            return;
        };
        let tool = tag(notice, "tool-use-id");
        if let Some(agent) = self.summary.agents.iter_mut().find(|a| match tool {
            Some(id) => a.id == id,
            None => a.agent_id.as_deref() == Some(task),
        }) && let Some(at) = timestamp(record)
        {
            agent.end = Some(agent.end.map_or(at, |old| old.min(at)));
        }
    }

    fn efforts(&mut self, transcript: &Path) -> bool {
        let mut changed = false;
        for agent in &mut self.summary.agents {
            if agent.effort_read {
                continue;
            }
            let Some(id) = agent.agent_id.as_deref().filter(|id| {
                !id.is_empty()
                    && id.len() <= 64
                    && id
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            }) else {
                continue;
            };
            let path = transcript
                .with_extension("")
                .join("subagents")
                .join(format!("agent-{id}.jsonl"));
            let Some(stamp) = Stamp::at(&path) else {
                continue;
            };
            let stamp = (stamp.len, stamp.modified);
            if agent.effort_stamp == Some(stamp) {
                continue;
            }
            let Ok(file) = File::open(path) else { continue };
            let mut bytes = Vec::new();
            if file.take(HEAD_BYTES).read_to_end(&mut bytes).is_err() {
                continue;
            }
            changed = true;
            agent.effort_stamp = Some(stamp);
            agent.effort_read = stamp.0 >= HEAD_BYTES;
            for line in bytes.split(|b| *b == b'\n') {
                if let Ok(value) = serde_json::from_slice::<Value>(line)
                    && value["type"] == "assistant"
                {
                    agent.effort_read = true;
                    agent.effort = string(&value["effort"], 32);
                    break;
                }
            }
        }
        changed
    }

    fn todo(&mut self, name: &str, id: &str, input: &Value) {
        match name {
            "TodoWrite" => {
                if let Some(items) = input["todos"].as_array() {
                    let next: Vec<_> = items
                        .iter()
                        .take(RECENT)
                        .filter_map(|v| {
                            Some(Todo {
                                content: string(&v["content"], 256)?,
                                status: task_status(&v["status"])?.into(),
                            })
                        })
                        .collect();
                    let mut claimed = Vec::new();
                    let mut moved = BTreeMap::new();
                    for (new, todo) in next.iter().enumerate() {
                        if let Some((old, _)) = self
                            .summary
                            .todos
                            .iter()
                            .enumerate()
                            .find(|(i, t)| t.content == todo.content && !claimed.contains(i))
                        {
                            claimed.push(old);
                            moved.insert(old, new);
                        }
                    }
                    self.move_tasks(|i| moved.get(&i).copied());
                    self.summary.todos = next;
                }
            }
            "TaskCreate" => {
                self.next_task += 1;
                if self.summary.todos.len() >= RECENT {
                    return;
                }
                let index = self.summary.todos.len();
                self.summary.todos.push(Todo {
                    content: string(&input["subject"], 256)
                        .or_else(|| string(&input["description"], 256))
                        .unwrap_or_else(|| "Untitled task".into()),
                    status: task_status(&input["status"]).unwrap_or("pending").into(),
                });
                let ids = scalar(&input["taskId"]).map_or_else(
                    || vec![self.next_task.to_string(), id.into()],
                    |id| vec![id],
                );
                for id in ids {
                    if self.task_index.len() < RECENT * 2 {
                        self.task_index.insert(id, Some(index));
                    }
                }
            }
            "TaskUpdate" => {
                let Some(key) = scalar(&input["taskId"]) else {
                    return;
                };
                let index = self.task_index.get(&key).copied().unwrap_or_else(|| {
                    key.parse::<usize>()
                        .ok()?
                        .checked_sub(1)
                        .filter(|i| *i < self.summary.todos.len())
                });
                let Some(index) = index.filter(|i| *i < self.summary.todos.len()) else {
                    return;
                };
                if input["status"] == "deleted" {
                    self.summary.todos.remove(index);
                    self.move_tasks(|i| {
                        if i == index {
                            None
                        } else {
                            Some(i - usize::from(i > index))
                        }
                    });
                    return;
                }
                let todo = &mut self.summary.todos[index];
                if let Some(status) = task_status(&input["status"]) {
                    todo.status = status.into();
                }
                if let Some(content) =
                    string(&input["subject"], 256).or_else(|| string(&input["description"], 256))
                {
                    todo.content = content;
                }
            }
            _ => {}
        }
    }

    fn move_tasks(&mut self, to: impl Fn(usize) -> Option<usize>) {
        for index in self.task_index.values_mut() {
            *index = index.and_then(&to);
        }
    }

    fn hook(&mut self, attachment: &Value) {
        let failed = match attachment["type"].as_str() {
            Some("hook_success") => false,
            Some("hook_non_blocking_error" | "hook_blocking_error") => true,
            _ => return,
        };
        let Some(event) = attachment["hookEvent"].as_str() else {
            return;
        };
        let name = attachment["command"]
            .as_str()
            .map(basename)
            .filter(|s| !s.is_empty())
            .or_else(|| string(&attachment["hookName"], 64));
        let Some(name) = name else { return };
        let hooks = &mut self.summary.hooks;
        hooks.last = Some(name);
        hooks.last_failed = failed;
        hooks.failed += usize::from(failed);
        increment(&mut hooks.counts, short_event(event));
    }

    fn accumulate_agy_tokens(&mut self, record: &Value, update_context: bool) {
        let inp = non_neg_u64(&record["input_tokens"]);
        let cr = non_neg_u64(&record["cache_read_tokens"]);
        let cw = non_neg_u64(
            record
                .get("cache_creation_tokens")
                .or_else(|| record.get("cache_creation_input_tokens"))
                .unwrap_or(&Value::Null),
        );
        let out = non_neg_u64(&record["output_tokens"]);
        let total = inp + cr + cw + out;
        if total > 0 {
            self.summary.session_tokens.input += inp;
            self.summary.session_tokens.cache_read += cr;
            self.summary.session_tokens.cache_write += cw;
            self.summary.session_tokens.output += out;
            if let Some(model) = record["model"].as_str()
                && let Some(p) = crate::models::builtin::price_at(
                    model,
                    false,
                    inp + cr + cw,
                    timestamp(record).unwrap_or(0),
                )
            {
                let usd = (inp as f64 * p.input
                    + cr as f64 * p.cache_read
                    + cw as f64 * p.cache_write_5m
                    + out as f64 * p.output)
                    / 1_000_000.0;
                self.summary.estimated_cost_micros = self
                    .summary
                    .estimated_cost_micros
                    .saturating_add((usd * 1_000_000.0).round() as u64);
            }
        }
        let ctx = inp + cr + cw;
        if update_context && ctx > 0 {
            self.summary.context_tokens = Some(ctx);
        }
    }

    fn agy_planner(&mut self, record: &Value) {
        let at = timestamp(record).unwrap_or(0);
        self.flush_agy(Status::Completed, at);
        if let Some(t) = timestamp(record) {
            self.summary.last_reply = Some(t);
        }
        if let Some(model) = string(&record["model"], 128) {
            self.summary.last_model = Some(model);
        }
        self.accumulate_agy_tokens(record, true);

        let Some(calls) = record["tool_calls"].as_array() else {
            return;
        };
        let step_tag = record["step_index"]
            .as_u64()
            .map_or_else(|| self.next_seq.to_string(), |n| n.to_string());

        for call in calls {
            let Some(raw_name) = call["name"].as_str().filter(|s| !s.is_empty()) else {
                continue;
            };
            self.next_seq += 1;
            let call_id = string(&call["id"], 128)
                .unwrap_or_else(|| format!("agy-{step_tag}-{}", self.next_seq));
            let args = call["args"].as_object();
            let lower = raw_name.to_ascii_lowercase();

            if matches!(lower.as_str(), "invoke_subagent" | "task" | "agent") {
                let kind = agy_pick_arg(args, &["TypeName", "subagent_type", "Role"], 32)
                    .unwrap_or_else(|| "subagent".into());
                let model = agy_pick_arg(args, &["Model", "model"], 64)
                    .filter(|m| !m.eq_ignore_ascii_case("inherit"));
                let description =
                    agy_pick_arg(args, &["Role", "description", "Prompt", "prompt"], 128)
                        .unwrap_or_default();
                self.summary.agent_count += 1;
                self.summary.agents.push(Agent {
                    id: call_id.clone(),
                    kind,
                    description,
                    model,
                    start: at,
                    ..Agent::default()
                });
                cap(&mut self.summary.agents, AGENTS);
                self.pending_agy.push(AgyPending {
                    kind: AgyPendingKind::Agent,
                    id: call_id,
                    mcp_server: None,
                });
                cap(&mut self.pending_agy, RECENT);
                continue;
            }

            if matches!(lower.as_str(), "skill" | "use_skill") {
                if let Some(skill) = agy_pick_arg(args, &["skill", "name", "skill_name"], 64) {
                    self.summary.skills.count += 1;
                    self.pending.push(Pending {
                        id: call_id.clone(),
                        skill: Some(skill),
                    });
                    cap(&mut self.pending, RECENT);
                    self.pending_agy.push(AgyPending {
                        kind: AgyPendingKind::Skill,
                        id: call_id,
                        mcp_server: None,
                    });
                    cap(&mut self.pending_agy, RECENT);
                }
                continue;
            }

            if lower == "update_steps" {
                if let Some(steps) = args
                    .and_then(|a| a.get("steps"))
                    .and_then(agy_parse_json_array)
                {
                    let todos: Vec<Todo> = steps
                        .into_iter()
                        .take(RECENT)
                        .filter_map(|item| {
                            Some(Todo {
                                content: string(&item["title"], 256)?,
                                status: task_status(&item["status"])?.into(),
                            })
                        })
                        .collect();
                    self.summary.todos = todos;
                }
                self.pending_agy.push(AgyPending {
                    kind: AgyPendingKind::Tool,
                    id: format!("skip-{}", self.next_seq),
                    mcp_server: None,
                });
                cap(&mut self.pending_agy, RECENT);
                continue;
            }

            if matches!(lower.as_str(), "read_file" | "view_file" | "read")
                && let Some(path) = agy_pick_arg(
                    args,
                    &["Path", "file_path", "path", "TargetFile", "AbsolutePath"],
                    256,
                )
                && let Some(skill) = extract_skill_from_path(&path)
            {
                self.summary.skills.count += 1;
                self.summary.skills.done.push(skill);
                cap(&mut self.summary.skills.done, 4);
            }

            let mcp_server = extract_mcp_server(raw_name);
            if let Some(ref srv) = mcp_server
                && !self.summary.mcp_servers.contains(srv)
            {
                self.summary.mcp_servers.push(srv.clone());
            }

            let name = clean(raw_name, 128);
            let target = agy_tool_target(&lower, args);
            self.summary.tool_calls.count += 1;
            self.summary.tool_calls.recent.push(Tool {
                id: call_id.clone(),
                name,
                target,
                status: Status::Running,
                start: at,
            });
            cap(&mut self.summary.tool_calls.recent, RECENT);
            self.pending_agy.push(AgyPending {
                kind: AgyPendingKind::Tool,
                id: call_id,
                mcp_server,
            });
            cap(&mut self.pending_agy, RECENT);
        }
    }

    fn resolve_agy_pending(&mut self, item: AgyPending, status: Status, at: i64) {
        match item.kind {
            AgyPendingKind::Tool => {
                if let Some(i) = self
                    .summary
                    .tool_calls
                    .recent
                    .iter()
                    .position(|t| t.id == item.id && t.status == Status::Running)
                {
                    let mut tool = self.summary.tool_calls.recent.remove(i);
                    tool.status = status;
                    let failed = status == Status::Error;
                    self.summary.tool_calls.last = Some((tool.name.clone(), failed));
                    increment(&mut self.summary.tool_calls.counts, tool.name.clone());
                    self.summary.tool_calls.recent.push(tool);
                }
                if let Some(srv) = item.mcp_server {
                    if status == Status::Error {
                        if !self.summary.mcp_errors.contains(&srv) {
                            self.summary.mcp_errors.push(srv);
                        }
                    } else {
                        self.summary.mcp_errors.retain(|s| s != &srv);
                    }
                }
            }
            AgyPendingKind::Agent => {
                if let Some(agent) = self.summary.agents.iter_mut().find(|a| a.id == item.id)
                    && agent.end.is_none()
                {
                    agent.end = Some(at);
                }
            }
            AgyPendingKind::Skill => {
                if let Some(i) = self.pending.iter().position(|p| p.id == item.id) {
                    let pending = self.pending.remove(i);
                    if let Some(skill) = pending.skill {
                        self.summary.skills.done.push(skill);
                        cap(&mut self.summary.skills.done, 4);
                    }
                }
            }
        }
    }

    fn flush_agy(&mut self, status: Status, at: i64) {
        let items = std::mem::take(&mut self.pending_agy);
        for item in items {
            self.resolve_agy_pending(item, status, at);
        }
    }

    fn agy_generic(&mut self, record: &Value) {
        if self.pending_agy.is_empty() {
            return;
        }
        let item = if let Some(id) = record["tool_call_id"].as_str().filter(|s| !s.is_empty()) {
            match self.pending_agy.iter().position(|p| p.id == id) {
                Some(idx) => self.pending_agy.remove(idx),
                None => self.pending_agy.remove(0),
            }
        } else {
            self.pending_agy.remove(0)
        };
        let status_str = record["status"].as_str().unwrap_or("");
        if status_str == "RUNNING" {
            if let Some(step) = record["step_index"].as_i64() {
                self.running_bg.insert(step, item);
            }
            return;
        }
        let outcome = if status_str == "ERROR" {
            Status::Error
        } else {
            Status::Completed
        };
        self.resolve_agy_pending(item, outcome, timestamp(record).unwrap_or(0));
    }

    fn agy_system_message(&mut self, record: &Value) {
        let Some(content) = record["content"].as_str() else {
            return;
        };
        let Some((_, after)) = content.split_once("task-") else {
            return;
        };
        let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
        let Ok(step_idx) = digits.parse::<i64>() else {
            return;
        };
        let lower = content.to_ascii_lowercase();
        let outcome = if lower.contains("finished")
            || lower.contains("completed")
            || lower.contains("done")
        {
            Status::Completed
        } else if lower.contains("failed")
            || lower.contains("error")
            || lower.contains("killed")
            || lower.contains("stopped")
            || lower.contains("cancel")
        {
            Status::Error
        } else {
            return;
        };
        if let Some(item) = self.running_bg.remove(&step_idx) {
            self.resolve_agy_pending(item, outcome, timestamp(record).unwrap_or(0));
        }
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

fn non_neg_u64(v: &Value) -> u64 {
    v.as_u64().unwrap_or(0)
}

fn decode_agy_arg(raw: &Value) -> Option<String> {
    if let Some(s) = raw.as_str() {
        let trimmed = s.trim();
        if ((trimmed.starts_with('"') && trimmed.ends_with('"'))
            || matches!(trimmed, "null" | "true" | "false"))
            && let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
        {
            return match parsed {
                Value::String(inner) => Some(inner),
                Value::Number(n) => Some(n.to_string()),
                Value::Bool(b) => Some(b.to_string()),
                _ => None,
            };
        }
        return Some(s.to_string());
    }
    if raw.is_number() || raw.is_boolean() {
        return Some(raw.to_string());
    }
    None
}

fn agy_pick_arg(
    args: Option<&serde_json::Map<String, Value>>,
    keys: &[&str],
    limit: usize,
) -> Option<String> {
    let args = args?;
    for key in keys {
        if let Some(raw) = args.get(*key)
            && let Some(val) = decode_agy_arg(raw)
        {
            let first = val.lines().next().unwrap_or("");
            let cleaned = clean(first, limit);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn agy_parse_json_array(raw: &Value) -> Option<Vec<Value>> {
    if let Some(arr) = raw.as_array() {
        return Some(arr.clone());
    }
    if let Some(s) = raw.as_str()
        && let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(s)
    {
        return Some(arr);
    }
    None
}

fn extract_mcp_server(raw_name: &str) -> Option<String> {
    let rest = raw_name.strip_prefix("mcp__")?;
    let raw = rest.split("__").next().unwrap_or(rest);
    let cleaned = clean(raw, 40);
    (!cleaned.is_empty()).then_some(cleaned)
}

fn extract_skill_from_path(raw_path: &str) -> Option<String> {
    let norm = raw_path.replace('\\', "/");
    if !norm.to_ascii_lowercase().ends_with("/skill.md") {
        return None;
    }
    if let Some((_, after)) = norm.split_once("/skills/") {
        let name = after.split('/').next().unwrap_or("");
        let cleaned = clean(name, 40);
        if !cleaned.is_empty() {
            return Some(cleaned);
        }
    }
    let parts: Vec<&str> = norm.split('/').filter(|s| !s.is_empty()).collect();
    if parts.len() >= 2 {
        let parent = parts[parts.len() - 2];
        if !parent.eq_ignore_ascii_case("skills") {
            let cleaned = clean(parent, 40);
            if !cleaned.is_empty() {
                return Some(cleaned);
            }
        }
    }
    None
}

fn agy_tool_target(lower_name: &str, args: Option<&serde_json::Map<String, Value>>) -> String {
    if matches!(
        lower_name,
        "read_file"
            | "write_file"
            | "edit_file"
            | "view_file"
            | "write_to_file"
            | "replace_file_content"
            | "multi_replace_file_content"
            | "read"
            | "write"
            | "edit"
    ) {
        if let Some(p) = agy_pick_arg(
            args,
            &["Path", "TargetFile", "AbsolutePath", "file_path", "path"],
            128,
        ) {
            let norm = p.replace('\\', "/");
            return clean(norm.rsplit('/').next().unwrap_or(&norm), 28);
        }
        return String::new();
    }
    if matches!(lower_name, "run_command" | "bash") {
        return agy_pick_arg(args, &["CommandLine", "command", "toolSummary"], 40)
            .unwrap_or_default();
    }
    if matches!(
        lower_name,
        "search_web" | "grep_search" | "find_by_name" | "glob" | "grep"
    ) {
        return agy_pick_arg(
            args,
            &[
                "query",
                "Query",
                "Pattern",
                "pattern",
                "SearchPath",
                "SearchDirectory",
                "domain",
                "path",
            ],
            32,
        )
        .unwrap_or_default();
    }
    agy_pick_arg(
        args,
        &[
            "Path",
            "file_path",
            "command",
            "query",
            "Pattern",
            "pattern",
            "url",
            "Recipient",
            "Action",
        ],
        28,
    )
    .unwrap_or_default()
}

fn string(value: &Value, limit: usize) -> Option<String> {
    let text = clean(value.as_str()?, limit);
    (!text.is_empty()).then_some(text)
}

fn scalar(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(|s| clean(s, 128))
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

fn task_status(value: &Value) -> Option<&'static str> {
    match value.as_str()? {
        "pending" | "not_started" => Some("pending"),
        "in_progress" | "running" => Some("in_progress"),
        "completed" | "complete" | "done" => Some("completed"),
        _ => None,
    }
}

fn tag<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    text.split_once(&format!("<{name}>"))?
        .1
        .split_once(&format!("</{name}>"))
        .map(|(value, _)| value)
}

fn target(name: &str, input: &Value) -> String {
    match name {
        "Read" | "Write" | "Edit" => {
            let path = input["file_path"]
                .as_str()
                .or_else(|| input["path"].as_str())
                .unwrap_or_default()
                .replace('\\', "/");
            clean(path.rsplit('/').next().unwrap_or_default(), 128)
        }
        "Glob" | "Grep" => string(&input["pattern"], 128).unwrap_or_default(),
        "Bash" => {
            let command = clean(input["command"].as_str().unwrap_or_default(), 256);
            if command.chars().count() > 30 {
                format!(
                    "{}...",
                    command.chars().take(30).collect::<String>().trim_end()
                )
            } else {
                command
            }
        }
        _ => String::new(),
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
    fn tool_status_targets_exclusions_and_completion_order() {
        let records = [
            json!({"type":"assistant", "message":{"content":[
                {"type":"tool_use","id":"bash","name":"Bash","input":{"command":"  cargo   test\n --package crate --all-targets --features extra"}},
                {"type":"tool_use","id":"edit","name":"Edit","input":{"file_path":"C:\\repo\\lib.rs"}},
                {"type":"tool_use","id":"mcp","name":"mcp__docs__search","input":{}},
                {"type":"tool_use","id":"skill","name":"Skill","input":{"skill":"review"}},
                {"type":"tool_use","id":"list","name":"TaskList"},
                {"type":"tool_use","id":"get","name":"TaskGet"}
            ]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"edit"}]}}),
            json!({"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"bash","is_error":true}]}}),
        ];
        let summary = Fixture::new().summary(&lines(&records));
        let tools = summary.tool_calls;
        assert_eq!(tools.count, 3);
        assert_eq!(
            tools
                .recent
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            ["mcp", "edit", "bash"]
        );
        assert_eq!(
            tools.counts,
            [("Bash".into(), 1), ("Edit".into(), 1)].into()
        );
        assert_eq!(
            tools.recent.iter().find(|t| t.id == "bash").unwrap().status,
            Status::Error
        );
        assert_eq!(
            tools.recent.iter().find(|t| t.id == "bash").unwrap().target,
            "cargo test --package crate --a..."
        );
        assert_eq!(
            tools.recent.iter().find(|t| t.id == "edit").unwrap().target,
            "lib.rs"
        );
        assert_eq!(
            tools.recent.iter().find(|t| t.id == "mcp").unwrap().status,
            Status::Running
        );
        assert_eq!(summary.skills.running, ["review"]);
    }

    #[test]
    fn agent_launch_completion_model_and_effort_survive_cached_renders() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        let records = [
            json!({"type":"assistant", "timestamp":"1970-01-01T00:01:00Z", "message":{"content":[
                {"type":"tool_use","id":"agent","name":"Agent","input":{"model":"haiku", "subagent_type":"Explore", "description":"scan the crate", "run_in_background":true}}
            ]}}),
            json!({"type":"user", "timestamp":"1970-01-01T00:01:01Z", "toolUseResult":{"agentId":"abc-1","resolvedModel":"claude-haiku-4-5-20251001","isAsync":true}, "message":{"content":[
                {"type":"tool_result","tool_use_id":"agent"}
            ]}}),
        ];
        let subagents = f.0.join("session/subagents");
        std::fs::create_dir_all(&subagents).unwrap();
        let effort = subagents.join("agent-abc-1.jsonl");
        std::fs::write(
            &effort,
            lines(&[
                json!({"type":"user","effort":"wrong"}),
                json!({"type":"assistant","agentId":"abc-1","effort":"high"}),
                json!({"type":"assistant","effort":"low"}),
            ]),
        )
        .unwrap();
        std::fs::write(&path, lines(&records)).unwrap();
        let first = read_at(&path, &cache).unwrap();
        let agent = &first.agents[0];
        assert_eq!(agent.end, None);
        assert_eq!(agent.start, 60);
        assert_eq!(agent.model.as_deref(), Some("claude-haiku-4-5-20251001"));
        assert_eq!(agent.effort.as_deref(), Some("high"));
        let input = crate::input::Input::default();
        let registry = crate::models::Registry::default();
        let ctx = crate::metrics::Ctx::new(
            &input,
            &registry,
            122,
            chrono::FixedOffset::east_opt(0).unwrap(),
            crate::style::GlyphTier::Unicode,
        )
        .with_transcript(crate::metrics::TranscriptSource::Fixed(Some(first.clone())));
        let metric = crate::metrics::find("activity.agents").unwrap();
        let reading = crate::metrics::read(metric, &ctx, &crate::metrics::Options::new()).unwrap();
        assert_eq!(
            reading.get("now").unwrap().text,
            "◐ Explore [haiku-4.5 high] · scan the crate (1m 2s)"
        );
        std::fs::write(&effort, "must not reread").unwrap();
        assert_eq!(read_at(&path, &cache), Some(first));
        let notices = [
            json!({"type":"queue-operation","operation":"enqueue","timestamp":"1970-01-01T00:02:02Z","content":"<task-id>abc-1</task-id><status>completed</status>"}),
            json!({"type":"attachment","timestamp":"1970-01-01T00:02:03Z","attachment":{"type":"queued_command","prompt":"<task-id>abc-1</task-id><tool-use-id>agent</tool-use-id>"}}),
        ];
        File::options()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(lines(&notices).as_bytes())
            .unwrap();
        let done = read_at(&path, &cache).unwrap();
        assert_eq!(done.agent_count, 1);
        assert_eq!(done.agents[0].end, Some(122));
        assert_eq!(done.agents[0].effort.as_deref(), Some("high"));
        assert_eq!(read_at(&path, &cache), Some(done));
    }

    #[test]
    fn effort_follows_a_large_prompt_snapshot() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        std::fs::write(&path, lines(&[
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Agent"}]}}),
            json!({"type":"user","toolUseResult":{"agentId":"big"},"message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}),
        ])).unwrap();
        let dir = f.0.join("session/subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let snapshot = json!({"type":"prompt_snapshot","systemPrompt":["x".repeat(190 * 1024)]});
        std::fs::write(
            dir.join("agent-big.jsonl"),
            lines(&[snapshot, json!({"type":"assistant","effort":"high"})]),
        )
        .unwrap();
        let summary = read_at(&path, &f.0.join("cache")).unwrap();
        assert_eq!(summary.agents[0].effort.as_deref(), Some("high"));
    }

    #[test]
    fn effort_head_is_bounded_and_late_first_reply_is_cached() {
        let f = Fixture::new();
        let path = f.0.join("session.jsonl");
        let cache = f.0.join("cache");
        std::fs::write(&path, lines(&[
            json!({"type":"assistant","message":{"content":[{"type":"tool_use","id":"a","name":"Agent"},{"type":"tool_use","id":"b","name":"Agent"}]}}),
            json!({"type":"user","toolUseResult":{"agentId":"late"},"message":{"content":[{"type":"tool_result","tool_use_id":"a"}]}}),
            json!({"type":"user","toolUseResult":{"agentId":"large"},"message":{"content":[{"type":"tool_result","tool_use_id":"b"}]}}),
        ])).unwrap();
        let dir = f.0.join("session/subagents");
        std::fs::create_dir_all(&dir).unwrap();
        let late = dir.join("agent-late.jsonl");
        std::fs::write(&late, "{\"type\":\"user\"}\n").unwrap();
        std::fs::write(
            dir.join("agent-large.jsonl"),
            " ".repeat(HEAD_BYTES as usize) + "\n{\"type\":\"assistant\",\"effort\":\"low\"}\n",
        )
        .unwrap();
        let first = read_at(&path, &cache).unwrap();
        assert!(first.agents.iter().all(|a| a.effort.is_none()));
        File::options()
            .append(true)
            .open(late)
            .unwrap()
            .write_all(b"{\"type\":\"assistant\",\"effort\":\"high\"}\n")
            .unwrap();
        let next = read_at(&path, &cache).unwrap();
        assert_eq!(next.agents[0].effort.as_deref(), Some("high"));
        assert!(next.agents[1].effort.is_none());
        let cache_file = cache.join(format!(
            "{}.json",
            super::super::file_key(&path.to_string_lossy())
        ));
        let modified = cache_file.metadata().unwrap().modified().unwrap();
        assert_eq!(read_at(&path, &cache), Some(next));
        assert_eq!(cache_file.metadata().unwrap().modified().unwrap(), modified);
    }

    #[test]
    fn async_result_alone_keeps_agent_running_and_sync_task_finishes() {
        for (input, result, end) in [
            (json!({}), json!({"status":"async_launched"}), None),
            (json!({}), json!({"isAsync":true}), None),
            (json!({"run_in_background":true}), json!({}), None),
            (json!({}), json!({}), Some(2)),
        ] {
            let summary = Fixture::new().summary(&lines(&[
                json!({"type":"assistant","timestamp":"1970-01-01T00:00:01Z","message":{"content":[{"type":"tool_use","id":"task","name":"Task","input":input}]}}),
                json!({"type":"user","timestamp":"1970-01-01T00:00:02Z","toolUseResult":result,"message":{"content":[{"type":"tool_result","tool_use_id":"task"}]}}),
            ]));
            assert_eq!(summary.agents[0].end, end);
            assert_eq!(summary.tool_calls.count, 0);
        }
    }

    #[test]
    fn todo_replacement_retains_matching_task_ids_and_retires_dropped_ids() {
        let mut scan = Scan::default();
        scan.tool(
            &json!({"type":"tool_use","id":"a","name":"TaskCreate","input":{"subject":"keep"}}),
            0,
        );
        scan.tool(
            &json!({"type":"tool_use","id":"b","name":"TaskCreate","input":{"subject":"drop"}}),
            0,
        );
        scan.tool(&json!({"type":"tool_use","id":"c","name":"TaskUpdate","input":{"taskId":"1","status":"in_progress"}}), 0);
        assert_eq!(scan.summary.todos[0].status, "in_progress");
        scan.tool(&json!({"type":"tool_use","id":"d","name":"TodoWrite","input":{"todos":[{"content":"new","status":"pending"},{"content":"keep","status":"pending"}]}}), 0);
        scan.tool(&json!({"type":"tool_use","id":"e","name":"TaskUpdate","input":{"taskId":"2","status":"completed"}}), 0);
        assert!(scan.summary.todos.iter().all(|t| t.status == "pending"));
        scan.tool(&json!({"type":"tool_use","id":"f","name":"TaskUpdate","input":{"taskId":"1","subject":"kept","status":"completed"}}), 0);
        assert_eq!(
            scan.summary.todos[1],
            Todo {
                content: "kept".into(),
                status: "completed".into()
            }
        );
        scan.tool(&json!({"type":"tool_use","id":"g","name":"TaskUpdate","input":{"taskId":"1","status":"deleted"}}), 0);
        assert_eq!(scan.summary.todos.len(), 1);
        assert_eq!(scan.summary.todos[0].content, "new");
        assert_eq!(scan.summary.tool_calls.count, 0);
    }

    #[test]
    fn hook_commands_use_basename_and_short_event_names() {
        assert_eq!(short_event("CustomToolUse"), "CustomToolUse");
        let summary = Fixture::new().summary(&lines(&[
            json!({"type":"attachment","attachment":{"type":"hook_success","hookEvent":"PostToolUse","hookName":"fallback","command":"/tmp/hooks/format.sh"}}),
            json!({"type":"attachment","attachment":{"type":"hook_blocking_error","hookEvent":"PostToolUseFailure","hookName":"fallback","command":"C:\\hooks\\check.py"}}),
        ]));
        assert_eq!(summary.hooks.last.as_deref(), Some("check"));
        assert_eq!(
            summary.hooks.counts,
            [("Post".into(), 1), ("PostFailure".into(), 1)].into()
        );
        assert!(summary.hooks.last_failed);
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
        assert_eq!(summary.tool_calls.count, 0);
        assert!(summary.tool_calls.counts.is_empty());
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
        assert_eq!(hooks.counts, [("Pre".into(), 2), ("Stop".into(), 1)].into());
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
        assert_eq!(first.tool_calls.count, 1);
        assert_eq!(first.tool_calls.last, None);
        assert_eq!(read_at(&path, &cache), Some(first));
        let mut file = File::options().append(true).open(&path).unwrap();
        writeln!(file, r#"{{"content":[{{"type":"tool_result","tool_use_id":"skill"}},{{"type":"tool_result","tool_use_id":"bash","is_error":true}}]}}}}"#).unwrap();
        let second = read_at(&path, &cache).unwrap();
        assert_eq!(second.start, Some(60));
        assert!(second.skills.running.is_empty());
        assert_eq!(second.skills.done, ["review"]);
        assert_eq!(second.skills.count, 1);
        assert_eq!(second.tool_calls.count, 1);
        assert_eq!(second.tool_calls.last, Some(("Bash".into(), true)));
        assert_eq!(second.hooks.counts["Pre"], 1);
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
        assert_eq!(second.tool_calls.count, 0);
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
        assert_eq!(last.tool_calls.count, 0);
        assert_eq!(last.tool_calls.last, None);
        assert!(last.skills.running.is_empty());
        assert_eq!(last.skills.done, ["review"]);
    }

    #[test]
    fn bounds_recent_calls_and_persists_session_totals() {
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
        assert!(cache_path.metadata().unwrap().len() < CACHE_BYTES);
        assert_eq!(
            read_at(&path, &cache).unwrap().tool_calls.recent.len(),
            RECENT
        );
        let second = read_at(&path, &cache).unwrap();
        assert_eq!(second.tool_calls.count, 2200);
        assert_eq!(second.tool_calls.last, None);
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

    #[test]
    fn parses_antigravity_transcript_compactions_tokens_tools_and_annotations() {
        let fixture = Fixture::new();
        let jsonl = concat!(
            r#"{"type":"USER_INPUT","timestamp":"2026-10-01T00:00:00Z","content":"hello"}"#,
            "\n",
            r#"{"type":"PLANNER_RESPONSE","step_index":1,"timestamp":"2026-10-01T00:00:05Z","model":"claude-opus-4-6-thinking","input_tokens":1200,"cache_read_tokens":4000,"cache_creation_tokens":300,"output_tokens":250,"tool_calls":[{"id":"c1","name":"run_command","args":{"CommandLine":"cargo test"}},{"id":"c2","name":"update_steps","args":{"steps":[{"title":"Write tests","status":"in_progress"}]}},{"id":"c3","name":"invoke_subagent","args":{"TypeName":"research","Role":"Inspector"}},{"id":"c4","name":"read_file","args":{"Path":"/home/u/.agents/skills/tdd/SKILL.md"}}]}"#,
            "\n",
            r#"{"type":"GENERIC","tool_call_id":"c1","status":"SUCCESS","timestamp":"2026-10-01T00:00:08Z"}"#,
            "\n",
            r#"{"type":"CHECKPOINT","timestamp":"2026-10-01T00:01:00Z","model":"claude-opus-4-6-thinking","input_tokens":100,"cache_read_tokens":200,"output_tokens":50,"userIntent":"compacted"}"#,
            "\n"
        );
        let s = fixture.summary(jsonl);
        assert_eq!(s.compactions, 1);
        assert_eq!(s.last_model.as_deref(), Some("claude-opus-4-6-thinking"));
        assert_eq!(s.context_tokens, Some(5_500));
        assert_eq!(
            s.session_tokens,
            SessionTokens {
                input: 1_300,
                output: 300,
                cache_read: 4_200,
                cache_write: 300,
            }
        );
        assert!(s.estimated_cost_micros > 0);
        assert_eq!(s.todos.len(), 1);
        assert_eq!(s.todos[0].content, "Write tests");
        assert_eq!(s.todos[0].status, "in_progress");
        assert_eq!(s.agent_count, 1);
        assert_eq!(s.agents[0].kind, "research");
        assert_eq!(s.skills.done, vec!["tdd"]);
        assert_eq!(s.tool_calls.count, 2); // run_command + read_file
        assert_eq!(s.tool_calls.counts.get("run_command"), Some(&1));
    }

    #[test]
    fn parses_model_selection_without_trailing_instruction_text() {
        let content = "<USER_SETTINGS_CHANGE>\nThe user changed setting `Model Selection` from None to Gemini 4 Carbon (High). No need to comment on this change if the user doesn't ask about it. If reporting what model you are, please use a human readable name instead of the exact string.\n</USER_SETTINGS_CHANGE>";
        assert_eq!(
            parse_selected_model(content).as_deref(),
            Some("Gemini 4 Carbon (High)")
        );
    }
}
