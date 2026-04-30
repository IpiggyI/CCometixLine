use super::{Segment, SegmentData};
use crate::config::{InputData, NormalizedUsage, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct CacheHitSegment;

impl CacheHitSegment {
    pub fn new() -> Self {
        Self
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
}

impl Segment for CacheHitSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let usage = transcript::parse_last_assistant_usage(&input.transcript_path)?;

        // Prompt-side total: input + cache_read + cache_creation. Output excluded
        // because it does not participate in cache lookup semantics.
        let denom =
            usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens;

        let mut metadata = HashMap::new();
        metadata.insert(
            "cache_read".to_string(),
            usage.cache_read_input_tokens.to_string(),
        );
        metadata.insert(
            "cache_creation".to_string(),
            usage.cache_creation_input_tokens.to_string(),
        );
        metadata.insert("input_tokens".to_string(), usage.input_tokens.to_string());
        metadata.insert(
            "cache_creation_5m".to_string(),
            usage.cache_creation_5m.to_string(),
        );
        metadata.insert(
            "cache_creation_1h".to_string(),
            usage.cache_creation_1h.to_string(),
        );

        if denom == 0 {
            return Some(SegmentData {
                primary: "-".to_string(),
                secondary: String::new(),
                metadata,
            });
        }

        let hit_rate = (usage.cache_read_input_tokens as f64 / denom as f64) * 100.0;
        let primary = if hit_rate.fract() == 0.0 {
            format!("{:.0}%", hit_rate)
        } else {
            format!("{:.1}%", hit_rate)
        };

        let ttl = Self::ttl_label(&usage);
        let secondary = if ttl.is_empty() {
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
        };

        metadata.insert("hit_rate".to_string(), format!("{:.2}", hit_rate));
        metadata.insert("ttl".to_string(), ttl.to_string());

        Some(SegmentData {
            primary,
            secondary,
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::CacheHit
    }
}
