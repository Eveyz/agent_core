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

pub struct HelpModal;

impl Widget for HelpModal {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let area = centered(70, 22, screen);
        Clear.render(area, buf);
        let body = crate::commands::help_text();
        let mut lines: Vec<Line> = vec![Line::from(Span::styled(
            " Help ",
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ))];
        lines.push(Line::raw(""));
        for l in body.lines().take(16) {
            lines.push(Line::raw(l.to_string()));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            " Esc close · ? toggle · G/End follow · y yank · t thought · p pager ",
            Style::default().fg(Color::DarkGray),
        )));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(TOOL_COLOR))
            .style(Style::default().bg(CODE_BG));
        Paragraph::new(Text::from(lines)).block(block).render(area, buf);
    }
}
