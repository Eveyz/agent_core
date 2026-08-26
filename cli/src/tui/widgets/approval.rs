use crate::tui::state::{APPROVAL_CHOICES, AppState};
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

pub struct ApprovalModal<'a> {
    state: &'a AppState,
}

impl<'a> ApprovalModal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for ApprovalModal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let (tool_name, tool_input, danger_level, explanation, selected, subagent_id) =
            match &self.state.modal {
                crate::tui::state::ModalState::Approval {
                    tool_name,
                    tool_input,
                    danger_level,
                    explanation,
                    selected,
                    subagent_id,
                    ..
                } => (
                    tool_name,
                    tool_input,
                    danger_level,
                    explanation,
                    *selected,
                    subagent_id,
                ),
                _ => return,
            };

        let area = centered(72, 18, screen);
        Clear.render(area, buf);

        let mut lines = vec![
            Line::from(Span::styled(
                " Approval Required ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::from(vec![
                Span::raw("Tool: "),
                Span::styled(
                    tool_name.clone(),
                    Style::default().fg(TOOL_COLOR).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(
                    format!("[{danger_level}]"),
                    Style::default().fg(Color::Rgb(224, 108, 117)),
                ),
            ]),
        ];
        if let Some(sid) = subagent_id {
            lines.push(Line::from(format!("Subagent: {sid}")));
        }
        lines.push(Line::raw(explanation.clone()));
        lines.push(Line::raw(""));
        let args =
            serde_json::to_string_pretty(tool_input).unwrap_or_else(|_| tool_input.to_string());
        for l in args.lines().take(6) {
            lines.push(Line::from(Span::styled(
                l.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        }
        lines.push(Line::raw(""));
        for (i, (key, label)) in APPROVAL_CHOICES.iter().enumerate() {
            let prefix = if i == selected { "▶ " } else { "  " };
            let style = if i == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(TOOL_COLOR)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix}{key}. {label}"),
                style,
            )));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            " ↑↓ / 1-5 select   Enter confirm   Esc = Deny ",
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
