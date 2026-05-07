pub mod cache_hit;
pub mod context_window;
pub mod cost;
pub mod directory;
pub mod git;
pub mod model;
pub mod output_style;
pub mod session;
pub mod session_tokens;
pub mod since_reply;
pub mod update;
pub mod usage;

use crate::config::{InputData, SegmentId};
use std::collections::HashMap;

// New Segment trait for data collection only
pub trait Segment {
    fn collect(&self, input: &InputData) -> Option<SegmentData>;
    fn id(&self) -> SegmentId;
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThresholdLevel {
    #[default]
    Normal,
    Warning,
    Critical,
}

#[derive(Debug, Clone)]
pub struct SegmentData {
    pub primary: String,
    pub secondary: String,
    pub metadata: HashMap<String, String>,
    pub level: ThresholdLevel,
}

// Re-export all segment types
pub use cache_hit::CacheHitSegment;
pub use context_window::ContextWindowSegment;
pub use cost::CostSegment;
pub use directory::DirectorySegment;
pub use git::GitSegment;
pub use model::ModelSegment;
pub use output_style::OutputStyleSegment;
pub use session::SessionSegment;
pub use session_tokens::SessionTokensSegment;
pub use since_reply::SinceReplySegment;
pub use update::UpdateSegment;
pub use usage::UsageSegment;
