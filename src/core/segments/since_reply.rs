use super::{Segment, SegmentData};
use crate::config::{InputData, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

#[derive(Default)]
pub struct SinceReplySegment;

impl SinceReplySegment {
    pub fn new() -> Self {
        Self
    }

    fn format_elapsed(seconds: i64) -> String {
        let seconds = seconds.max(0);
        match seconds {
            0..=59 => format!("{}s", seconds),
            60..=3599 => format!("{}m {}s", seconds / 60, seconds % 60),
            3600..=86_399 => format!("{}h {}m", seconds / 3600, (seconds % 3600) / 60),
            _ => format!("{}d", seconds / 86_400),
        }
    }
}

impl Segment for SinceReplySegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        let stats = transcript::parse_session_stats(&input.transcript_path)?;
        let last_assistant_at = stats.last_assistant_at?;
        let elapsed = (chrono::Utc::now() - last_assistant_at)
            .num_seconds()
            .max(0);

        let mut metadata = HashMap::new();
        metadata.insert("seconds".to_string(), elapsed.to_string());
        metadata.insert(
            "last_assistant_at".to_string(),
            last_assistant_at.to_rfc3339(),
        );

        Some(SegmentData {
            primary: Self::format_elapsed(elapsed),
            secondary: String::new(),
            metadata,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::SinceReply
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_elapsed_handles_boundaries() {
        let cases = [
            (-1, "0s"),
            (0, "0s"),
            (59, "59s"),
            (60, "1m 0s"),
            (3599, "59m 59s"),
            (3600, "1h 0m"),
            (86_399, "23h 59m"),
            (86_400, "1d"),
        ];

        for (seconds, expected) in cases {
            assert_eq!(SinceReplySegment::format_elapsed(seconds), expected);
        }
    }
}
