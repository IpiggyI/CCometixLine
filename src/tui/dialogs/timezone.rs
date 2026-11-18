//! Picking ccline's timezone: quick presets or any IANA name / UTC offset,
//! with a live clock preview.

use super::{Action, Dialog, Reply};
use crate::i18n::t;
use crate::tf;
use crate::timezone::{QUICK_TIMEZONES, Timezone, format_offset};
use crate::tui::doc::Doc;
use crate::tui::pages::Cx;
use crate::tui::ui::{self, TextInput};
use chrono::DateTime;
use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent};
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

const WIDTH: u16 = 72;

pub struct TimezoneDialog {
    input: TextInput,
    pick: Option<usize>,
    error: Option<String>,
}

impl TimezoneDialog {
    pub fn new(current: &Timezone) -> Self {
        let input = match current {
            Timezone::Auto => TextInput::default(),
            Timezone::Custom(s) => TextInput::new(s),
        };
        let mut d = TimezoneDialog {
            input,
            pick: None,
            error: None,
        };
        d.sync_pick();
        d
    }

    fn sync_pick(&mut self) {
        let raw = self.input.text();
        let trimmed = raw.trim();
        self.pick = if trimmed.is_empty() {
            Some(0)
        } else {
            QUICK_TIMEZONES
                .iter()
                .position(|(id, _)| id.eq_ignore_ascii_case(trimmed))
        };
    }

    fn select_pick(&mut self, idx: usize) {
        self.pick = Some(idx);
        let id = QUICK_TIMEZONES[idx].0;
        self.input = if idx == 0 {
            TextInput::default()
        } else {
            TextInput::new(id)
        };
    }

    fn preview_span(&self, cx: &Cx) -> Span<'static> {
        let raw = self.input.text();
        match Timezone::parse(&raw) {
            Ok(tz) => {
                let offset = tz.resolve(cx.env.now, cx.env.tz);
                let clock = DateTime::from_timestamp(cx.env.now, 0)
                    .map(|t| t.with_timezone(&offset).format("%H:%M").to_string())
                    .unwrap_or_default();
                Span::styled(
                    format!("{} · {clock}", format_offset(offset)),
                    Style::default().fg(ui::OK),
                )
            }
            Err(_) => Span::styled(t("unknown timezone"), Style::default().fg(ui::WARN)),
        }
    }
}

impl Dialog for TimezoneDialog {
    fn render(&mut self, f: &mut Frame, area: Rect, cx: &Cx) {
        let height = QUICK_TIMEZONES.len() as u16 + 8 + u16::from(self.error.is_some());
        let rect = ui::popup(f, area, WIDTH, height);
        let input_line = Line::from(
            [
                vec![Span::raw(format!(" {}  ", ui::pad(t("Timezone"), 10)))],
                self.input.line(26, true, t("auto")).spans,
                vec![Span::raw("  "), self.preview_span(cx)],
            ]
            .concat(),
        );
        let mut text = vec![
            input_line,
            Line::styled(
                t(" auto, Asia/Shanghai, Beijing, +08:00, UTC+8, America/New_York…"),
                ui::muted(),
            ),
            Line::raw(""),
        ];
        for (i, (id, desc)) in QUICK_TIMEZONES.iter().enumerate() {
            let on = self.pick == Some(i);
            text.push(Line::from(ui::clip(
                vec![
                    Span::styled(if on { ui::SELECTED } else { "  " }, ui::accent()),
                    Span::styled(
                        format!("{id:<20}"),
                        if on { ui::accent() } else { Style::default() },
                    ),
                    Span::styled(t(desc).to_string(), ui::muted()),
                ],
                WIDTH as usize - 2,
            )));
        }
        if let Some(e) = &self.error {
            text.push(Line::from(ui::clip(
                vec![Span::styled(format!(" {e}"), Style::default().fg(ui::BAD))],
                WIDTH as usize - 2,
            )));
        }
        text.push(Line::raw(""));
        text.push(ui::hints(&[
            ("↑↓", "choose"),
            ("type", "custom"),
            ("enter", "apply"),
            ("esc", "cancel"),
        ]));
        f.render_widget(Paragraph::new(text).block(ui::panel("Timezone")), rect);
    }

    fn handle(&mut self, key: KeyEvent, doc: &mut Doc) -> Reply {
        self.error = None;
        match key.code {
            KeyCode::Esc => return Reply::Close,
            KeyCode::Up => {
                let next = match self.pick {
                    Some(i) => i.saturating_sub(1),
                    None => QUICK_TIMEZONES.len() - 1,
                };
                self.select_pick(next);
                return Reply::Stay;
            }
            KeyCode::Down => {
                let next = match self.pick {
                    Some(i) => (i + 1).min(QUICK_TIMEZONES.len() - 1),
                    None => 0,
                };
                self.select_pick(next);
                return Reply::Stay;
            }
            KeyCode::Enter => {
                return match Timezone::parse(&self.input.text()) {
                    Ok(tz) => {
                        let shown = tz.as_str().to_string();
                        doc.edit(|c, _| c.timezone = tz);
                        Reply::Then(Action::Toast(tf!("Timezone: {tz}", tz = shown)))
                    }
                    Err(_) => {
                        self.error = Some(
                            t("Use auto, an IANA name like Asia/Shanghai, or an offset like +08:00 or UTC+8.")
                                .into(),
                        );
                        Reply::Stay
                    }
                };
            }
            _ => {
                if self.input.handle(key) {
                    self.sync_pick();
                }
            }
        }
        Reply::Stay
    }

    fn paste(&mut self, text: &str) -> bool {
        self.error = None;
        self.input.insert_str(text.trim());
        self.sync_pick();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::doc::tests::temp_paths;
    use ratatui::crossterm::event::KeyModifiers;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn picks_quick_or_typed_timezone_and_rejects_invalid() {
        let (dir, paths) = temp_paths("tz-dialog");
        let mut doc = Doc::open(paths);
        let mut d = TimezoneDialog::new(&doc.config.timezone);
        assert_eq!(d.pick, Some(0));

        // Down selects Asia/Shanghai and Enter applies it.
        d.handle(key(KeyCode::Down), &mut doc);
        assert_eq!(d.input.text(), "Asia/Shanghai");
        assert!(matches!(
            d.handle(key(KeyCode::Enter), &mut doc),
            Reply::Then(Action::Toast(_))
        ));
        assert_eq!(
            doc.config.timezone,
            Timezone::Custom("Asia/Shanghai".into())
        );

        // Typing an invalid value shows an error and keeps the dialog open.
        let mut d = TimezoneDialog::new(&Timezone::Auto);
        for ch in "Mars/Phobos".chars() {
            d.handle(key(KeyCode::Char(ch)), &mut doc);
        }
        assert!(matches!(
            d.handle(key(KeyCode::Enter), &mut doc),
            Reply::Stay
        ));
        assert!(d.error.is_some());

        // Pasting a valid offset applies cleanly.
        let mut d = TimezoneDialog::new(&doc.config.timezone);
        d.handle(key(KeyCode::Up), &mut doc);
        assert_eq!(d.pick, Some(0));
        d.paste("+05:30");
        assert!(matches!(
            d.handle(key(KeyCode::Enter), &mut doc),
            Reply::Then(Action::Toast(_))
        ));
        assert_eq!(doc.config.timezone, Timezone::Custom("+05:30".into()));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
