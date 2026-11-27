use ccometixline::codex::{URI, within_limits};
use serde_json::{Value, json};
use std::io::Write;
use std::process::{Command, Stdio};

fn snapshot(model: &str) -> Value {
    json!({
        "account": {"rate_limits": []},
        "app": {"cli_version": "0.172.0", "display_mode": "standard"},
        "config": {"statusline_items": []},
        "thread": {
            "id": "00000000-0000-4000-8000-000000000001",
            "name": null,
            "agent": {"state": "ready"},
            "model": {"id": model, "display_name": "GPT-Reserve", "provider": "openai",
                      "reasoning_level": "xhigh", "service_tier": "fast"},
            "workspace": {"cwd": "/nonexistent/codexline-test"},
            "usage": {"context": {"context_window_size": 258400, "used_tokens": 80104, "used_percentage": 31}},
        },
    })
}

/// Every line the provider printed, parsed, for these request lines.
fn serve(requests: &[Value]) -> Vec<Value> {
    // The default preset at a fixed width, so the footer is laid out the
    // same with or without a terminal.
    let mut config = ccometixline::presets::default_config();
    config.layout.width = ccometixline::config::Dim(Some(120));
    serve_with(requests, &config.to_toml(), "default", |_| {})
}

/// `serve` with this config, and Codex's home prepared by `codex`.
fn serve_with(
    requests: &[Value],
    config: &str,
    tag: &str,
    codex: impl FnOnce(&std::path::Path),
) -> Vec<Value> {
    let home =
        std::env::temp_dir().join(format!("codexline-provider-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    std::fs::create_dir_all(home.join("codex")).unwrap();
    std::fs::write(home.join("config.toml"), config).unwrap();
    codex(&home.join("codex"));
    let mut child = Command::new(env!("CARGO_BIN_EXE_codexline"))
        .env("CODEXLINE_HOME", &home)
        .env("CODEX_HOME", home.join("codex"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    for r in requests {
        writeln!(stdin, "{r}").unwrap();
    }
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    let _ = std::fs::remove_dir_all(&home);
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).expect("stdout carries only JSON-RPC lines"))
        .collect()
}

fn text(tree: &Value) -> String {
    match tree {
        Value::String(s) => s.clone(),
        Value::Object(_) => tree["children"]
            .as_array()
            .map(|c| c.iter().map(text).collect())
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[test]
fn provider_draws_the_model_badge_in_codex_ui_trees() {
    let models = [
        "gpt-6-luna",
        "gpt-6.1-terra",
        "gpt-6.2-sol",
        "gpt-6.2-astra",
        "gpt-reserve",
    ];
    let mut requests = vec![
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
    ];
    for (i, m) in models.iter().enumerate() {
        requests.push(
            json!({"jsonrpc": "2.0", "id": 10 + i, "method": "resources/read", "params": {
                "uri": URI, "_meta": {"com.openai/codex-statusline-state": snapshot(m)},
            }}),
        );
    }
    let replies = serve(&requests);
    // No reply to the notification.
    assert_eq!(replies.len(), 1 + models.len());
    let expected = [
        "˙☾ · 6 luna",
        "🌏 · 6.1 terra",
        "☀ ·•• 6.2 sol",
        "──✦ 6.2 astra",
        // The registry keeps the id when the name only differs in case.
        "gpt-reserve",
    ];
    for (i, want) in expected.iter().enumerate() {
        let reply = &replies[1 + i];
        assert_eq!(reply["id"], 10 + i);
        let raw = reply["result"]["contents"][0]["text"].as_str().unwrap();
        let tree: Value = serde_json::from_str(raw).unwrap();
        assert!(within_limits(&tree), "{raw}");
        let shown = text(&tree).replace('\u{a0}', " ");
        assert!(shown.contains(want), "{shown:?} lacks {want:?}");
        assert!(shown.contains("xHigh · fast"), "{shown:?}");
        assert!(shown.contains("31% · 80.1k"), "{shown:?}");
        // The badge is a row with the navy background.
        assert!(raw.contains(r##""background":"#16161e""##), "{raw}");
        // Padded to 120 columns less Codex's indent and the slack.
        assert_eq!(
            ccometixline::render::width::display_width(&text(&tree)),
            116,
            "{shown:?}"
        );
    }
}

#[test]
fn provider_reads_the_thread_rollout() {
    // A UUIDv7 made on 2026-11-26 at 11:35:40 UTC.
    let id = "01a217a3-ce4b-7000-8000-000000000001";
    let resets = chrono::Utc::now().timestamp() + 3600;
    let records = [
        json!({"timestamp": "2026-11-26T11:36:23.206Z", "type": "session_meta", "payload": {"id": id}}),
        json!({"timestamp": "2026-11-26T11:36:30.000Z", "type": "event_msg", "payload": {
            "type": "item_completed", "started_at_ms": 1795692988000_i64, "completed_at_ms": 1795692990000_i64,
            "item": {"type": "CommandExecution", "id": "exec-1", "status": "completed",
                     "command": ["/usr/bin/zsh", "-lc", "cargo test"]}}}),
        json!({"timestamp": "2026-11-26T11:36:31.000Z", "type": "event_msg", "payload": {
            "type": "token_count", "rate_limits": {"limit_id": "codex",
                "primary": {"used_percent": 42.0, "window_minutes": 300, "resets_at": resets},
                "secondary": null}}}),
        json!({"timestamp": "2026-11-26T11:36:33.000Z", "type": "event_msg", "payload": {
            "type": "item_completed", "content_token_count": 100,
            "started_at_ms": 1795692991000_i64, "completed_at_ms": 1795692993000_i64,
            "item": {"type": "AgentMessage", "id": "m1"}}}),
        json!({"timestamp": "2026-11-26T11:36:34.000Z", "type": "event_msg", "payload": {
            "type": "task_complete", "time_to_first_token_ms": 1500}}),
    ];
    let config = r#"
color_depth = "none"
glyphs = "ascii"

[layout]
width = 200

[[line]]
left = [
  { metric = "quota.5h", widget = "text", icon = "", format = "{pct}%" },
  { metric = "timing.ttft", icon = "" },
  { metric = "timing.tps", icon = "" },
  { metric = "activity.tools", icon = "" },
]
"#;
    let mut thread = snapshot("gpt-6.2-sol")["thread"].clone();
    thread["id"] = json!(id);
    let read = json!({"jsonrpc": "2.0", "id": 1, "method": "resources/read", "params": {
        "uri": URI,
        "_meta": {"com.openai/codex-statusline-state": {"account": {"rate_limits": []}, "thread": thread}},
    }});
    let replies = serve_with(&[read], config, "rollout", |codex| {
        // The folder is the local date of creation, next to the UTC one.
        let day = codex.join("sessions/2026/11/26");
        std::fs::create_dir_all(&day).unwrap();
        let text: String = records.iter().map(|r| format!("{r}\n")).collect();
        std::fs::write(
            day.join(format!("rollout-2026-11-26T03-35-40-{id}.jsonl")),
            text,
        )
        .unwrap();
    });
    let raw = replies[0]["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    let shown = text(&serde_json::from_str(raw).unwrap()).replace('\u{a0}', " ");
    // The snapshot has no limits; the rollout's latest stand in.
    for want in ["42%", "1.5s", "50 tok/s", "Shell x1"] {
        assert!(shown.contains(want), "{shown:?} lacks {want:?}");
    }
}
