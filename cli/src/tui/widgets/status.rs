use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);

pub struct StatusBar<'a> {
    state: &'a AppState,
}

impl<'a> StatusBar<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for StatusBar<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let state = self.state;
        let (state_text, state_style) = match state.agent_state.as_str() {
            "streaming" => (
                "working... [esc]",
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK),
            ),
            "thinking" => (
                "thinking... [esc]",
                Style::default()
                    .fg(Color::Rgb(198, 120, 221))
                    .add_modifier(Modifier::BOLD),
            ),
            "responding" => (
                "responding... [esc]",
                Style::default()
                    .fg(Color::Rgb(86, 182, 194))
                    .add_modifier(Modifier::BOLD),
            ),
            "running tools" => (
                "running tools... [esc]",
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD),
            ),
            "idle" => ("idle", Style::default().fg(SUCCESS_COLOR)),
            other => (other, Style::default().fg(Color::White)),
        };

        let mut status_spans = vec![
            Span::styled(
                " 🤖 Agent Core ",
                Style::default()
                    .fg(Color::Rgb(97, 175, 239))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" │ "),
            Span::styled("Model: ", Style::default().fg(Color::Rgb(55, 62, 80))),
            Span::styled(
                format!("{} ", state.model),
                Style::default().fg(Color::Rgb(140, 148, 168)),
            ),
            Span::raw(" │ "),
            Span::styled("Tokens: ", Style::default().fg(Color::Rgb(55, 62, 80))),
            Span::styled(
                format!("{} ", state.tokens),
                Style::default().fg(Color::Rgb(140, 148, 168)),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("{} ", state_text),
                state_style,
            ),
        ];

        if state.scroll > 0 {
            status_spans.push(Span::raw(" │ "));
            status_spans.push(Span::styled(
                "⬆ scroll paused — press End to resume ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }

        let status_text = Line::from(status_spans);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

        Paragraph::new(status_text).block(block).render(area, buf);
    }
}
