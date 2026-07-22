use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget, Wrap},
};
use unicode_width::UnicodeWidthStr;

const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);

pub fn estimate_height(input: &str, width: u16) -> u16 {
    let inner = width.saturating_sub(4).max(1) as usize;
    let mut rows = 0usize;
    for line in input.lines() {
        let w = line.width().max(1);
        rows += (w + inner - 1) / inner;
    }
    if input.is_empty() || input.ends_with('\n') {
        rows += 1;
    }
    (rows.max(1) as u16).saturating_add(2) // + borders
}

pub struct InputBar<'a> {
    state: &'a AppState,
}

impl<'a> InputBar<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }

    pub fn cursor_position(&self, area: Rect) -> (u16, u16) {
        let state = self.state;
        let before = &state.input[..state.cursor_pos.min(state.input.len())];
        let lines: Vec<&str> = before.split('\n').collect();
        let row = (lines.len().saturating_sub(1)) as u16;
        let col = lines.last().map(|l| l.width()).unwrap_or(0) as u16;
        let y = area.y + 1 + row.saturating_sub(state.input_scroll as u16);
        let x = area.x + 4 + col; // border + prompt
        (x.min(area.x + area.width.saturating_sub(1)), y.min(area.y + area.height.saturating_sub(1)))
    }
}

impl<'a> Widget for InputBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = self.state;
        let prompt = Span::styled(
            " ❯ ",
            Style::default()
                .fg(SUCCESS_COLOR)
                .add_modifier(Modifier::BOLD),
        );
        let mut spans = vec![prompt];
        for (i, line) in state.input.split('\n').enumerate() {
            if i > 0 {
                // Paragraph wraps on newlines via Text lines — build multi Line
            }
            let _ = line;
        }
        let lines: Vec<Line> = {
            let mut out = Vec::new();
            let parts: Vec<&str> = state.input.split('\n').collect();
            for (i, part) in parts.iter().enumerate() {
                if i == 0 {
                    out.push(Line::from(vec![
                        Span::styled(
                            " ❯ ",
                            Style::default()
                                .fg(SUCCESS_COLOR)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::raw((*part).to_string()),
                    ]));
                } else {
                    out.push(Line::from(vec![
                        Span::raw("   "),
                        Span::raw((*part).to_string()),
                    ]));
                }
            }
            if out.is_empty() {
                out.push(Line::from(vec![
                    Span::styled(
                        " ❯ ",
                        Style::default()
                            .fg(SUCCESS_COLOR)
                            .add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(" ", Style::default().bg(Color::White)),
                ]));
            }
            out
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));
        let inner = block.inner(area);
        block.render(area, buf);
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((state.input_scroll as u16, 0))
            .render(inner, buf);
    }
}
