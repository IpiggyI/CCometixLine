use chrono::{DateTime, FixedOffset, Utc};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_codexline")
}

fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccline-cli-tz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The footer text codexline draws for one Codex snapshot, read over the
/// provider's MCP protocol.
fn run_statusline(home: &std::path::Path, extra_tz: Option<&str>, snapshot: &str) -> String {
    let mut cmd = Command::new(bin());
    cmd.env("CODEXLINE_HOME", home)
        .env_remove("CCLINE_TZ")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(tz) = extra_tz {
        cmd.env("CCLINE_TZ", tz);
    }
    let snapshot: serde_json::Value = serde_json::from_str(snapshot).unwrap();
    let read = serde_json::json!({
        "jsonrpc": "2.0", "id": 1, "method": "resources/read",
        "params": {
            "uri": ccometixline::codex::URI,
            "_meta": { "com.openai/codex-statusline-state": snapshot },
        },
    });
    let mut child = cmd.spawn().unwrap();
    writeln!(child.stdin.take().unwrap(), "{read}").unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let reply: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let tree: serde_json::Value =
        serde_json::from_str(reply["result"]["contents"][0]["text"].as_str().unwrap()).unwrap();
    fn text(v: &serde_json::Value) -> String {
        match v {
            serde_json::Value::String(s) => s.clone(),
            _ => v["children"]
                .as_array()
                .map(|c| c.iter().map(text).collect())
                .unwrap_or_default(),
        }
    }
    // Codex keeps no-break spaces only; compare the text as it reads.
    text(&tree).replace('\u{a0}', " ")
}

/// A Codex snapshot whose five-hour window resets at `reset_at`.
fn snapshot(thread: serde_json::Value, reset_at: i64) -> String {
    serde_json::json!({
        "account": { "rate_limits": [{
            "limit_id": "codex", "limit_name": null,
            "primary_limit": { "used_percent": 20.0, "window_minutes": 300, "resets_at": reset_at },
            "secondary_limit": null,
        }]},
        "thread": thread,
    })
    .to_string()
}

#[test]
fn configured_timezone_controls_clocks_and_daily_ledger_date() {
    let home = temp_home("config");
    let config = r#"
color_depth = "none"
glyphs = "ascii"
timezone = "Asia/Shanghai"

[[line]]
left = [{ metric = "quota.5h", widget = "text", format = "{reset}", options = { reset_format = "%Y-%m-%d %H:%M" } }, "cost.today"]
"#;
    std::fs::write(home.join("config.toml"), config).unwrap();

    let bj = FixedOffset::east_opt(8 * 3600).unwrap();
    let before = Utc::now();
    let reset_at = before.timestamp() + 3600;
    let payload = snapshot(
        serde_json::json!({
            "id": "cli-tz-test",
            "usage": { "cost": { "total_cost_usd": 1.25 } },
        }),
        reset_at,
    );

    let stdout = run_statusline(&home, Some("UTC"), &payload);
    let after = Utc::now();

    let expected_reset = DateTime::from_timestamp(reset_at, 0)
        .unwrap()
        .with_timezone(&bj)
        .format("%Y-%m-%d %H:%M")
        .to_string();
    assert!(
        stdout.contains(&expected_reset),
        "expected {expected_reset} in {stdout:?}"
    );

    let ledger_text = std::fs::read_to_string(home.join("state/ledger.json")).unwrap();
    let ledger: serde_json::Value = serde_json::from_str(&ledger_text).unwrap();
    let date = ledger["date"].as_str().unwrap();
    let d1 = before.with_timezone(&bj).format("%Y-%m-%d").to_string();
    let d2 = after.with_timezone(&bj).format("%Y-%m-%d").to_string();
    assert!(date == d1 || date == d2, "unexpected ledger date {date:?}");

    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn ccline_tz_env_applies_when_config_is_auto_and_invalid_config_warns() {
    let home = temp_home("env-and-warn");
    let config = r#"
color_depth = "none"
glyphs = "ascii"
timezone = "auto"

[[line]]
left = [{ metric = "quota.5h", widget = "text", format = "{reset}", options = { reset_format = "%H:%M" } }]
"#;
    std::fs::write(home.join("config.toml"), config).unwrap();

    let reset_at = Utc::now().timestamp() + 3600;
    let payload = snapshot(serde_json::json!({}), reset_at);

    let stdout_bj = run_statusline(&home, Some("Asia/Shanghai"), &payload);
    let bj = FixedOffset::east_opt(8 * 3600).unwrap();
    let expected_bj = DateTime::from_timestamp(reset_at, 0)
        .unwrap()
        .with_timezone(&bj)
        .format("%H:%M")
        .to_string();
    assert!(
        stdout_bj.contains(&expected_bj),
        "expected {expected_bj} in {stdout_bj:?}"
    );

    // Invalid timezone in config.toml preserves lines and appends `! codexline tz`.
    std::fs::write(
        home.join("config.toml"),
        config.replace("timezone = \"auto\"", "timezone = \"Mars/Phobos\""),
    )
    .unwrap();
    let stdout_bad = run_statusline(&home, None, &payload);
    assert!(stdout_bad.contains("! codexline tz"), "{stdout_bad:?}");

    std::fs::remove_dir_all(&home).unwrap();
}

#[test]
fn preview_accepts_tz_flag_and_rejects_invalid_tz() {
    let home = temp_home("preview");
    std::fs::write(
        home.join("config.toml"),
        "[[line]]\nleft = [{ metric = \"quota.5h\", widget = \"text\", icon = \"\", format = \"{reset}\", options = { reset_format = \"%H:%M\" } }]\n",
    )
    .unwrap();
    let run = |tz: &str| {
        let out = Command::new(bin())
            .env("CODEXLINE_HOME", &home)
            .args(["preview", "-s", "normal", "--tz", tz, "--color", "none"])
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "stderr: {}",
            String::from_utf8_lossy(&out.stderr)
        );
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(
            out.stderr.is_empty(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        stdout
    };
    let utc = run("UTC");
    let bj = run("Asia/Shanghai");
    assert_ne!(utc.trim(), bj.trim());
    let parse_hhmm = |s: &str| -> u32 {
        let plain = s.replace("\x1b[0m", "");
        let trimmed = plain.trim();
        let (h, m) = trimmed.split_once(':').unwrap();
        h.parse::<u32>().unwrap() * 60 + m.parse::<u32>().unwrap()
    };
    assert_eq!((parse_hhmm(&bj) + 1440 - parse_hhmm(&utc)) % 1440, 8 * 60);

    let bad = Command::new(bin())
        .env("CODEXLINE_HOME", &home)
        .args(["preview", "--tz", "Mars/Phobos"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("invalid timezone"), "{stderr}");

    std::fs::remove_dir_all(&home).unwrap();
}
