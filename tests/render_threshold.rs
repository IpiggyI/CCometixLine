//! Snapshot test: threshold-based dynamic colorization.
//!
//! Verifies that the renderer picks the correct ANSI 256-color code based on
//! the segment's `ThresholdLevel`, with the documented fallback chain
//! (critical -> warning -> text).

use ccometixline::config::{
    AnsiColor, ColorConfig, Config, IconConfig, LayoutConfig, SegmentConfig, SegmentId,
    StyleConfig, StyleMode, TextStyleConfig,
};
use ccometixline::core::segments::{SegmentData, ThresholdLevel};
use ccometixline::core::StatusLineGenerator;
use std::collections::HashMap;

fn c256(value: u8) -> AnsiColor {
    AnsiColor::Color256 { c256: value }
}

fn render(level: ThresholdLevel, colors: ColorConfig) -> String {
    let segment_config = SegmentConfig {
        id: SegmentId::ContextWindow,
        enabled: true,
        icon: IconConfig {
            plain: String::new(),
            nerd_font: String::new(),
        },
        colors,
        styles: TextStyleConfig::default(),
        options: HashMap::new(),
    };
    let config = Config {
        style: StyleConfig {
            mode: StyleMode::Plain,
            separator: " | ".to_string(),
        },
        segments: vec![segment_config.clone()],
        theme: "test".to_string(),
        layout: LayoutConfig::default(),
    };
    let data = SegmentData {
        primary: "42%".to_string(),
        secondary: String::new(),
        metadata: HashMap::new(),
        level,
    };
    StatusLineGenerator::new(config)
        .generate(vec![(segment_config, data)])
        .join("\n")
}

#[test]
fn renderer_uses_normal_warning_and_critical_text_colors() {
    let colors = ColorConfig {
        icon: None,
        text: Some(c256(142)),
        background: None,
        warning: Some(c256(214)),
        critical: Some(c256(167)),
    };
    assert!(render(ThresholdLevel::Normal, colors.clone()).contains("\x1b[38;5;142m"));
    assert!(render(ThresholdLevel::Warning, colors.clone()).contains("\x1b[38;5;214m"));
    assert!(render(ThresholdLevel::Critical, colors).contains("\x1b[38;5;167m"));
}

#[test]
fn renderer_falls_back_critical_to_warning_then_text() {
    let only_warning = ColorConfig {
        icon: None,
        text: Some(c256(142)),
        background: None,
        warning: Some(c256(214)),
        critical: None,
    };
    assert!(render(ThresholdLevel::Critical, only_warning).contains("\x1b[38;5;214m"));

    let only_text = ColorConfig {
        icon: None,
        text: Some(c256(142)),
        background: None,
        warning: None,
        critical: None,
    };
    assert!(render(ThresholdLevel::Critical, only_text).contains("\x1b[38;5;142m"));
}

#[test]
fn renderer_falls_back_warning_to_text() {
    let only_text = ColorConfig {
        icon: None,
        text: Some(c256(142)),
        background: None,
        warning: None,
        critical: None,
    };
    assert!(render(ThresholdLevel::Warning, only_text).contains("\x1b[38;5;142m"));
}
