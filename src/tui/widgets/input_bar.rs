use crate::tui::state::{AppState, CommandMode};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);

pub struct InputBar<'a> {
    state: &'a AppState,
}

impl<'a> InputBar<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    /// Compute the cursor position relative to the given area.
    /// Returns `(x, y)` in screen coordinates.
    pub fn cursor_position(&self, area: Rect) -> (u16, u16) {
        let state = self.state;
        let y = area.y + (area.height.saturating_sub(1)) / 2;

        if !matches!(state.command_mode, CommandMode::None) {
            let hint = state.command_mode.prompt();
            let hint_w = hint.len() + 3;
            let cursor_w = state.input[..state.cursor_pos.min(state.input.len())].width();
            let cursor_x = (hint_w + cursor_w) as u16;
            let max_x = area.width.saturating_sub(1);
            return (area.x + cursor_x.min(max_x), y);
        }

        let cursor_width = state.input[..state.cursor_pos.min(state.input.len())].width();
        let cursor_x = 4 + cursor_width.min(area.width.saturating_sub(6) as usize) as u16;
        (area.x + cursor_x, y)
    }
}

impl<'a> Widget for InputBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = self.state;

        // ── Command-mode prompt ──
        if !matches!(state.command_mode, CommandMode::None) {
            let hint = state.command_mode.prompt();
            let prompt = Span::styled(
                format!(" {hint} "),
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD),
            );
            let text = Span::raw(&state.input);
            let cursor = if state.input.is_empty() {
                Span::styled(" ", Style::default().bg(Color::White))
            } else {
                Span::raw("")
            };

            let line = Line::from(vec![prompt, text, cursor]);
            let block = Block::default()
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(Color::Rgb(229, 192, 123)));

            let inner = block.inner(area);
            let chunks = Layout::vertical([
                Constraint::Length((inner.height.saturating_sub(1)) / 2),
                Constraint::Length(1),
                Constraint::Min(0),
            ])
            .split(inner);

            block.render(area, buf);
            Paragraph::new(line).render(chunks[1], buf);
            return;
        }

        // ── Normal prompt ──
        let prompt = Span::styled(
            " ❯ ",
            Style::default()
                .fg(SUCCESS_COLOR)
                .add_modifier(Modifier::BOLD),
        );
        let text = Span::raw(&state.input);
        let cursor = if state.input.is_empty() {
            Span::styled(" ", Style::default().bg(Color::White))
        } else {
            Span::raw("")
        };

        let line = Line::from(vec![prompt, text, cursor]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

        let inner = block.inner(area);
        let chunks = Layout::vertical([
            Constraint::Length((inner.height.saturating_sub(1)) / 2),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(inner);

        block.render(area, buf);
        Paragraph::new(line).render(chunks[1], buf);
    }
}
