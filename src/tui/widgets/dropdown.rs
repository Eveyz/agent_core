use crate::tui::state::AppState;
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    widgets::{Block, BorderType, Borders, Paragraph, Widget},
};

const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const CODE_BG: Color = Color::Rgb(22, 24, 29);

pub struct Dropdown<'a> {
    state: &'a AppState,
}

impl<'a> Dropdown<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Self { state }
    }
}

impl<'a> Widget for Dropdown<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

        let inner = block.inner(area);
        block.render(area, buf);

        let options = &self.state.autocomplete.filtered_options;
        let sel = self.state.autocomplete.selected_index;
        let max_h = inner.height as usize;
        let max_w = inner.width as usize;

        // Compute viewport scroll offset so the selected item stays visible
        let total = options.len();
        let scroll_offset = if total <= max_h {
            0
        } else if sel < max_h / 2 {
            0
        } else if sel + max_h / 2 >= total {
            total.saturating_sub(max_h)
        } else {
            sel.saturating_sub(max_h / 2)
        };

        // Split inner area into rows — each option gets exactly one row.
        let visible = options.iter().enumerate().skip(scroll_offset).take(max_h).count();
        let constraints: Vec<Constraint> = (0..max_h)
            .map(|i| {
                if i < visible {
                    Constraint::Length(1)
                } else {
                    Constraint::Min(0)
                }
            })
            .collect();
        let rows = Layout::vertical(constraints).split(inner);

        // Render each visible option
        for (idx, (i, opt)) in options.iter().enumerate().skip(scroll_offset).take(max_h).enumerate() {
            let row_area = rows[idx];
            let style = if i == sel {
                Style::default()
                    .fg(Color::Black)
                    .bg(TOOL_COLOR)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White).bg(CODE_BG)
            };
            // Fill the entire row background
            buf.set_style(row_area, style);
            // Truncate text to fit within row width
            let truncated: String = opt.chars().take(max_w).collect();
            Paragraph::new(truncated).style(style).render(row_area, buf);
        }

        // Fill remaining empty rows with CODE_BG
        for row_idx in visible..max_h {
            if let Some(&row_area) = rows.get(row_idx) {
                buf.set_style(row_area, Style::default().bg(CODE_BG));
            }
        }
    }
}
