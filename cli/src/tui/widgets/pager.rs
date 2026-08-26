use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget, Wrap},
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

pub struct PagerModal<'a> {
    state: &'a AppState,
}

impl<'a> PagerModal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for PagerModal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let (title, lines, scroll) = match &self.state.modal {
            crate::tui::state::ModalState::Pager {
                title,
                lines,
                scroll,
            } => (title.as_str(), lines.as_slice(), *scroll),
            _ => return,
        };
        let h = screen.height.saturating_sub(4).max(10);
        let w = screen.width.saturating_sub(6).max(40);
        let area = centered(w, h, screen);
        Clear.render(area, buf);
        let mut out = vec![Line::from(Span::styled(
            format!(" {title} "),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ))];
        out.push(Line::raw(""));
        let visible = h.saturating_sub(4) as usize;
        for l in lines.iter().skip(scroll).take(visible) {
            out.push(Line::raw(l.clone()));
        }
        out.push(Line::raw(""));
        out.push(Line::from(Span::styled(
            " PgUp/PgDn scroll · Esc close ",
            Style::default().fg(Color::DarkGray),
        )));
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(TOOL_COLOR))
            .style(Style::default().bg(CODE_BG));
        Paragraph::new(Text::from(out))
            .wrap(Wrap { trim: false })
            .block(block)
            .render(area, buf);
    }
}
