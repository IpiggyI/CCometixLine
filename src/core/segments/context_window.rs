use super::{Segment, SegmentData, ThresholdLevel};
use crate::config::{InputData, ModelConfig, SegmentId};
use crate::core::transcript;
use std::collections::HashMap;

pub struct ContextWindowSegment {
    warning_threshold: f64,
    critical_threshold: f64,
}

impl Default for ContextWindowSegment {
    fn default() -> Self {
        Self {
            warning_threshold: 70.0,
            critical_threshold: 85.0,
        }
    }
}

impl ContextWindowSegment {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_thresholds(mut self, warning_threshold: f64, critical_threshold: f64) -> Self {
        self.warning_threshold = warning_threshold;
        self.critical_threshold = critical_threshold;
        self
    }

    /// Get context limit for the specified model
    fn get_context_limit_for_model(model_id: &str) -> u32 {
        let model_config = ModelConfig::load();
        model_config.get_context_limit(model_id)
    }

    fn classify_percentage(&self, percentage: f64) -> ThresholdLevel {
        if percentage >= self.critical_threshold {
            ThresholdLevel::Critical
        } else if percentage >= self.warning_threshold {
            ThresholdLevel::Warning
        } else {
            ThresholdLevel::Normal
        }
    }
}

impl Segment for ContextWindowSegment {
    fn collect(&self, input: &InputData) -> Option<SegmentData> {
        // Dynamically determine context limit based on current model ID
        let context_limit = Self::get_context_limit_for_model(&input.model.id);

        let context_used_token_opt = transcript::parse_last_assistant_usage(&input.transcript_path)
            .map(|u| u.display_tokens());

        let (percentage_display, tokens_display, level) = match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate = (context_used_token as f64 / context_limit as f64) * 100.0;

                let percentage = if context_used_rate.fract() == 0.0 {
                    format!("{:.0}%", context_used_rate)
                } else {
                    format!("{:.1}%", context_used_rate)
                };

                let tokens = if context_used_token >= 1000 {
                    let k_value = context_used_token as f64 / 1000.0;
                    if k_value.fract() == 0.0 {
                        format!("{}k", k_value as u32)
                    } else {
                        format!("{:.1}k", k_value)
                    }
                } else {
                    context_used_token.to_string()
                };

                (percentage, tokens, self.classify_percentage(context_used_rate))
            }
            None => {
                // No usage data available
                ("-".to_string(), "-".to_string(), ThresholdLevel::Normal)
            }
        };

        let mut metadata = HashMap::new();
        match context_used_token_opt {
            Some(context_used_token) => {
                let context_used_rate = (context_used_token as f64 / context_limit as f64) * 100.0;
                metadata.insert("tokens".to_string(), context_used_token.to_string());
                metadata.insert("percentage".to_string(), context_used_rate.to_string());
            }
            None => {
                metadata.insert("tokens".to_string(), "-".to_string());
                metadata.insert("percentage".to_string(), "-".to_string());
            }
        }
        metadata.insert("limit".to_string(), context_limit.to_string());
        metadata.insert("model".to_string(), input.model.id.clone());

        Some(SegmentData {
            primary: format!("{} · {} tokens", percentage_display, tokens_display),
            secondary: String::new(),
            metadata,
            level,
        })
    }

    fn id(&self) -> SegmentId {
        SegmentId::ContextWindow
    }
}
