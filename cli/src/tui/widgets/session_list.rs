use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const CODE_BG: Color = Color::Rgb(22, 24, 29);

fn centered(width: u16, height: u16, r: Rect) -> Rect {
    let popup = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(r.height.saturating_sub(height) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(r.width.saturating_sub(width) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(popup[1])[1]
}

pub struct SessionListModal<'a> {
    state: &'a AppState,
}

impl<'a> SessionListModal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for SessionListModal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let (sessions, selected) = match &self.state.modal {
            crate::tui::state::ModalState::SessionList {
                sessions,
                selected,
                filter: _,
            } => (sessions.as_slice(), *selected),
            _ => return,
        };
        let h = (sessions.len() + 5).min(20) as u16;
        let area = centered(70, h.max(8), screen);
        Clear.render(area, buf);
        let mut lines = vec![
            Line::from(Span::styled(
                " Sessions ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
        ];
        if sessions.is_empty() {
            lines.push(Line::raw("  (no sessions)"));
        } else {
            for (i, (id, line)) in sessions.iter().enumerate() {
                let prefix = if i == selected { "▶ " } else { "  " };
                let style = if i == selected {
                    Style::default().fg(Color::Black).bg(TOOL_COLOR)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(Span::styled(
                    format!("{prefix}{id:.8}  {line}"),
                    style,
                )));
            }
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            " ↑↓ navigate  Enter resume  Esc cancel ",
            Style::default().fg(Color::DarkGray),
        )));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(TOOL_COLOR))
            .style(Style::default().bg(CODE_BG));
        Paragraph::new(Text::from(lines))
            .block(block)
            .render(area, buf);
    }
}
