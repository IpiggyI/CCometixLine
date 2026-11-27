//! Codex CLI's status line provider protocol.
//!
//! Codex runs the provider as a stdio MCP server: newline-delimited JSON-RPC
//! on stdin and stdout. It reads the advertised resource about once a second
//! and hands over the displayed thread's snapshot in
//! `params._meta["com.openai/codex-statusline-state"]`. The reply is a UI
//! tree of `row`, `text` and `link` nodes, not ANSI text.
//!
//! A JSON-RPC error or a read slower than one second stops Codex from
//! polling until the provider is chosen again, so a render that fails or
//! runs late answers with a short message in the footer instead.
//!
//! Codex lays a tree's rows side by side and wraps them only when the next
//! one doesn't fit, and it shows three lines at most. Lines come out as rows
//! padded to the footer's width, so each one starts a line of its own.
//! Without a terminal to measure, as under Codex's background server, they
//! come out as one text node with newlines, which has no backgrounds.

use crate::config::{Config, ItemSpec, LoadNote};
use crate::env::AutoCompact;
use crate::input::Input;
use crate::metrics::{Commands, GitSource};
use crate::models::Registry;
use crate::render::ansi::sanitize;
use crate::render::span::{Line, Span, Style, line_width};
use crate::state;
use crate::statusline::{self, Frame};
use crate::style::color::Color;
use crate::style::palette::Token;
use crate::timezone;
use serde_json::{Map, Value, json};
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, mpsc};
use std::time::Duration;

pub const URI: &str = "ui://codexline/footer";
const STATE_KEY: &str = "com.openai/codex-statusline-state";
const PROTOCOL: &str = "2025-06-18";

/// Time a render may take before the read answers without it; Codex
/// allows one second per read.
const RENDER_BUDGET: Duration = Duration::from_millis(700);

/// Codex's limits on one UI tree.
const MAX_NODES: usize = 256;
const MAX_DEPTH: usize = 16;
const MAX_TEXT_BYTES: usize = 16 * 1024;
const MAX_JSON_BYTES: usize = 64 * 1024;

/// Lines Codex's footer shows; it drops the rest.
const MAX_LINES: usize = 3;
/// Columns Codex indents the footer by.
const FOOTER_INDENT: usize = 2;
/// Columns a padded line stays short of the footer: one column too many and
/// Codex cuts the line with `…`, while this little room is too narrow for
/// the next line to join it.
const SLACK: usize = 2;

/// Serve requests from stdin until Codex closes it.
pub fn serve() {
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = handle(&line, render_in_time) else {
            continue;
        };
        if writeln!(stdout, "{reply}")
            .and_then(|()| stdout.flush())
            .is_err()
        {
            break;
        }
    }
}

/// The reply to one request line; `None` for notifications, which get none.
pub fn handle(line: &str, render: impl Fn(&Value) -> Value) -> Option<Value> {
    let request: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return Some(error(Value::Null, -32700, "Parse error")),
    };
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Some(error(Value::Null, -32600, "Invalid request"));
    };
    let id = request.get("id")?.clone();
    let params = request.get("params").cloned().unwrap_or(Value::Null);
    let result = match method {
        "initialize" => json!({
            "protocolVersion": PROTOCOL,
            "capabilities": { "tools": {}, "resources": {} },
            "serverInfo": { "name": "codexline", "version": env!("CARGO_PKG_VERSION") },
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": [tool()] }),
        "tools/call" => {
            if params.get("name").and_then(Value::as_str) != Some("statusline") {
                return Some(error(id, -32602, "Unknown tool name"));
            }
            json!({ "content": [{ "type": "text", "text": format!("The statusline resource is {URI}.") }] })
        }
        "resources/list" => json!({
            "resources": [{ "name": "footer", "uri": URI, "mimeType": "application/json" }]
        }),
        "resources/templates/list" => json!({ "resourceTemplates": [] }),
        "resources/read" => {
            if params.get("uri").and_then(Value::as_str) != Some(URI) {
                return Some(error(id, -32602, "Unknown resource URI"));
            }
            let snapshot = params
                .get("_meta")
                .and_then(|m| m.get(STATE_KEY))
                .cloned()
                .unwrap_or(Value::Null);
            let tree = render(&snapshot);
            json!({ "contents": [{ "uri": URI, "mimeType": "application/json", "text": tree.to_string() }] })
        }
        _ => return Some(error(id, -32601, "Method not found")),
    };
    Some(json!({ "jsonrpc": "2.0", "id": id, "result": result }))
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn tool() -> Value {
    json!({
        "name": "statusline",
        "title": "codexline",
        "description": "Show the model, workspace, git and usage in the Codex footer.",
        "inputSchema": { "type": "object", "properties": {} },
        "_meta": { "openai/ui": { "entrypoints": [{ "type": "statusline", "resourceUri": URI }] } },
    })
}

/// Render on a worker thread so a slow or panicking render still leaves
/// time to answer.
fn render_in_time(snapshot: &Value) -> Value {
    let (tx, rx) = mpsc::channel();
    let snapshot = snapshot.clone();
    std::thread::spawn(move || {
        let _ = tx.send(render(&snapshot));
    });
    match rx.recv_timeout(RENDER_BUDGET) {
        Ok(tree) => tree,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!("codexline: render took longer than {RENDER_BUDGET:?}");
            Value::from("codexline: render timed out")
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            eprintln!("codexline: render failed (see the panic above)");
            Value::from("codexline: render failed")
        }
    }
}

/// The UI tree for one snapshot, with the user's config.
pub fn render(snapshot: &Value) -> Value {
    let rollout = snapshot["thread"]["id"].as_str().and_then(rollout);
    let raw = to_input(snapshot, rollout.as_deref());
    let text = raw.to_string();
    let input = match Input::parse(&text) {
        Ok(input) => input,
        Err(e) => {
            eprintln!("codexline: could not read the snapshot ({e})");
            return Value::from("codexline: could not read the snapshot");
        }
    };
    let (mut config, note) = Config::load();
    let width = footer_width(&config);
    if width.is_none() {
        // Without a width the lines share one text node, which has no
        // background: a badge would leave its rounded ends behind.
        drop_badges(&mut config);
    }
    let registry = Registry::load();
    let now = chrono::Utc::now().timestamp();
    let tz = config
        .timezone
        .resolve(now, timezone::detect_system_offset(now));
    let view = state::observe(&input, &text, &registry, now, tz);
    let frame = Frame {
        now,
        tz,
        width,
        git: GitSource::Cached(config.git.cache_ms),
        state: view,
        compact: AutoCompact::default(),
        commands: Commands::Run { stdin: text },
    };
    let mut lines = statusline::lines(&input, &config, &registry, frame);
    let mut warnings = Vec::new();
    if let LoadNote::Invalid(_) = note {
        warnings.push(" ! codexline config");
    }
    if config.timezone.invalid_spec().is_some() {
        warnings.push(" ! codexline tz");
    }
    if !warnings.is_empty() {
        let warn = statusline::theme(&config).palette.color(Token::Warn);
        for w in warnings {
            let mark = Span::styled(
                w,
                Style {
                    fg: warn,
                    ..Style::default()
                },
            );
            match lines.first_mut() {
                Some(first) => first.push(mark),
                None => lines.push(vec![mark]),
            }
        }
    }
    tree(&lines, width)
}

/// Columns a footer line may take: the configured or measured terminal
/// width, less Codex's indent, the slack and the configured reserve.
/// `None` without a terminal.
fn footer_width(config: &Config) -> Option<usize> {
    let columns = config
        .layout
        .width
        .0
        .map(usize::from)
        .or_else(terminal_columns)?;
    Some(columns.saturating_sub(FOOTER_INDENT + SLACK + usize::from(config.layout.reserve)))
}

/// The terminal's width, through `/dev/tty`: Codex talks to the provider
/// over pipes. When Codex's background server starts the provider, it has
/// no terminal.
fn terminal_columns() -> Option<usize> {
    let columns = ratatui::crossterm::terminal::window_size().ok()?.columns;
    (columns > 0).then_some(usize::from(columns))
}

fn drop_badges(config: &mut Config) {
    for line in &mut config.lines {
        for item in line.left.iter_mut().chain(line.right.iter_mut()) {
            if let ItemSpec::Item(item) = item {
                item.badge = None;
            }
        }
    }
}

/// The file Codex records thread `id` in:
/// `sessions/YYYY/MM/DD/rollout-<local time>-<id>.jsonl` under its home.
/// Thread ids are UUIDv7, which begin with the creation time in
/// milliseconds; the folder is named by the local date, so the days either
/// side of that UTC date are searched too. Codex creates the file with the
/// thread's first message.
fn rollout(id: &str) -> Option<PathBuf> {
    static FOUND: Mutex<Option<(String, PathBuf)>> = Mutex::new(None);
    if let Ok(found) = FOUND.lock()
        && let Some((known, path)) = found.as_ref()
        && known == id
        && path.is_file()
    {
        return Some(path.clone());
    }
    let hex: String = id.chars().filter(|c| *c != '-').collect();
    if hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let ms = i64::from_str_radix(&hex[..12], 16).ok()?;
    let day = chrono::DateTime::from_timestamp_millis(ms)?.date_naive();
    let sessions = crate::paths::codex_home().join("sessions");
    let suffix = format!("-{id}.jsonl");
    let path = [day.pred_opt(), Some(day), day.succ_opt()]
        .into_iter()
        .flatten()
        .find_map(|day| {
            std::fs::read_dir(sessions.join(day.format("%Y/%m/%d").to_string()))
                .ok()?
                .flatten()
                .map(|entry| entry.path())
                .find(|path| {
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with("rollout-") && n.ends_with(&suffix))
                })
        })?;
    if let Ok(mut found) = FOUND.lock() {
        *found = Some((id.to_owned(), path.clone()));
    }
    Some(path)
}

/// The snapshot in the shape of Claude Code's status line input, which the
/// metrics read, with the thread's `rollout` as the transcript. Codex's CLI
/// version is left out: the input's `version` is Claude Code's.
pub fn to_input(snapshot: &Value, rollout: Option<&Path>) -> Value {
    let thread = &snapshot["thread"];
    let model = &thread["model"];
    let cwd = &thread["workspace"]["cwd"];
    let context = &thread["usage"]["context"];
    let mut input = json!({
        "session_id": thread["id"],
        "session_name": thread["name"],
        "cwd": cwd,
        "workspace": { "current_dir": cwd, "project_dir": cwd },
        "model": { "id": model["id"], "display_name": model["display_name"] },
        "effort": { "level": model["reasoning_level"] },
        "context_window": {
            "context_window_size": context["context_window_size"],
            "used_percentage": context["used_percentage"],
            "total_input_tokens": context["used_tokens"],
        },
        "cost": { "total_cost_usd": thread["usage"]["cost"]["total_cost_usd"] },
    });
    // With /fast on, the snapshot reports "fast"; config.toml names the same
    // tier "priority".
    if let Some(tier) = model["service_tier"].as_str() {
        input["fast_mode"] = Value::Bool(matches!(tier, "fast" | "priority"));
    }
    if let Some(path) = rollout {
        input["transcript_path"] = Value::from(path.to_string_lossy());
    }
    // A snapshot made with an API key has no limits, while the rollout keeps
    // the latest a response reported, however old.
    let limits = rate_limits(&snapshot["account"]["rate_limits"]).or_else(|| {
        let saved = state::transcript::read(rollout?)?.codex.rate_limits?;
        rate_limits(&Value::Array(vec![saved]))
    });
    if let Some(limits) = limits {
        input["rate_limits"] = limits;
    }
    input
}

/// The five-hour and weekly windows of the first rate limit, told apart by
/// their length.
fn rate_limits(list: &Value) -> Option<Value> {
    let first = list.as_array()?.first()?;
    let mut out = Map::new();
    for window in [&first["primary_limit"], &first["secondary_limit"]] {
        let key = match window["window_minutes"].as_u64() {
            Some(300) => "five_hour",
            Some(10080) => "seven_day",
            _ => continue,
        };
        out.insert(
            key.into(),
            json!({ "used_percentage": window["used_percent"], "resets_at": window["resets_at"] }),
        );
    }
    (!out.is_empty()).then_some(Value::Object(out))
}

/// The lines as a UI tree, the first three that have something to show.
/// With the footer's `width`, each line is a row padded to it; without, all
/// lines are one text node with newlines between them. Spans with the same
/// look are merged. A tree over Codex's limits falls back to plain text.
pub fn tree(lines: &[Line], width: Option<usize>) -> Value {
    let mut shown: Vec<Line> = lines
        .iter()
        .filter(|l| l.iter().any(|s| !s.text.trim().is_empty()))
        .cloned()
        .collect();
    if shown.len() > MAX_LINES {
        shown.truncate(MAX_LINES);
        // Codex would drop the lines without a mark.
        shown[MAX_LINES - 1].push(Span::raw("…"));
    }
    let root = match (shown.len(), width) {
        (0, _) => return Value::Null,
        (1, Some(width)) => line_row(&shown[0], width),
        (_, Some(width)) => json!({
            "type": "row",
            "props": { "gap": 0, "wrap": "wrap" },
            "children": shown.iter().map(|l| line_row(l, width)).collect::<Vec<_>>(),
        }),
        (_, None) => paragraph(&shown),
    };
    if within_limits(&root) {
        return root;
    }
    let plain: Vec<String> = shown
        .iter()
        .map(|l| l.iter().map(|s| sanitize(&s.text)).collect())
        .collect();
    Value::from(spaced(&plain.join("\n")))
}

/// Codex trims and collapses ASCII spaces in text, so spaces that lay out
/// the line become no-break spaces, which it keeps.
fn spaced(text: &str) -> String {
    text.replace(' ', "\u{a0}")
}

/// One line as a row that doesn't wrap, with each run of spans on a
/// background in a row of that color, padded with no-break spaces to
/// `width` so the next line can't fit beside it.
fn line_row(line: &[Span], width: usize) -> Value {
    let mut children = Vec::new();
    let mut run: Vec<Value> = Vec::new();
    let mut run_bg: Option<Color> = None;
    let spans = merged(line);
    for span in &spans {
        let bg = span.style.bg;
        if bg != run_bg && !run.is_empty() {
            children.extend(close_run(std::mem::take(&mut run), run_bg));
        }
        run_bg = bg;
        run.push(text_node(span));
    }
    children.extend(close_run(run, run_bg));
    let pad = width.saturating_sub(line_width(&spans));
    if pad > 0 {
        children.push(Value::from("\u{a0}".repeat(pad)));
    }
    json!({ "type": "row", "props": { "gap": 0, "wrap": "nowrap" }, "children": children })
}

/// The lines in one text node with a newline between them, the one shape
/// Codex breaks lines in without knowing the width. A text node holds no
/// rows, so backgrounds and links are left out.
fn paragraph(lines: &[Line]) -> Value {
    let mut children = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            children.push(Value::from("\n"));
        }
        children.extend(merged(line).iter().map(styled_text));
    }
    json!({ "type": "text", "children": children })
}

fn close_run(run: Vec<Value>, bg: Option<Color>) -> Vec<Value> {
    match bg {
        Some(bg) if !run.is_empty() => vec![json!({
            "type": "row",
            "props": { "gap": 0, "background": color(bg), "wrap": "nowrap" },
            "children": run,
        })],
        _ => run,
    }
}

/// Adjacent spans with the same style and link joined into one.
fn merged(line: &[Span]) -> Vec<Span> {
    let mut out: Vec<Span> = Vec::new();
    for span in line {
        let text = sanitize(&span.text);
        if text.is_empty() {
            continue;
        }
        match out.last_mut() {
            Some(last) if last.style == span.style && last.link == span.link => {
                last.text.push_str(&text)
            }
            _ => out.push(Span {
                text,
                ..span.clone()
            }),
        }
    }
    out
}

fn text_node(span: &Span) -> Value {
    let node = styled_text(span);
    match span.link.as_deref().filter(|l| is_web_link(l)) {
        Some(href) => json!({ "type": "link", "props": { "href": href }, "children": [node] }),
        None => node,
    }
}

fn styled_text(span: &Span) -> Value {
    let mut props = Map::new();
    if let Some(fg) = span.style.fg {
        props.insert("color".into(), color(fg));
    }
    if span.style.bold {
        props.insert("weight".into(), "bold".into());
    }
    if span.style.italic {
        props.insert("italic".into(), true.into());
    }
    if span.style.underline {
        props.insert("underline".into(), true.into());
    }
    let mut node = json!({ "type": "text", "children": [spaced(&span.text)] });
    if !props.is_empty() {
        node["props"] = Value::Object(props);
    }
    node
}

/// Codex takes absolute HTTP(S) links only; others leave just the text.
fn is_web_link(link: &str) -> bool {
    (link.starts_with("https://") || link.starts_with("http://"))
        && link.len() <= 8 * 1024
        && !link.chars().any(char::is_control)
}

const NAMED: [&str; 8] = [
    "black", "red", "green", "yellow", "blue", "magenta", "cyan", "white",
];

/// A color Codex accepts. The first eight ANSI colors and the bright ones
/// keep their names, so they follow the terminal's theme; bright black,
/// which would turn into black, and the rest become `#rrggbb`.
fn color(c: Color) -> Value {
    let named = match c {
        Color::Ansi16(n) | Color::Ansi256(n) if n < 16 && n != 8 => Some(NAMED[n as usize % 8]),
        _ => None,
    };
    match named {
        Some(name) => Value::from(name),
        None => {
            let (r, g, b) = c.to_rgb();
            Value::from(format!("#{r:02x}{g:02x}{b:02x}"))
        }
    }
}

/// Whether the tree fits Codex's limits on nodes, depth and text.
pub fn within_limits(tree: &Value) -> bool {
    fn walk(v: &Value, depth: usize, nodes: &mut usize, text: &mut usize) -> bool {
        if v.is_null() {
            return true;
        }
        *nodes += 1;
        if depth > MAX_DEPTH {
            return false;
        }
        match v {
            Value::String(s) => {
                *text += s.len();
                true
            }
            _ => v["children"]
                .as_array()
                .is_none_or(|c| c.iter().all(|c| walk(c, depth + 1, nodes, text))),
        }
    }
    let (mut nodes, mut text) = (0, 0);
    walk(tree, 0, &mut nodes, &mut text)
        && nodes <= MAX_NODES
        && text <= MAX_TEXT_BYTES
        && tree.to_string().len() <= MAX_JSON_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(line: Value) -> Option<Value> {
        handle(&line.to_string(), |snapshot| {
            Value::from(snapshot["thread"]["id"].as_str().unwrap_or("none"))
        })
    }

    #[test]
    fn answers_requests_and_ignores_notifications() {
        assert!(call(json!({"jsonrpc": "2.0", "method": "notifications/initialized"})).is_none());
        let init = call(json!({"jsonrpc": "2.0", "id": 1, "method": "initialize"})).unwrap();
        assert_eq!(init["id"], 1);
        assert_eq!(init["result"]["serverInfo"]["name"], "codexline");
        let tools = call(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"})).unwrap();
        let entry = &tools["result"]["tools"][0]["_meta"]["openai/ui"]["entrypoints"];
        assert_eq!(*entry, json!([{"type": "statusline", "resourceUri": URI}]));
        let unknown = call(json!({"jsonrpc": "2.0", "id": 3, "method": "nope"})).unwrap();
        assert_eq!(unknown["error"]["code"], -32601);
        let garbage = handle("{", |_| Value::Null).unwrap();
        assert_eq!(garbage["error"]["code"], -32700);
    }

    #[test]
    fn read_renders_the_snapshot_it_was_given() {
        let read = |uri: &str| {
            call(
                json!({"jsonrpc": "2.0", "id": 7, "method": "resources/read", "params": {
                    "uri": uri,
                    "_meta": {STATE_KEY: {"thread": {"id": "t-1"}}},
                }}),
            )
            .unwrap()
        };
        let ok = read(URI);
        let content = &ok["result"]["contents"][0];
        assert_eq!(content["uri"], URI);
        assert_eq!(content["mimeType"], "application/json");
        assert_eq!(content["text"], "\"t-1\"");
        assert_eq!(read("ui://other")["error"]["code"], -32602);
    }

    #[test]
    fn snapshot_maps_onto_the_status_line_input() {
        let snapshot = json!({
            "app": {"cli_version": "0.172.0"},
            "account": {"rate_limits": [{
                "limit_id": "codex", "limit_name": null,
                "primary_limit": {"used_percent": 42.0, "window_minutes": 300, "resets_at": 1000},
                "secondary_limit": {"used_percent": 7.5, "window_minutes": 10080, "resets_at": 2000},
            }]},
            "thread": {
                "id": "t-1", "name": "fix it",
                "agent": {"state": "working"},
                "model": {"id": "gpt-6.2-sol", "display_name": "GPT-6.2-Sol", "provider": "openai",
                          "reasoning_level": "xhigh", "service_tier": "priority"},
                "workspace": {"cwd": "/w/fun"},
                "usage": {"context": {"context_window_size": 258400, "used_tokens": 80000, "used_percentage": 31}},
            },
        });
        let input = Input::parse(&to_input(&snapshot, None).to_string()).unwrap();
        assert_eq!(input.session_id.as_deref(), Some("t-1"));
        assert_eq!(input.version, None);
        assert_eq!(input.model_id(), Some("gpt-6.2-sol"));
        assert_eq!(input.effort.unwrap().level.as_deref(), Some("xhigh"));
        assert_eq!(input.fast_mode, Some(true));
        assert_eq!(
            input.workspace.unwrap().current_dir.as_deref(),
            Some("/w/fun")
        );
        let cw = input.context_window.unwrap();
        assert_eq!(cw.context_window_size, Some(258400));
        assert_eq!(cw.total_input_tokens, Some(80000));
        assert_eq!(cw.used_percentage, Some(31.0));
        let limits = input.rate_limits.unwrap();
        assert_eq!(limits.five_hour.unwrap().used_percentage, Some(42.0));
        assert_eq!(limits.seven_day.unwrap().resets_at, Some(2000.0));

        let normal = json!({"thread": {"model": {"service_tier": "normal"}}, "account": {"rate_limits": []}});
        let input = Input::parse(&to_input(&normal, None).to_string()).unwrap();
        assert_eq!(input.fast_mode, Some(false));
        assert!(input.rate_limits.is_none());
    }

    fn span(text: &str, fg: Option<Color>, bg: Option<Color>) -> Span {
        Span::styled(
            text,
            Style {
                fg,
                bg,
                ..Style::default()
            },
        )
    }

    #[test]
    fn tree_groups_backgrounds_and_merges_alike_spans() {
        let navy = Color::Rgb(0x1f, 0x23, 0x35);
        let line = vec![
            span("\u{e0b6}", Some(navy), None),
            span(" 6.2 ", Some(Color::Ansi16(15)), Some(navy)),
            span("sol", Some(Color::Rgb(0xe0, 0xaf, 0x68)), Some(navy)),
            span("  ", Some(Color::Ansi16(8)), None),
            span("main", Some(Color::Ansi16(8)), None),
        ];
        // As wide as the line, so no padding follows.
        let tree = tree(&[line], Some(15));
        let children = tree["children"].as_array().unwrap();
        assert_eq!(children.len(), 3);
        assert_eq!(children[0]["props"]["color"], "#1f2335");
        let badge = &children[1];
        assert_eq!(badge["type"], "row");
        assert_eq!(badge["props"]["background"], "#1f2335");
        assert_eq!(badge["children"][0]["props"]["color"], "white");
        assert_eq!(badge["children"][1]["props"]["color"], "#e0af68");
        // Bright black keeps its gray instead of turning black.
        // Spaces are no-break spaces, which Codex keeps.
        assert_eq!(children[2]["children"], json!(["\u{a0}\u{a0}main"]));
        assert_eq!(children[2]["props"]["color"], "#7f7f7f");
        assert!(within_limits(&tree));
    }

    #[test]
    fn no_ascii_space_reaches_codex() {
        // Codex trims and collapses ASCII spaces in text leaves.
        fn leaves(v: &Value, out: &mut Vec<String>) {
            match v {
                Value::String(s) => out.push(s.clone()),
                _ => v["children"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .for_each(|c| leaves(c, out)),
            }
        }
        let line = vec![
            span(" 6.2 ", None, Some(Color::Ansi16(0))),
            span("sol", None, None),
            span("   ", None, None),
            span("P  Q", None, None),
        ];
        for width in [None, Some(40)] {
            let mut text = Vec::new();
            leaves(&tree(std::slice::from_ref(&line), width), &mut text);
            assert!(text.iter().all(|t| !t.contains(' ')), "{text:?}");
            assert!(
                text.concat()
                    .starts_with("\u{a0}6.2\u{a0}sol\u{a0}\u{a0}\u{a0}P\u{a0}\u{a0}Q"),
                "{text:?}"
            );
        }
    }

    #[test]
    fn tree_keeps_web_links_only() {
        let web = span("PR #1", None, None).with_link(Some("https://example.com/pr/1".into()));
        let file = span(" ~/w", None, None).with_link(Some("file:///w".into()));
        let tree = tree(&[vec![web, file]], Some(9));
        assert_eq!(tree["children"][0]["type"], "link");
        assert_eq!(
            tree["children"][0]["props"]["href"],
            "https://example.com/pr/1"
        );
        assert_eq!(
            tree["children"][1],
            json!({"type": "text", "children": ["\u{a0}~/w"]})
        );
    }

    #[test]
    fn tree_over_the_limits_falls_back_to_text() {
        let line: Line = (0..300)
            .map(|i| span("x", Some(Color::Ansi256((i % 200) as u8 + 16)), None))
            .collect();
        let tree = tree(&[line], None);
        assert_eq!(tree, Value::from("x".repeat(300)));
        assert!(super::tree(&[], Some(80)).is_null());
    }

    fn texts(v: &Value) -> String {
        match v {
            Value::String(s) => s.clone(),
            _ => v["children"]
                .as_array()
                .map(|c| c.iter().map(texts).collect())
                .unwrap_or_default(),
        }
    }

    #[test]
    fn lines_fill_the_footer_so_codex_starts_each_on_a_new_line() {
        let navy = Some(Color::Rgb(0x16, 0x16, 0x1e));
        let lines = vec![
            vec![span(" sol ", None, navy), span(" ~/w", None, None)],
            vec![span("31%", None, None)],
            vec![],
            vec![span("🔧 3", None, None)],
            vec![span("dropped", None, None)],
        ];
        let tree = tree(&lines, Some(40));
        assert_eq!(tree["props"], json!({"gap": 0, "wrap": "wrap"}));
        let rows = tree["children"].as_array().unwrap();
        // Codex shows three lines; the empty one is skipped and a mark
        // tells that more were cut.
        assert_eq!(rows.len(), 3);
        for row in rows {
            assert_eq!(row["props"]["wrap"], "nowrap");
            let text = texts(row);
            // The last line too, or it would join the one before.
            assert_eq!(crate::render::width::display_width(&text), 40, "{text:?}");
        }
        assert!(texts(&rows[2]).starts_with("🔧\u{a0}3…"));
        assert_eq!(rows[0]["children"][0]["props"]["background"], "#16161e");
        assert!(within_limits(&tree));
    }

    #[test]
    fn without_a_width_lines_share_one_text_node() {
        let link = span("PR #1", None, None).with_link(Some("https://example.com/pr/1".into()));
        let lines = vec![
            vec![span(" sol ", None, Some(Color::Ansi16(0))), link],
            vec![span("31%", Some(Color::Ansi16(2)), None)],
        ];
        let tree = tree(&lines, None);
        assert_eq!(tree["type"], "text");
        let raw = tree.to_string();
        assert!(
            !raw.contains("\"row\"") && !raw.contains("\"link\""),
            "{raw}"
        );
        assert_eq!(texts(&tree), "\u{a0}sol\u{a0}PR\u{a0}#1\n31%");
        assert_eq!(tree["children"][2], "\n");
        assert_eq!(tree["children"][3]["props"]["color"], "green");
    }

    #[test]
    fn the_footer_width_leaves_codex_indent_and_slack() {
        let mut config = Config::default();
        config.layout.width = crate::config::Dim(Some(144));
        assert_eq!(footer_width(&config), Some(140));
        config.layout.reserve = 10;
        assert_eq!(footer_width(&config), Some(130));
    }
}
