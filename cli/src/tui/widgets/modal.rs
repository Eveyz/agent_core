use crate::tui::state::{AppState, ModalState};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};

const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const CODE_BG: Color = Color::Rgb(22, 24, 29);

/// Center a fixed-size rect inside the given parent rect using Layout constraints.
fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length((r.height.saturating_sub(height)) / 2),
            Constraint::Length(height),
            Constraint::Min(0),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length((r.width.saturating_sub(width)) / 2),
            Constraint::Length(width),
            Constraint::Min(0),
        ])
        .split(popup_layout[1])[1]
}

pub struct Modal<'a> {
    state: &'a AppState,
}

impl<'a> Modal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for Modal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        match &self.state.modal {
            ModalState::ModelPicker { models, selected } => {
                let modal_w = 50u16;
                let modal_h = (models.len() + 4).min(16) as u16;
                let area = centered_rect(modal_w, modal_h, screen);
                Clear.render(area, buf);

                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(
                    " Select Model ",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::raw(""));
                for (i, name) in models.iter().enumerate() {
                    let prefix = if i == *selected { "▶ " } else { "  " };
                    let line_str = format!("{}{}", prefix, name);
                    if i == *selected {
                        lines.push(Line::from(Span::styled(
                            line_str,
                            Style::default().fg(Color::Black).bg(TOOL_COLOR),
                        )));
                    } else {
                        lines.push(Line::from(Span::raw(line_str)));
                    }
                }
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    " ↑↓ navigate  Enter select  Esc cancel ",
                    Style::default().fg(Color::DarkGray),
                )));

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(TOOL_COLOR))
                    .style(Style::default().bg(CODE_BG));

                Paragraph::new(Text::from(lines)).block(block).render(area, buf);
            }
            ModalState::ModelForm {
                provider,
                base_url,
                api_key,
                model_id,
                active_field,
            } => {
                let modal_w = 60u16;
                let modal_h = 14u16;
                let area = centered_rect(modal_w, modal_h, screen);
                Clear.render(area, buf);

                let fields: [(&str, &str); 4] = [
                    ("Provider", provider),
                    ("Base URL", base_url),
                    ("API Key", api_key),
                    ("Model ID", model_id),
                ];

                let mut lines: Vec<Line> = Vec::new();
                lines.push(Line::from(Span::styled(
                    " Register New Model ",
                    Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::raw(""));
                for (i, (label, val)) in fields.iter().enumerate() {
                    let cursor = if i == *active_field { "▌" } else { "" };
                    let display = format!("{:>10}: {}{}", label, val, cursor);
                    if i == *active_field {
                        lines.push(Line::from(Span::styled(
                            display,
                            Style::default().fg(Color::Black).bg(TOOL_COLOR),
                        )));
                    } else {
                        lines.push(Line::from(Span::raw(display)));
                    }
                }
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    " ↑↓ switch field  Enter submit  Esc cancel ",
                    Style::default().fg(Color::DarkGray),
                )));

                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(TOOL_COLOR))
                    .style(Style::default().bg(CODE_BG));

                Paragraph::new(Text::from(lines)).block(block).render(area, buf);
            }
            ModalState::None => {}
        }
    }
}
