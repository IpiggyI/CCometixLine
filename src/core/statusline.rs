use crate::config::{AnsiColor, Config, SegmentConfig, StyleMode};
use crate::core::segments::{SegmentData, ThresholdLevel};

/// Resolve the text color for a segment based on its threshold level.
/// Fallback chain: critical -> warning -> text; warning -> text; normal -> text.
fn pick_text_color(config: &SegmentConfig, level: ThresholdLevel) -> Option<&AnsiColor> {
    match level {
        ThresholdLevel::Normal => config.colors.text.as_ref(),
        ThresholdLevel::Warning => config
            .colors
            .warning
            .as_ref()
            .or(config.colors.text.as_ref()),
        ThresholdLevel::Critical => config
            .colors
            .critical
            .as_ref()
            .or(config.colors.warning.as_ref())
            .or(config.colors.text.as_ref()),
    }
}

/// Resolve the icon color. At Normal level icons keep their dedicated color
/// so the static palette is preserved; on Warning/Critical icons follow the
/// text color chain to reinforce the alert state.
fn pick_icon_color(config: &SegmentConfig, level: ThresholdLevel) -> Option<&AnsiColor> {
    match level {
        ThresholdLevel::Normal => config.colors.icon.as_ref(),
        _ => pick_text_color(config, level).or(config.colors.icon.as_ref()),
    }
}

fn option_f64(config: &SegmentConfig, key: &str, default: f64) -> f64 {
    config
        .options
        .get(key)
        .and_then(|v| v.as_f64())
        .unwrap_or(default)
}

fn option_i64(config: &SegmentConfig, key: &str, default: i64) -> i64 {
    config
        .options
        .get(key)
        .and_then(|v| v.as_i64())
        .unwrap_or(default)
}

fn option_u32(config: &SegmentConfig, key: &str, default: u32) -> u32 {
    config
        .options
        .get(key)
        .and_then(|v| v.as_u64())
        .and_then(|v| u32::try_from(v).ok())
        .unwrap_or(default)
}

/// Strip ANSI escape sequences and return visible text length
fn visible_width(text: &str) -> usize {
    let mut visible = String::new();
    let mut in_escape = false;
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '\x1b' {
            // Start of ANSI escape sequence
            in_escape = true;
            // Skip the [ character
            if chars.peek() == Some(&'[') {
                chars.next();
            }
        } else if in_escape {
            // Skip until we find the end of the escape sequence (letter)
            if ch.is_alphabetic() {
                in_escape = false;
            }
        } else {
            // Regular character
            visible.push(ch);
        }
    }

    visible.chars().count()
}

pub struct StatusLineGenerator {
    config: Config,
}

impl StatusLineGenerator {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn generate(&self, segments: Vec<(SegmentConfig, SegmentData)>) -> String {
        let mut output = Vec::new();
        let enabled_segments: Vec<_> = segments
            .into_iter()
            .filter(|(config, _)| config.enabled)
            .collect();

        for (config, data) in enabled_segments.iter() {
            let rendered = self.render_segment(config, data);
            if !rendered.is_empty() {
                output.push(rendered);
            }
        }

        if output.is_empty() {
            return String::new();
        }

        // Handle Powerline arrow separators with color transition
        if self.config.style.separator == "\u{e0b0}" {
            self.join_with_powerline_arrows(&output, &enabled_segments)
        } else {
            // For all other separators, use white color and simple join
            self.join_with_white_separators(&output)
        }
    }

    /// Generate statusline for TUI preview with proper width calculation
    /// This method handles ANSI escape sequences properly for ratatui rendering
    pub fn generate_for_tui(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
    ) -> ratatui::text::Line<'static> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span};

        // Use the same generate method and convert to TUI
        let full_output = self.generate(segments);

        if let Ok(text) = full_output.into_text() {
            if let Some(line) = text.lines.into_iter().next() {
                return line;
            }
        }

        // Fallback to raw text
        Line::from(vec![Span::raw(full_output)])
    }

    /// Generate TUI-optimized text with intelligent wrapping by segment for preview
    pub fn generate_for_tui_preview(
        &self,
        segments: Vec<(SegmentConfig, SegmentData)>,
        max_width: u16,
    ) -> ratatui::text::Text<'_> {
        use ansi_to_tui::IntoText;
        use ratatui::text::{Line, Span, Text};

        let enabled_segments: Vec<_> = segments
            .into_iter()
            .filter(|(config, _)| config.enabled)
            .collect();

        if enabled_segments.is_empty() {
            return Text::from(vec![Line::default()]);
        }

        // Render each segment individually
        let mut rendered_segments = Vec::new();
        let mut segment_configs = Vec::new();

        for (config, data) in &enabled_segments {
            let rendered = self.render_segment(config, data);
            if !rendered.is_empty() {
                rendered_segments.push(rendered);
                segment_configs.push(config.clone());
            }
        }

        if rendered_segments.is_empty() {
            return Text::from(vec![Line::default()]);
        }

        // Pre-calculate separators between segments
        let mut separators = Vec::new();
        for i in 0..rendered_segments.len().saturating_sub(1) {
            let separator = if self.config.style.separator == "\u{e0b0}" {
                // Powerline arrows with color transition
                let prev_bg = segment_configs
                    .get(i)
                    .and_then(|config| config.colors.background.as_ref());
                let curr_bg = segment_configs
                    .get(i + 1)
                    .and_then(|config| config.colors.background.as_ref());
                self.create_powerline_arrow(prev_bg, curr_bg)
            } else {
                // Regular separators with white color
                format!("\x1b[37m{}\x1b[0m", self.config.style.separator)
            };
            separators.push(separator);
        }

        // Intelligent line wrapping by segment
        let mut lines: Vec<String> = Vec::new();
        let mut current_line = String::new();
        let mut current_width = 0usize;
        let max_w = max_width as usize;

        for i in 0..rendered_segments.len() {
            let segment = &rendered_segments[i];
            let segment_width = visible_width(segment);

            // Check if adding this segment would exceed max_width
            if current_width > 0 && current_width + segment_width > max_w {
                // Current line would overflow, start a new line
                lines.push(current_line.clone());
                current_line.clear();
                current_width = 0;
            }

            // Add the segment to current line
            current_line.push_str(segment);
            current_width += segment_width;

            // Handle separator if not the last segment
            if i < separators.len() {
                let separator = &separators[i];
                let separator_width = visible_width(separator);

                // Check if next segment exists
                if i + 1 < rendered_segments.len() {
                    let next_segment = &rendered_segments[i + 1];
                    let next_width = visible_width(next_segment);

                    // Check if separator AND next segment both fit
                    if current_width + separator_width + next_width <= max_w {
                        // Both fit, add separator and continue on same line
                        current_line.push_str(separator);
                        current_width += separator_width;
                    } else {
                        // Separator and/or next segment don't fit
                        // Don't add separator, just break line
                        lines.push(current_line.clone());
                        current_line.clear();
                        current_width = 0;
                    }
                }
            }
        }

        // Add the last line if it's not empty
        if !current_line.is_empty() {
            lines.push(current_line);
        }

        // Convert string lines to ratatui Text
        let mut tui_lines = Vec::new();
        for line in lines {
            if let Ok(text) = line.into_text() {
                for tui_line in text.lines {
                    tui_lines.push(tui_line);
                }
            } else {
                tui_lines.push(Line::from(vec![Span::raw(line)]));
            }
        }

        // Ensure we have at least one line
        if tui_lines.is_empty() {
            tui_lines.push(Line::default());
        }

        Text::from(tui_lines)
    }

    fn render_segment(&self, config: &SegmentConfig, data: &SegmentData) -> String {
        let icon = if let Some(dynamic_icon) = data.metadata.get("dynamic_icon") {
            dynamic_icon.clone()
        } else {
            self.get_icon(config)
        };

        // Apply background color to the entire segment if set
        if let Some(bg_color) = &config.colors.background {
            let bg_code = self.apply_background_color(bg_color);

            // Build the entire segment content first
            let icon_colored = if let Some(icon_color) = pick_icon_color(config, data.level) {
                self.apply_color(&icon, Some(icon_color))
                    .replace("\x1b[0m", "")
            } else {
                icon.clone()
            };

            let text_styled = self
                .apply_style(
                    &data.primary,
                    pick_text_color(config, data.level),
                    config.styles.text_bold,
                )
                .replace("\x1b[0m", "");

            let mut segment_content = format!(" {} {} ", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                let secondary_styled = self
                    .apply_style(
                        &data.secondary,
                        pick_text_color(config, data.level),
                        config.styles.text_bold,
                    )
                    .replace("\x1b[0m", "");
                segment_content.push_str(&format!("{} ", secondary_styled));
            }

            // Apply background to the entire content and reset at the end
            format!("{}{}\x1b[49m", bg_code, segment_content)
        } else {
            // No background color, use original logic
            let icon_colored = self.apply_color(&icon, pick_icon_color(config, data.level));
            let text_styled = self.apply_style(
                &data.primary,
                pick_text_color(config, data.level),
                config.styles.text_bold,
            );

            let mut segment = format!("{} {}", icon_colored, text_styled);

            if !data.secondary.is_empty() {
                segment.push_str(&format!(
                    " {}",
                    self.apply_style(
                        &data.secondary,
                        pick_text_color(config, data.level),
                        config.styles.text_bold
                    )
                ));
            }

            segment
        }
    }

    fn get_icon(&self, config: &SegmentConfig) -> String {
        match self.config.style.mode {
            StyleMode::Plain => config.icon.plain.clone(),
            StyleMode::NerdFont => config.icon.nerd_font.clone(),
            StyleMode::Powerline => config.icon.nerd_font.clone(), // Future: use Powerline icons
        }
    }

    fn apply_color(&self, text: &str, color: Option<&AnsiColor>) -> String {
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m{}\x1b[0m", code, text)
            }
            Some(AnsiColor::Color256 { c256 }) => {
                format!("\x1b[38;5;{}m{}\x1b[0m", c256, text)
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                format!("\x1b[38;2;{};{};{}m{}\x1b[0m", r, g, b, text)
            }
            None => text.to_string(),
        }
    }

    fn apply_style(&self, text: &str, color: Option<&AnsiColor>, bold: bool) -> String {
        let mut codes = Vec::new();

        // Add style codes
        if bold {
            codes.push("1".to_string()); // Bold: \x1b[1m
        }

        // Add color codes
        match color {
            Some(AnsiColor::Color16 { c16 }) => {
                let color_code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                codes.push(color_code.to_string());
            }
            Some(AnsiColor::Color256 { c256 }) => {
                codes.push("38".to_string());
                codes.push("5".to_string());
                codes.push(c256.to_string());
            }
            Some(AnsiColor::Rgb { r, g, b }) => {
                codes.push("38".to_string());
                codes.push("2".to_string());
                codes.push(r.to_string());
                codes.push(g.to_string());
                codes.push(b.to_string());
            }
            None => {}
        }

        if codes.is_empty() {
            text.to_string()
        } else {
            format!("\x1b[{}m{}\x1b[0m", codes.join(";"), text)
        }
    }

    fn apply_background_color(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 40 + c16 } else { 100 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[48;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[48;2;{};{};{}m", r, g, b)
            }
        }
    }

    /// Join segments with white separators (non-Powerline)
    fn join_with_white_separators(&self, rendered_segments: &[String]) -> String {
        if rendered_segments.is_empty() {
            return String::new();
        }

        // Use white color for separator
        let white_separator = format!("\x1b[37m{}\x1b[0m", self.config.style.separator);
        rendered_segments.join(&white_separator)
    }

    /// Join segments with Powerline arrow separators with proper color transitions
    fn join_with_powerline_arrows(
        &self,
        rendered_segments: &[String],
        segment_configs: &[(SegmentConfig, SegmentData)],
    ) -> String {
        if rendered_segments.is_empty() {
            return String::new();
        }

        if rendered_segments.len() == 1 {
            return rendered_segments[0].clone();
        }

        let mut result = rendered_segments[0].clone();

        for (i, _) in rendered_segments.iter().enumerate().skip(1) {
            let prev_bg = segment_configs
                .get(i - 1)
                .and_then(|(config, _)| config.colors.background.as_ref());
            let curr_bg = segment_configs
                .get(i)
                .and_then(|(config, _)| config.colors.background.as_ref());

            // Create Powerline arrow with color transition
            let arrow = self.create_powerline_arrow(prev_bg, curr_bg);

            result.push_str(&arrow);
            result.push_str(&rendered_segments[i]);
        }

        // Reset colors at the end
        result.push_str("\x1b[0m");
        result
    }

    /// Create a Powerline arrow with proper color transition
    fn create_powerline_arrow(
        &self,
        prev_bg: Option<&AnsiColor>,
        curr_bg: Option<&AnsiColor>,
    ) -> String {
        let arrow_char = "\u{e0b0}";

        match (prev_bg, curr_bg) {
            (Some(prev), Some(curr)) => {
                // Arrow foreground = previous segment's background
                // Arrow background = current segment's background
                let fg_code = self.color_to_foreground_code(prev);
                let bg_code = self.apply_background_color(curr);
                format!("{}{}{}\x1b[0m", bg_code, fg_code, arrow_char)
            }
            (Some(prev), None) => {
                // Previous segment has background, current doesn't
                let fg_code = self.color_to_foreground_code(prev);
                format!("{}{}\x1b[0m", fg_code, arrow_char)
            }
            (None, Some(curr)) => {
                // Current segment has background, previous doesn't
                let bg_code = self.apply_background_color(curr);
                format!("{}{}\x1b[0m", bg_code, arrow_char)
            }
            (None, None) => {
                // Neither segment has background color
                arrow_char.to_string()
            }
        }
    }

    /// Convert AnsiColor to foreground color code
    fn color_to_foreground_code(&self, color: &AnsiColor) -> String {
        match color {
            AnsiColor::Color16 { c16 } => {
                let code = if *c16 < 8 { 30 + c16 } else { 90 + (c16 - 8) };
                format!("\x1b[{}m", code)
            }
            AnsiColor::Color256 { c256 } => {
                format!("\x1b[38;5;{}m", c256)
            }
            AnsiColor::Rgb { r, g, b } => {
                format!("\x1b[38;2;{};{};{}m", r, g, b)
            }
        }
    }
}

pub fn collect_all_segments(
    config: &Config,
    input: &crate::config::InputData,
) -> Vec<(SegmentConfig, SegmentData)> {
    use crate::core::segments::*;

    let mut results = Vec::new();

    for segment_config in &config.segments {
        // Skip disabled segments to avoid unnecessary API requests
        if !segment_config.enabled {
            continue;
        }

        let segment_data = match segment_config.id {
            crate::config::SegmentId::Model => {
                let segment = ModelSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Directory => {
                let segment = DirectorySegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Git => {
                let show_sha = segment_config
                    .options
                    .get("show_sha")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let push_warning =
                    option_u32(segment_config, "push_warning_threshold", 5);
                let push_critical =
                    option_u32(segment_config, "push_critical_threshold", 20);
                let segment = GitSegment::new()
                    .with_sha(show_sha)
                    .with_push_thresholds(push_warning, push_critical);
                segment.collect(input)
            }
            crate::config::SegmentId::ContextWindow => {
                let warning = option_f64(segment_config, "warning_threshold", 70.0);
                let critical = option_f64(segment_config, "critical_threshold", 85.0);
                let segment = ContextWindowSegment::new().with_thresholds(warning, critical);
                segment.collect(input)
            }
            crate::config::SegmentId::Usage => {
                let segment = UsageSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Cost => {
                let segment = CostSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Session => {
                let segment = SessionSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::OutputStyle => {
                let segment = OutputStyleSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::Update => {
                let segment = UpdateSegment::new();
                segment.collect(input)
            }
            crate::config::SegmentId::CacheHit => {
                let mode = segment_config
                    .options
                    .get("mode")
                    .and_then(|value| value.as_str())
                    .unwrap_or("last");
                let warning_below = option_f64(segment_config, "warning_below", 60.0);
                let critical_below = option_f64(segment_config, "critical_below", 30.0);
                let segment = CacheHitSegment::new()
                    .with_mode(mode)
                    .with_thresholds(warning_below, critical_below);
                segment.collect(input)
            }
            crate::config::SegmentId::SinceReply => {
                let warning_seconds = option_i64(segment_config, "warning_seconds", 300);
                let critical_seconds = option_i64(segment_config, "critical_seconds", 1800);
                let segment = SinceReplySegment::new()
                    .with_thresholds(warning_seconds, critical_seconds);
                segment.collect(input)
            }
            crate::config::SegmentId::SessionTokens => {
                let segment = SessionTokensSegment::new();
                segment.collect(input)
            }
        };

        if let Some(data) = segment_data {
            results.push((segment_config.clone(), data));
        }
    }

    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ColorConfig, IconConfig, SegmentId, TextStyleConfig};
    use std::collections::HashMap;

    fn color(c256: u8) -> AnsiColor {
        AnsiColor::Color256 { c256 }
    }

    fn make_segment_config(colors: ColorConfig) -> SegmentConfig {
        SegmentConfig {
            id: SegmentId::ContextWindow,
            enabled: true,
            icon: IconConfig {
                plain: String::new(),
                nerd_font: String::new(),
            },
            colors,
            styles: TextStyleConfig::default(),
            options: HashMap::new(),
        }
    }

    #[test]
    fn pick_text_normal_returns_text_color() {
        let cfg = make_segment_config(ColorConfig {
            icon: None,
            text: Some(color(142)),
            background: None,
            warning: Some(color(214)),
            critical: Some(color(167)),
        });
        assert_eq!(
            pick_text_color(&cfg, ThresholdLevel::Normal),
            cfg.colors.text.as_ref()
        );
    }

    #[test]
    fn pick_text_warning_falls_back_to_text_when_missing() {
        let cfg = make_segment_config(ColorConfig {
            icon: None,
            text: Some(color(142)),
            background: None,
            warning: None,
            critical: Some(color(167)),
        });
        assert_eq!(
            pick_text_color(&cfg, ThresholdLevel::Warning),
            cfg.colors.text.as_ref()
        );
    }

    #[test]
    fn pick_text_critical_falls_back_through_chain() {
        let to_warning = make_segment_config(ColorConfig {
            icon: None,
            text: Some(color(142)),
            background: None,
            warning: Some(color(214)),
            critical: None,
        });
        assert_eq!(
            pick_text_color(&to_warning, ThresholdLevel::Critical),
            to_warning.colors.warning.as_ref()
        );

        let to_text = make_segment_config(ColorConfig {
            icon: None,
            text: Some(color(142)),
            background: None,
            warning: None,
            critical: None,
        });
        assert_eq!(
            pick_text_color(&to_text, ThresholdLevel::Critical),
            to_text.colors.text.as_ref()
        );
    }

    #[test]
    fn pick_icon_normal_keeps_static_icon_color() {
        let cfg = make_segment_config(ColorConfig {
            icon: Some(color(39)),
            text: Some(color(142)),
            background: None,
            warning: Some(color(214)),
            critical: Some(color(167)),
        });
        assert_eq!(
            pick_icon_color(&cfg, ThresholdLevel::Normal),
            cfg.colors.icon.as_ref()
        );
    }

    #[test]
    fn pick_icon_critical_follows_text_chain() {
        let cfg = make_segment_config(ColorConfig {
            icon: Some(color(39)),
            text: Some(color(142)),
            background: None,
            warning: Some(color(214)),
            critical: Some(color(167)),
        });
        assert_eq!(
            pick_icon_color(&cfg, ThresholdLevel::Critical),
            cfg.colors.critical.as_ref()
        );
    }
}
