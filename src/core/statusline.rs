use crate::config::{
    AnsiColor, Config, LayoutMode, MergeGroup, SegmentConfig, SegmentId, StyleMode,
};
use crate::core::segments::{SegmentData, ThresholdLevel};
use std::collections::HashMap;

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

/// Detect terminal width with three-tier fallback:
/// `crossterm::terminal::size()` → `$COLUMNS` env → `usize::MAX` (no auto_stack).
fn detect_terminal_width() -> usize {
    crossterm::terminal::size()
        .map(|(width, _)| usize::from(width))
        .ok()
        .filter(|width| *width > 0)
        .or_else(|| {
            std::env::var("COLUMNS")
                .ok()
                .and_then(|value| value.parse::<usize>().ok())
                .filter(|width| *width > 0)
        })
        .unwrap_or(usize::MAX)
}

/// Auto target row width = ⌊0.7 × term_width⌋ when configured to None.
/// When term_width is unknown (`usize::MAX`), fall back to a reasonable 80.
fn default_target_row_width(term_width: usize) -> usize {
    if term_width == usize::MAX {
        80
    } else {
        ((term_width as f64) * 0.7) as usize
    }
}

#[derive(Clone, Copy)]
enum BalanceDirection {
    Up,
    Down,
}

/// V3.1 row balancing — greedy, capped by `MAX_PASSES`.
///
/// Walks the row list, finds the shortest row, picks the narrower neighbor,
/// merges if `width + separator + neighbor ≤ cap`, otherwise stops. The cap is
/// `min(term_width, max(target_row_width, min_row_width))` to respect both the
/// physical terminal limit and the user's target soft cap.
///
/// Pre-merged rows produced by `merge_groups` are atomic: they are never
/// split here, only used as neighbors. Rows shorter than `min_row_width`
/// trigger merging; once all rows clear that bar, the loop exits early.
fn balance_rows(
    mut rows: Vec<String>,
    term_width: usize,
    min_row_width: usize,
    target_row_width: usize,
    separator: &str,
) -> Vec<String> {
    const MAX_PASSES: usize = 5;

    if rows.len() < 2 || min_row_width == 0 {
        return rows;
    }

    let cap = term_width.min(target_row_width.max(min_row_width));
    let separator_width = visible_width(separator);

    for _ in 0..MAX_PASSES {
        let Some((idx, width)) = rows
            .iter()
            .enumerate()
            .map(|(idx, row)| (idx, visible_width(row)))
            .min_by_key(|(_, width)| *width)
        else {
            break;
        };

        if width >= min_row_width {
            break;
        }

        let prev_width = if idx > 0 {
            Some(visible_width(&rows[idx - 1]))
        } else {
            None
        };
        let next_width = if idx + 1 < rows.len() {
            Some(visible_width(&rows[idx + 1]))
        } else {
            None
        };

        let (direction, neighbor_width) = match (prev_width, next_width) {
            (Some(prev), Some(next)) if prev <= next => (BalanceDirection::Up, prev),
            (Some(_), Some(next)) => (BalanceDirection::Down, next),
            (Some(prev), None) => (BalanceDirection::Up, prev),
            (None, Some(next)) => (BalanceDirection::Down, next),
            (None, None) => break,
        };

        if width + separator_width + neighbor_width > cap {
            break;
        }

        match direction {
            BalanceDirection::Up => {
                rows[idx - 1] = format!("{}{}{}", rows[idx - 1], separator, rows[idx]);
                rows.remove(idx);
            }
            BalanceDirection::Down => {
                rows[idx] = format!("{}{}{}", rows[idx], separator, rows[idx + 1]);
                rows.remove(idx + 1);
            }
        }
    }

    rows
}

/// Build O(1) lookup from segment id → its merge group (if any).
/// Single-element groups are filtered out — they would never trigger a merge.
fn build_merge_lookup(merge_groups: &[MergeGroup]) -> HashMap<SegmentId, &MergeGroup> {
    let mut lookup = HashMap::new();

    for group in merge_groups.iter().filter(|group| group.ids.len() > 1) {
        for id in &group.ids {
            lookup.entry(*id).or_insert(group);
        }
    }

    lookup
}

pub struct StatusLineGenerator {
    config: Config,
}

impl StatusLineGenerator {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn generate(&self, segments: Vec<(SegmentConfig, SegmentData)>) -> Vec<String> {
        let enabled_segments: Vec<_> = segments
            .into_iter()
            .filter(|(config, _)| config.enabled)
            .collect();

        match self.config.layout.mode {
            LayoutMode::Compact => vec![self.render_single_line(&enabled_segments)],
            LayoutMode::Expanded => self.render_expanded(&enabled_segments),
        }
    }

    /// Compact (v1.1.2) path: render every enabled segment, join with the
    /// configured separator (or Powerline arrows). Output is byte-identical
    /// to the pre-V3 generator when `layout = LayoutConfig::default()`.
    fn render_single_line(&self, enabled_segments: &[(SegmentConfig, SegmentData)]) -> String {
        let mut output = Vec::new();

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
            self.join_with_powerline_arrows(&output, enabled_segments)
        } else {
            // For all other separators, use white color and simple join
            self.join_with_white_separators(&output)
        }
    }

    /// V3 expanded path with V3.1 row balancing applied as a post-pass.
    fn render_expanded(&self, enabled_segments: &[(SegmentConfig, SegmentData)]) -> Vec<String> {
        self.render_expanded_with_width(enabled_segments, detect_terminal_width())
    }

    /// Width-injected variant exposed for tests so the result is deterministic.
    fn render_expanded_with_width(
        &self,
        enabled_segments: &[(SegmentConfig, SegmentData)],
        term_width: usize,
    ) -> Vec<String> {
        let mut rows = self.expand_with_merge_groups(enabled_segments, term_width);

        if self.config.layout.balance && rows.len() > 1 {
            let target = self
                .config
                .layout
                .target_row_width
                .unwrap_or_else(|| default_target_row_width(term_width));
            rows = balance_rows(
                rows,
                term_width,
                self.config.layout.min_row_width,
                target,
                &self.expanded_separator(),
            );
        }

        rows
    }

    /// Apply user-declared `merge_groups` as hard constraints. Adjacent enabled
    /// segments that share a group merge onto a single row joined by
    /// `expanded_separator()`. If the merged row exceeds `term_width` and
    /// `auto_stack_on_overflow` is on, fall back to per-segment rows.
    fn expand_with_merge_groups(
        &self,
        enabled_segments: &[(SegmentConfig, SegmentData)],
        term_width: usize,
    ) -> Vec<String> {
        let merge_lookup = build_merge_lookup(&self.config.layout.merge_groups);
        let separator = self.expanded_separator();
        let auto_stack = self.config.layout.auto_stack_on_overflow;
        let mut rows = Vec::new();
        let mut index = 0;

        while index < enabled_segments.len() {
            let segment_id = enabled_segments[index].0.id;

            if let Some(group) = merge_lookup.get(&segment_id) {
                let mut consumed = Vec::new();
                let mut cursor = index;

                while cursor < enabled_segments.len()
                    && group.ids.contains(&enabled_segments[cursor].0.id)
                {
                    consumed.push(cursor);
                    cursor += 1;
                }

                if consumed.len() > 1 {
                    let merged =
                        self.render_segments_joined(enabled_segments, &consumed, &separator);

                    if !auto_stack || visible_width(&merged) <= term_width {
                        if !merged.is_empty() {
                            rows.push(merged);
                        }
                    } else {
                        for segment_index in consumed {
                            let (config, data) = &enabled_segments[segment_index];
                            let rendered = self.render_segment(config, data);
                            if !rendered.is_empty() {
                                rows.push(rendered);
                            }
                        }
                    }

                    index = cursor;
                    continue;
                }
            }

            let (config, data) = &enabled_segments[index];
            let rendered = self.render_segment(config, data);
            if !rendered.is_empty() {
                rows.push(rendered);
            }
            index += 1;
        }

        rows
    }

    fn render_segments_joined(
        &self,
        enabled_segments: &[(SegmentConfig, SegmentData)],
        indices: &[usize],
        separator: &str,
    ) -> String {
        indices
            .iter()
            .filter_map(|index| {
                let (config, data) = &enabled_segments[*index];
                let rendered = self.render_segment(config, data);
                (!rendered.is_empty()).then_some(rendered)
            })
            .collect::<Vec<_>>()
            .join(separator)
    }

    /// Separator used to join segments that share a row in expanded mode
    /// or that get fused by V3.1 balancing. Hard-coded to a white-grey ` │ `
    /// so the visual weight stays consistent regardless of `style.separator`
    /// (which may be a Powerline arrow that doesn't compose well mid-row).
    fn expanded_separator(&self) -> String {
        "\x1b[37m │ \x1b[0m".to_string()
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
        let full_output = self.generate(segments).join("\n");

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
    use crate::config::{
        ColorConfig, IconConfig, LayoutConfig, LayoutMode, MergeGroup, SegmentId, StyleConfig,
        StyleMode, TextStyleConfig,
    };
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

    // ===== V3 multi-line layout + V3.1 row-balance tests =====

    fn test_config(layout: LayoutConfig) -> Config {
        Config {
            style: StyleConfig {
                mode: StyleMode::Plain,
                separator: " │ ".to_string(),
            },
            segments: Vec::new(),
            theme: "test".to_string(),
            layout,
        }
    }

    fn expanded_layout() -> LayoutConfig {
        LayoutConfig {
            mode: LayoutMode::Expanded,
            balance: false,
            ..LayoutConfig::default()
        }
    }

    fn make_render_segment(
        id: SegmentId,
        icon: &str,
        primary: &str,
    ) -> (SegmentConfig, SegmentData) {
        make_render_segment_with_enabled(id, icon, primary, true)
    }

    fn make_render_segment_with_enabled(
        id: SegmentId,
        icon: &str,
        primary: &str,
        enabled: bool,
    ) -> (SegmentConfig, SegmentData) {
        (
            SegmentConfig {
                id,
                enabled,
                icon: IconConfig {
                    plain: icon.to_string(),
                    nerd_font: icon.to_string(),
                },
                colors: ColorConfig {
                    icon: None,
                    text: None,
                    background: None,
                    warning: None,
                    critical: None,
                },
                styles: TextStyleConfig::default(),
                options: HashMap::new(),
            },
            SegmentData {
                primary: primary.to_string(),
                secondary: String::new(),
                metadata: HashMap::new(),
                level: ThresholdLevel::Normal,
            },
        )
    }

    #[test]
    fn compact_layout_keeps_v2_single_line_bytes() {
        let config = test_config(LayoutConfig::default());
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Directory, "D", "dir"),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(lines, vec!["M model\x1b[37m │ \x1b[0mD dir".to_string()]);
    }

    #[test]
    fn expanded_layout_renders_one_row_per_segment() {
        let config = test_config(expanded_layout());
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Directory, "D", "dir"),
            make_render_segment(SegmentId::Git, "G", "git"),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(
            lines,
            vec![
                "M model".to_string(),
                "D dir".to_string(),
                "G git".to_string(),
            ]
        );
    }

    #[test]
    fn expanded_merge_group_fits_same_row() {
        let mut layout = expanded_layout();
        layout.merge_groups = vec![MergeGroup {
            ids: vec![SegmentId::Model, SegmentId::Directory],
        }];
        let config = test_config(layout);
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Directory, "D", "dir"),
            make_render_segment(SegmentId::Git, "G", "git"),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(
            lines,
            vec![
                "M model\x1b[37m │ \x1b[0mD dir".to_string(),
                "G git".to_string(),
            ]
        );
    }

    #[test]
    fn expanded_merge_group_overflow_stacks_when_enabled() {
        let mut layout = expanded_layout();
        layout.merge_groups = vec![MergeGroup {
            ids: vec![SegmentId::Model, SegmentId::Directory],
        }];
        layout.auto_stack_on_overflow = true;
        let config = test_config(layout);
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "abcdefghij"),
            make_render_segment(SegmentId::Directory, "D", "klmnopqrst"),
        ];

        let lines = StatusLineGenerator::new(config).render_expanded_with_width(&segments, 20);

        assert_eq!(
            lines,
            vec!["M abcdefghij".to_string(), "D klmnopqrst".to_string()]
        );
    }

    #[test]
    fn expanded_merge_group_overflow_keeps_merged_when_stack_disabled() {
        let mut layout = expanded_layout();
        layout.merge_groups = vec![MergeGroup {
            ids: vec![SegmentId::Model, SegmentId::Directory],
        }];
        layout.auto_stack_on_overflow = false;
        let config = test_config(layout);
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "abcdefghij"),
            make_render_segment(SegmentId::Directory, "D", "klmnopqrst"),
        ];

        let lines = StatusLineGenerator::new(config).render_expanded_with_width(&segments, 20);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0], "M abcdefghij\x1b[37m │ \x1b[0mD klmnopqrst");
    }

    #[test]
    fn expanded_disabled_segment_is_skipped() {
        let config = test_config(expanded_layout());
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment_with_enabled(SegmentId::Directory, "D", "dir", false),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(lines, vec!["M model".to_string()]);
    }

    #[test]
    fn layout_default_is_compact_when_toml_omits_layout() {
        let config: Config = toml::from_str(
            r#"
theme = "test"

[style]
mode = "plain"
separator = " │ "

[[segments]]
id = "model"
enabled = true
options = {}

[segments.icon]
plain = "M"
nerd_font = "M"

[segments.colors]

[segments.styles]
text_bold = false
"#,
        )
        .unwrap();

        assert_eq!(config.layout, LayoutConfig::default());
    }

    #[test]
    fn balance_disabled_passthrough() {
        let config = test_config(expanded_layout());
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Directory, "D", "dir"),
            make_render_segment(SegmentId::Git, "G", "git"),
        ];

        let lines = StatusLineGenerator::new(config).render_expanded_with_width(&segments, 120);

        assert_eq!(
            lines,
            vec![
                "M model".to_string(),
                "D dir".to_string(),
                "G git".to_string(),
            ]
        );
    }

    #[test]
    fn balance_skip_single_row() {
        let rows = vec!["tiny".to_string()];

        assert_eq!(
            balance_rows(rows.clone(), 120, 20, 80, "\x1b[37m │ \x1b[0m"),
            rows
        );
    }

    #[test]
    fn balance_merge_short_to_narrower_neighbor() {
        let rows = vec![
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
            "tiny".to_string(),
            "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
        ];

        let balanced = balance_rows(rows, 120, 20, 80, "\x1b[37m │ \x1b[0m");

        assert_eq!(
            balanced,
            vec![
                "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\x1b[37m │ \x1b[0mtiny".to_string(),
                "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb".to_string(),
            ]
        );
    }

    #[test]
    fn balance_short_at_end_merges_up() {
        let rows = vec!["aaaaaaaaaaaaaaaaaaaa".to_string(), "tiny".to_string()];

        let balanced = balance_rows(rows, 120, 20, 80, "\x1b[37m │ \x1b[0m");

        assert_eq!(
            balanced,
            vec!["aaaaaaaaaaaaaaaaaaaa\x1b[37m │ \x1b[0mtiny".to_string()]
        );
    }

    #[test]
    fn balance_short_at_start_merges_down() {
        let rows = vec!["tiny".to_string(), "aaaaaaaaaaaaaaaaaaaa".to_string()];

        let balanced = balance_rows(rows, 120, 20, 80, "\x1b[37m │ \x1b[0m");

        assert_eq!(
            balanced,
            vec!["tiny\x1b[37m │ \x1b[0maaaaaaaaaaaaaaaaaaaa".to_string()]
        );
    }

    #[test]
    fn balance_stops_at_cap() {
        let rows = vec!["aaaaaaaaaaaaaaaaaaaa".to_string(), "tiny".to_string()];

        let balanced = balance_rows(rows.clone(), 120, 20, 22, "\x1b[37m │ \x1b[0m");

        assert_eq!(balanced, rows);
    }

    #[test]
    fn balance_max_passes_safety() {
        let rows = vec!["x".to_string(); 10];

        let balanced = balance_rows(rows, 120, 100, 120, "\x1b[37m │ \x1b[0m");

        assert_eq!(balanced.len(), 5);
    }

    #[test]
    fn compact_layout_skips_balance() {
        let layout = LayoutConfig {
            balance: true,
            min_row_width: 100,
            ..LayoutConfig::default()
        };
        let config = test_config(layout);
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Directory, "D", "dir"),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(lines, vec!["M model\x1b[37m │ \x1b[0mD dir".to_string()]);
    }

    /// merge_group declared but only one of its members is enabled — the
    /// segment must fall through to the single-segment render path.
    #[test]
    fn expanded_merge_group_with_single_member_falls_through() {
        let mut layout = expanded_layout();
        layout.merge_groups = vec![MergeGroup {
            ids: vec![SegmentId::Model, SegmentId::Directory],
        }];
        let config = test_config(layout);
        let segments = vec![
            make_render_segment(SegmentId::Model, "M", "model"),
            make_render_segment(SegmentId::Git, "G", "git"),
        ];

        let lines = StatusLineGenerator::new(config).generate(segments);

        assert_eq!(lines, vec!["M model".to_string(), "G git".to_string()]);
    }

    /// `cap = min(term_width, max(target_row_width, min_row_width))` —
    /// when target < min, the floor is min_row_width, so the merge stays
    /// allowed up to min_row_width even with a tighter target.
    #[test]
    fn balance_cap_uses_min_when_target_below_min() {
        let rows = vec!["short".to_string(), "tail".to_string()];

        let balanced = balance_rows(rows, 120, 20, 5, "\x1b[37m │ \x1b[0m");

        assert_eq!(
            balanced,
            vec!["short\x1b[37m │ \x1b[0mtail".to_string()]
        );
    }
}
