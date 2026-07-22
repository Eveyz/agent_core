use crate::tui::state::{AppState, ModalState};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, Paragraph, Widget},
};
use unicode_width::UnicodeWidthStr;

const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const CODE_BG: Color = Color::Rgb(22, 24, 29);

fn centered_rect(width: u16, height: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
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
            ModalState::ModelPicker {
                models,
                selected,
                filter,
                current,
            } => {
                let filtered: Vec<&String> = models
                    .iter()
                    .filter(|m| filter.is_empty() || m.contains(filter.as_str()))
                    .collect();
                let modal_w = 56u16;
                let modal_h = (filtered.len() + 6).min(18) as u16;
                let area = centered_rect(modal_w, modal_h, screen);
                Clear.render(area, buf);
                let mut lines = vec![
                    Line::from(Span::styled(
                        " Select Model ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )),
                    Line::from(format!(" Filter: {filter}_")),
                    Line::raw(""),
                ];
                for (i, name) in filtered.iter().enumerate() {
                    let mark = if **name == *current { "*" } else { " " };
                    let prefix = if i == *selected { "▶" } else { " " };
                    let style = if i == *selected {
                        Style::default().fg(Color::Black).bg(TOOL_COLOR)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(
                        format!("{prefix}{mark} {name}"),
                        style,
                    )));
                }
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    " type to filter  ↑↓  Enter  Esc ",
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
                cursor,
                show_api_key,
            } => {
                let area = centered_rect(64, 14, screen);
                Clear.render(area, buf);
                let key_disp = if *show_api_key {
                    api_key.clone()
                } else {
                    "*".repeat(api_key.chars().count())
                };
                let fields: [(&str, &str); 4] = [
                    ("Provider", provider),
                    ("Base URL", base_url),
                    ("API Key", &key_disp),
                    ("Model ID", model_id),
                ];
                let mut lines = vec![
                    Line::from(Span::styled(
                        " Register New Model ",
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                    )),
                    Line::raw(""),
                ];
                for (i, (label, val)) in fields.iter().enumerate() {
                    let cursor_ch = if i == *active_field { "▌" } else { "" };
                    let display = format!("{:>10}: {}{}", label, val, cursor_ch);
                    let style = if i == *active_field {
                        Style::default().fg(Color::Black).bg(TOOL_COLOR)
                    } else {
                        Style::default()
                    };
                    lines.push(Line::from(Span::styled(display, style)));
                }
                let _ = cursor; // mid-field editing tracked in state
                lines.push(Line::raw(""));
                lines.push(Line::from(Span::styled(
                    " ↑↓ field  Enter submit  Esc cancel ",
                    Style::default().fg(Color::DarkGray),
                )));
                let block = Block::default()
                    .borders(Borders::ALL)
                    .border_type(BorderType::Rounded)
                    .border_style(Style::default().fg(TOOL_COLOR))
                    .style(Style::default().bg(CODE_BG));
                Paragraph::new(Text::from(lines)).block(block).render(area, buf);
            }
            _ => {}
        }
    }
}

pub struct AnswerModal<'a> {
    state: &'a AppState,
}

impl<'a> AnswerModal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for AnswerModal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let (prompt, input) = match &self.state.modal {
            ModalState::Answer { prompt, input, .. } => (prompt.as_str(), input.as_str()),
            _ => return,
        };
        let area = centered_rect(64, 10, screen);
        Clear.render(area, buf);
        let lines = vec![
            Line::from(Span::styled(
                " Input Required ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw(prompt.to_string()),
            Line::raw(""),
            Line::from(format!("❯ {input}▌")),
            Line::raw(""),
            Line::from(Span::styled(
                " Enter submit  Esc cancel ",
                Style::default().fg(Color::DarkGray),
            )),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(TOOL_COLOR))
            .style(Style::default().bg(CODE_BG));
        Paragraph::new(Text::from(lines)).block(block).render(area, buf);
    }
}

pub struct RewindModal<'a> {
    state: &'a AppState,
}

impl<'a> RewindModal<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for RewindModal<'a> {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let (points, selected) = match &self.state.modal {
            ModalState::RewindList { points, selected } => (points.as_slice(), *selected),
            _ => return,
        };
        let area = centered_rect(70, (points.len() + 5).min(18) as u16, screen);
        Clear.render(area, buf);
        let mut lines = vec![
            Line::from(Span::styled(
                " Rewind ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
        ];
        for (i, (idx, preview)) in points.iter().enumerate() {
            let prefix = if i == selected { "▶ " } else { "  " };
            let style = if i == selected {
                Style::default().fg(Color::Black).bg(TOOL_COLOR)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(
                format!("{prefix}[{idx}] {preview}"),
                style,
            )));
        }
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            " Enter rewind  Esc cancel ",
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

pub struct QuitConfirmModal;

impl Widget for QuitConfirmModal {
    fn render(self, screen: Rect, buf: &mut Buffer) {
        let area = centered_rect(46, 7, screen);
        Clear.render(area, buf);
        let lines = vec![
            Line::from(Span::styled(
                " Quit? ",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::raw(""),
            Line::raw("  Press y / Enter to quit, Esc to cancel."),
        ];
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(224, 108, 117)))
            .style(Style::default().bg(CODE_BG));
        Paragraph::new(Text::from(lines)).block(block).render(area, buf);
    }
}

#[allow(dead_code)]
fn _width_unused(s: &str) -> usize {
    s.width()
}
