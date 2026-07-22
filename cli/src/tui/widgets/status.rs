use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget},
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
            "streaming" | "thinking" | "responding" | "running tools" => (
                format!("{} [esc abort]", state.agent_state),
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD),
            ),
            "idle" => ("idle".into(), Style::default().fg(SUCCESS_COLOR)),
            other => (other.to_string(), Style::default().fg(Color::White)),
        };

        let mut spans = vec![
            Span::styled(
                " ageverse ",
                Style::default()
                    .fg(Color::Rgb(97, 175, 239))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("│ "),
            Span::styled(&state.model, Style::default().fg(Color::Rgb(140, 148, 168))),
            Span::raw(" │ "),
            Span::styled(
                format!("{}tok", state.tokens),
                Style::default().fg(Color::Rgb(140, 148, 168)),
            ),
            Span::raw(" │ "),
            Span::styled(
                format!("{:.0}%", state.context_pct),
                Style::default().fg(Color::Rgb(140, 148, 168)),
            ),
            Span::raw(" │ "),
            Span::styled(
                state.permission_label.clone(),
                Style::default().fg(Color::Rgb(140, 148, 168)),
            ),
            Span::raw(" │ "),
            Span::styled(state_text, state_style),
        ];
        if state.steer_queue_depth > 0 {
            spans.push(Span::raw(" │ "));
            spans.push(Span::styled(
                format!("steer:{}", state.steer_queue_depth),
                Style::default().fg(Color::Rgb(198, 120, 221)),
            ));
        }
        if !state.is_follow_mode() {
            spans.push(Span::raw(" │ "));
            spans.push(Span::styled(
                "⬆ paused — G or End to follow",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ));
        }
        if !state.session_short.is_empty() {
            spans.push(Span::raw(" │ "));
            spans.push(Span::styled(
                state.session_short.clone(),
                Style::default().fg(Color::DarkGray),
            ));
        }

        Paragraph::new(Line::from(spans)).render(area, buf);
    }
}
