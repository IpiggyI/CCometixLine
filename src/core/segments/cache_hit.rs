use super::{Segment, SegmentData, ThresholdLevel};
use crate::config::{InputData, NormalizedUsage, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct CacheHitSegment {
    mode: Option<String>,
    warning_below: Option<f64>,
    critical_below: Option<f64>,
}

impl CacheHitSegment {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure the display mode (`last` | `total` | `both`). Default = `last`.
    pub fn with_mode<S: Into<String>>(mut self, mode: S) -> Self {
        self.mode = Some(mode.into());
        self
    }

    /// Reverse-direction thresholds: lower hit-rate is worse.
    /// Defaults: warning_below=60%, critical_below=30%.
    pub fn with_thresholds(mut self, warning_below: f64, critical_below: f64) -> Self {
        self.warning_below = Some(warning_below);
        self.critical_below = Some(critical_below);
        self
    }

    fn mode_or_default(&self) -> &str {
        match self.mode.as_deref() {
            Some("total") => "total",
            Some("both") => "both",
            _ => "last",
        }
    }

    /// Format token count: <1000 → raw, ≥1000 → "12.3k" / "12k".
    fn format_tokens(value: u32) -> String {
        if value < 1000 {
            return value.to_string();
        }
        let k = value as f64 / 1000.0;
        if k.fract() == 0.0 {
            format!("{}k", k as u32)
        } else {
            format!("{:.1}k", k)
        }
    }

    /// Pick TTL label from cache_creation breakdown. Empty when both are zero.
    fn ttl_label(usage: &NormalizedUsage) -> &'static str {
        match (usage.cache_creation_5m > 0, usage.cache_creation_1h > 0) {
            (true, true) => "[5m+1h]",
            (true, false) => "[5m]",
            (false, true) => "[1h]",
            (false, false) => "",
        }
    }

    fn build_secondary(usage: &NormalizedUsage) -> String {
        let denom = prompt_denom(usage);
        if denom == 0 {
            return String::new();
        }

        let ttl = Self::ttl_label(usage);
        if ttl.is_empty() {
            format!(
                "{}/{}",
                Self::format_tokens(usage.cache_read_input_tokens),
                Self::format_tokens(denom)
            )
        } else {
            format!(
                "{}/{} {}",
                Self::format_tokens(usage.cache_read_input_tokens),
                Self::format_tokens(denom),
                ttl
            )
        }
    }

    fn classify_hit_rate(&self, percentage: Option<f64>) -> ThresholdLevel {
        let warning_below = self.warning_below.unwrap_or(60.0);
        let critical_below = self.critical_below.unwrap_or(30.0);
        match percentage {
            Some(p) if p < critical_below => ThresholdLevel::Critical,
            Some(p) if p < warning_below => ThresholdLevel::Warning,
            _ => ThresholdLevel::Normal,
        }
    }
}

impl Segment for CacheHitSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;
        let mode = self.mode_or_default();
        let last_pct = compute_hit_rate(&stats.last_turn);
        let total_pct = compute_hit_rate(&stats.totals);

        let primary = match mode {
            "total" => format_pct(total_pct),
            "both" => format!("{}/{}", format_pct(last_pct), format_pct(total_pct)),
            _ => format_pct(last_pct),
        };
        // Threshold colorization basis: total drives "both"/"total"; last drives "last".
        let pct_for_color = match mode {
            "total" | "both" => total_pct,
            _ => last_pct,
        };
        let level = self.classify_hit_rate(pct_for_color);
        let secondary = Self::build_secondary(&stats.last_turn);
        let ttl = Self::ttl_label(&stats.last_turn);

        let mut metadata = HashMap::new();
        metadata.insert(
            "cache_read".to_string(),
            stats.last_turn.cache_read_input_tokens.to_string(),
        );
        metadata.insert(
            "cache_creation".to_string(),
            stats.last_turn.cache_creation_input_tokens.to_string(),
        );
        metadata.insert(
            "input_tokens".to_string(),
            stats.last_turn.input_tokens.to_string(),
        );
        metadata.insert(
            "cache_creation_5m".to_string(),
            stats.last_turn.cache_creation_5m.to_string(),
        );
        metadata.insert(
            "cache_creation_1h".to_string(),
            stats.last_turn.cache_creation_1h.to_string(),
        );
        if let Some(hit_rate) = last_pct {
            metadata.insert("hit_rate".to_string(), format!("{:.2}", hit_rate));
        }
        metadata.insert("hit_rate_last".to_string(), format_optional_pct(last_pct));
        metadata.insert("hit_rate_total".to_string(), format_optional_pct(total_pct));
        metadata.insert("mode".to_string(), mode.to_string());
        metadata.insert("turns".to_string(), stats.assistant_count.to_string());
        metadata.insert("ttl".to_string(), ttl.to_string());

        Some(SegmentData {
            primary,
            secondary,
            metadata,
            level,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::CacheHit
    }
}

fn prompt_denom(usage: &NormalizedUsage) -> u32 {
    usage
        .input_tokens
        .saturating_add(usage.cache_read_input_tokens)
        .saturating_add(usage.cache_creation_input_tokens)
}

fn compute_hit_rate(usage: &NormalizedUsage) -> Option<f64> {
    let denom = prompt_denom(usage);
    if denom == 0 {
        return None;
    }

    Some((usage.cache_read_input_tokens as f64 / denom as f64) * 100.0)
}

fn format_pct(value: Option<f64>) -> String {
    match value {
        Some(percent) if percent.fract() == 0.0 => format!("{:.0}%", percent),
        Some(percent) => format!("{:.1}%", percent),
        None => "-".to_string(),
    }
}

fn format_optional_pct(value: Option<f64>) -> String {
    value
        .map(|percent| format!("{:.2}", percent))
        .unwrap_or_else(|| "-".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{InputData, Model, Workspace};
    use std::fs;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn write_transcript(content: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "ccline-cache-hit-{}-{}.jsonl",
            std::process::id(),
            unique
        ));
        fs::write(&path, content).unwrap();
        path
    }

    fn input_for(path: PathBuf) -> InputData {
        InputData {
            model: Model {
                id: "claude-sonnet-4".to_string(),
                display_name: "Sonnet 4".to_string(),
            },
            workspace: Workspace {
                current_dir: ".".to_string(),
            },
            transcript_path: path.to_string_lossy().into_owned(),
            cost: None,
            output_style: None,
        }
    }

    fn transcript_with_last_and_total_rates() -> String {
        [
            r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":0,"cache_creation_input_tokens":0}}}"#,
            r#"{"type":"assistant","message":{"usage":{"input_tokens":100,"cache_read_input_tokens":300,"cache_creation_input_tokens":100}}}"#,
        ]
        .join("\n")
    }

    #[test]
    fn default_mode_preserves_last_turn_output() {
        let path = write_transcript(&transcript_with_last_and_total_rates());
        let input = input_for(path);
        let data = CacheHitSegment::new().collect(&input).unwrap();

        assert_eq!(data.primary, "60%");
        assert_eq!(data.secondary, "300/500");
    }

    #[test]
    fn total_mode_formats_total_rate() {
        let path = write_transcript(&transcript_with_last_and_total_rates());
        let input = input_for(path);
        let data = CacheHitSegment::new()
            .with_mode("total")
            .collect(&input)
            .unwrap();

        assert_eq!(data.primary, "50%");
    }

    #[test]
    fn both_mode_formats_last_and_total_rates() {
        let path = write_transcript(&transcript_with_last_and_total_rates());
        let input = input_for(path);
        let data = CacheHitSegment::new()
            .with_mode("both")
            .collect(&input)
            .unwrap();

        assert_eq!(data.primary, "60%/50%");
    }

    #[test]
    fn zero_denominator_renders_dash_and_empty_secondary() {
        // Pure output-only assistant turn (no input/cache_read/cache_creation).
        let path = write_transcript(
            r#"{"type":"assistant","message":{"usage":{"output_tokens":42}}}"#,
        );
        let input = input_for(path);
        let data = CacheHitSegment::new().collect(&input).unwrap();

        assert_eq!(data.primary, "-");
        assert_eq!(data.secondary, "");
    }
}
