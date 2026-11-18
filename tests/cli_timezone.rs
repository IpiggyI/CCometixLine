use chrono::{DateTime, FixedOffset, Utc};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_ccometixline")
}

fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ccline-cli-tz-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn run_statusline(home: &std::path::Path, extra_tz: Option<&str>, payload: &str) -> String {
    let mut cmd = Command::new(bin());
    cmd.env("CCLINE_HOME", home)
        .env_remove("CCLINE_TZ")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(tz) = extra_tz {
        cmd.env("CCLINE_TZ", tz);
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(payload.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap()
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
    let payload = serde_json::json!({
        "session_id": "cli-tz-test",
        "version": "2.1.305",
        "cost": { "total_cost_usd": 1.25, "total_duration_ms": 60000.0 },
        "rate_limits": {
            "five_hour": { "used_percentage": 20.0, "resets_at": reset_at as f64 }
        }
    })
    .to_string();

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
    let payload = serde_json::json!({
        "version": "2.1.305",
        "rate_limits": { "five_hour": { "used_percentage": 20.0, "resets_at": reset_at as f64 } }
    })
    .to_string();

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

    // Invalid timezone in config.toml preserves lines and appends `! ccline tz`.
    std::fs::write(
        home.join("config.toml"),
        config.replace("timezone = \"auto\"", "timezone = \"Mars/Phobos\""),
    )
    .unwrap();
    let stdout_bad = run_statusline(&home, None, &payload);
    assert!(stdout_bad.contains("! ccline tz"), "{stdout_bad:?}");

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
            .env("CCLINE_HOME", &home)
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
        .env("CCLINE_HOME", &home)
        .args(["preview", "--tz", "Mars/Phobos"])
        .output()
        .unwrap();
    assert!(!bad.status.success());
    let stderr = String::from_utf8_lossy(&bad.stderr);
    assert!(stderr.contains("invalid timezone"), "{stderr}");

    std::fs::remove_dir_all(&home).unwrap();
}
