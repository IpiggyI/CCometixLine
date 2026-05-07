use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct SessionTokensSegment;

impl SessionTokensSegment {
    pub fn new() -> Self {
        Self
    }

    fn format_tokens(value: u32) -> String {
        if value < 1000 {
            return value.to_string();
        }

        if value >= 1_000_000 {
            return format!("{:.1}M", value as f64 / 1_000_000.0);
        }

        if value.is_multiple_of(1000) {
            format!("{}k", value / 1000)
        } else {
            format!("{:.1}k", value as f64 / 1000.0)
        }
    }
}

impl Segment for SessionTokensSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;

        let in_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.input_tokens),
            Self::format_tokens(stats.totals.input_tokens)
        );
        let out_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.output_tokens),
            Self::format_tokens(stats.totals.output_tokens)
        );
        let cache_pair = format!(
            "{}|{}",
            Self::format_tokens(stats.last_turn.cache_read_input_tokens),
            Self::format_tokens(stats.totals.cache_read_input_tokens)
        );

        let mut metadata = HashMap::new();
        metadata.insert("turns".to_string(), stats.assistant_count.to_string());
        metadata.insert(
            "total_input".to_string(),
            stats.totals.input_tokens.to_string(),
        );
        metadata.insert(
            "total_output".to_string(),
            stats.totals.output_tokens.to_string(),
        );
        metadata.insert(
            "total_cache_read".to_string(),
            stats.totals.cache_read_input_tokens.to_string(),
        );

        Some(SegmentData {
            primary: format!(
                "in {} · out {} · cache {}",
                in_pair, out_pair, cache_pair
            ),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::SessionTokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_tokens_handles_boundaries() {
        let cases = [
            (0, "0"),
            (999, "999"),
            (1000, "1k"),
            (1500, "1.5k"),
            (1_000_000, "1.0M"),
            (1_500_000, "1.5M"),
        ];

        for (tokens, expected) in cases {
            assert_eq!(SessionTokensSegment::format_tokens(tokens), expected);
        }
    }
}
