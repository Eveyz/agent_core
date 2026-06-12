use crate::tui::markdown;
use crate::tui::state::{CachedBlock, Entry, SubagentState, ToolResult, TurnBlock};
use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Paragraph, Widget, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── Theme ───────────────────────────────────────────────────────────

const USER_BG: Color = Color::Rgb(28, 30, 42);
const CODE_BG: Color = Color::Rgb(22, 24, 29);
const HOVER_BG: Color = Color::Rgb(38, 42, 55);
const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const SUBAGENT_COLOR: Color = Color::Rgb(198, 120, 221);
const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);
const ERROR_COLOR: Color = Color::Rgb(224, 108, 117);
const WARN_COLOR: Color = Color::Rgb(229, 192, 123);

// ── Height estimation (same logic as old render.rs) ─────────────────

pub fn estimate_wrapped_rows(line: &Line<'_>, width: usize) -> usize {
    if width == 0 {
        return 1;
    }
    let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
    if text.is_empty() {
        return 1;
    }
    if text.width() <= width {
        return 1;
    }

    let mut rows = 0usize;
    let mut remaining = text.as_str();

    while !remaining.is_empty() {
        let mut current_width = 0usize;
        let mut last_space_end = 0usize;
        let mut chars_processed = 0usize;

        for (byte_idx, ch) in remaining.char_indices() {
            let cw = ch.width().unwrap_or(0);
            if current_width + cw > width {
                if last_space_end > 0 {
                    remaining = &remaining[last_space_end..];
                } else {
                    remaining = &remaining[byte_idx..];
                }
                rows += 1;
                break;
            }
            if ch == ' ' || ch == '\t' {
                last_space_end = byte_idx + ch.len_utf8();
            }
            current_width += cw;
            chars_processed += 1;
        }
        if chars_processed >= remaining.chars().count() {
            rows += 1;
            break;
        }
    }
    rows.max(1)
}

pub fn compute_block_height(lines: &[Line<'_>], width: u16) -> usize {
    let w = usize::from(width).max(1);
    lines.iter().map(|l| estimate_wrapped_rows(l, w)).sum()
}

// ── Line generators (used by rebuild_cache) ─────────────────────────

pub fn system_block_lines(text: &str) -> Vec<Line<'static>> {
    let style = Style::default().fg(Color::Cyan);
    text.lines().map(|l| Line::from(vec![Span::styled(l.to_string(), style)])).collect()
}

pub fn user_block_lines(text: &str, _width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::raw("")); // top padding
    for line in text.lines() {
        lines.push(Line::raw(line.to_string()));
    }
    lines.push(Line::raw("")); // bottom padding
    lines
}

pub fn thought_block_lines(text: &str, _width: usize, pad: &str) -> Vec<Line<'static>> {
    let style = Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::ITALIC);
    let mut lines = Vec::new();
    let mut first = true;
    for line in text.lines() {
        let line = line.trim_start();
        if line.is_empty() {
            lines.push(Line::raw(""));
            continue;
        }
        if first {
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("💭 Thought: ".to_string(), style.add_modifier(Modifier::BOLD)),
                Span::styled(line.to_string(), style),
            ]));
            first = false;
        } else {
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled(line.to_string(), style),
            ]));
        }
    }
    lines
}

pub fn response_block_lines(text: &str, width: usize, pad: &str) -> Vec<Line<'static>> {
    let inner_width = width.saturating_sub(pad.width());
    let md_lines = markdown::markdown_to_lines(text, inner_width);
    if pad.is_empty() {
        md_lines
    } else {
        md_lines
            .into_iter()
            .map(|mut line| {
                line.spans.insert(0, Span::raw(pad.to_string()));
                line
            })
            .collect()
    }
}

const TOOL_TITLE_HEIGHT: usize = 1;

/// Build the args summary string for the tool title.
fn tool_args_summary(args: &str) -> String {
    if args.trim().is_empty() {
        return String::new();
    }
    let first = args.lines().next().unwrap_or("").trim();
    let stripped = first.strip_prefix("{").unwrap_or(first).strip_suffix("}").unwrap_or(first);
    format!("  {}", stripped.trim())
}

/// Truncate a string to fit within `max_width` display columns, appending `…` if truncated.
fn truncate_str(text: &str, max_width: usize) -> String {
    if text.width() <= max_width {
        return text.to_string();
    }
    let ellipsis = "…";
    let ellipsis_w = ellipsis.width();
    let target = max_width.saturating_sub(ellipsis_w);
    let mut current = 0;
    let mut result = String::new();
    for ch in text.chars() {
        let w = ch.width().unwrap_or(0);
        if current + w > target {
            break;
        }
        result.push(ch);
        current += w;
    }
    result + ellipsis
}

/// Returns only the content lines of a tool block (no title).
/// The title is rendered dynamically by `ToolBlock` to support animation.
pub fn tool_block_lines(
    name: &str,
    _args: &str,
    result: &Option<ToolResult>,
    width: usize,
    pad: &str,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let indent = "    ";
    let max_text_width = width.saturating_sub(indent.width());

    if let Some(r) = result {
        if name == "edit" && !r.is_error {
            let diff_text = r.text.lines().skip(1).collect::<Vec<_>>().join("\n");
            let mut diff_lines = Vec::new();
            super::diff::render_diff_output(&diff_text, &mut diff_lines, width.saturating_sub(indent.width()), pad);
            for mut line in diff_lines {
                line.spans.insert(0, Span::raw(indent.to_string()));
                lines.push(line);
            }
        } else {
            let mut shown = 0;
            for line in r.text.lines().take(8) {
                lines.push(Line::raw(format!("{}{}", indent, truncate_str(line, max_text_width))));
                shown += 1;
            }
            let total = r.text.lines().count();
            if total > shown {
                lines.push(Line::raw(format!("{}… {} more lines", indent, total - shown)));
            }
        }
    } else {
        lines.push(Line::raw(format!("{}⏳  Waiting for result…", indent)));
    }
    lines
}

pub fn subagent_block_lines(sa: &SubagentState, _width: usize, pad: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Top padding
    lines.push(Line::raw(""));

    // Top separator
    lines.push(Line::raw(format!("{}  ────────────────────────────────────────", pad)));

    // Label
    let elapsed_str = if sa.done {
        Some(format_duration(sa.elapsed_ms))
    } else {
        sa.started_at.map(|t| format_duration(t.elapsed().as_millis() as u64))
    };
    let label_text = if sa.done {
        let tag = if sa.success { "✓ complete" } else { "✗ incomplete" };
        match &elapsed_str {
            Some(e) => format!("⚡ Subagent: {} ({} {} iter {})", sa.id, tag, sa.iterations, e),
            None => format!("⚡ Subagent: {} ({} {} iter)", sa.id, tag, sa.iterations),
        }
    } else {
        match &elapsed_str {
            Some(e) => format!("⚡ Subagent: {} (Working... {})", sa.id, e),
            None => format!("⚡ Subagent: {} (Starting...)", sa.id),
        }
    };
    lines.push(Line::raw(format!("{}  {}", pad, label_text)));

    // Activity
    lines.push(Line::raw(format!("{}  {}", pad, sa.current_activity)));

    // Click hint
    lines.push(Line::raw(format!("{}  [Click] details", pad)));

    // Bottom separator
    lines.push(Line::raw(format!("{}  ────────────────────────────────────────", pad)));

    lines
}

pub fn notice_block_lines(msg: &str, _width: usize, pad: &str) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if msg.contains("Available commands:") {
        for line_text in msg.lines() {
            lines.push(Line::raw(format!("{}{}", pad, line_text)));
        }
    } else {
        let icon = if msg.contains("Failed") || msg.contains("Error") || msg.contains("nknown command") {
            "✗"
        } else if msg.contains("registered") || msg.contains("Switched") || msg.contains("cleared") {
            "✓"
        } else if msg.contains("Registered") {
            "ℹ"
        } else {
            "⚠"
        };
        for line_text in msg.lines() {
            lines.push(Line::raw(format!("{}  {}  {}", pad, icon, line_text)));
        }
    }
    lines
}

pub fn error_block_lines(e: &str, _width: usize, pad: &str) -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(
            format!("✗  {e}"),
            Style::default().fg(ERROR_COLOR).add_modifier(Modifier::BOLD),
        ),
    ])]
}

pub fn working_block_lines() -> Vec<Line<'static>> {
    vec![Line::from(vec![
        Span::styled("  ⏳  ", Style::default().fg(Color::Rgb(229, 192, 123)).add_modifier(Modifier::BOLD)),
        Span::styled(
            "Working...",
            Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
        ),
    ])]
}

pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

// ── Cache building helpers ──────────────────────────────────────────

pub fn entry_to_blocks(entry: &Entry, blocks: &mut Vec<CachedBlock>, width: usize) {
    match entry {
        Entry::System { text } => {
            let lines = system_block_lines(text);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::System(text.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
        Entry::User { text } => {
            let lines = user_block_lines(text, width);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::User(text.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
        Entry::Turn { blocks: turn_blocks, .. } => {
            for (i, block) in turn_blocks.iter().enumerate() {
                if i > 0 {
                    blocks.push(CachedBlock::spacing());
                }
                turn_block_to_blocks(block, blocks, width, 0);
            }
        }
    }
}

pub fn turn_block_to_blocks(
    block: &TurnBlock,
    blocks: &mut Vec<CachedBlock>,
    width: usize,
    indent: usize,
) {
    let pad = " ".repeat(indent * 2);
    let inner_width = width.saturating_sub(pad.width());

    match block {
        TurnBlock::Thought(text) => {
            let lines = thought_block_lines(text, width, &pad);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Thought(text.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
        TurnBlock::Response(text) => {
            let lines = response_block_lines(text, inner_width, &pad);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Response(text.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
        TurnBlock::Tool { name, args, result, .. } => {
            let lines = tool_block_lines(name, args, result, inner_width, &pad);
            let content_height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Tool {
                    name: name.clone(),
                    args: args.clone(),
                    result: result.clone(),
                },
                wrapped_height: content_height + TOOL_TITLE_HEIGHT,
                subagent_id: None,
                lines,
            });
        }
        TurnBlock::Subagent(sa) => {
            let lines = subagent_block_lines(sa, inner_width, &pad);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Subagent(sa.clone()),
                wrapped_height: height,
                subagent_id: Some(sa.id.clone()),
                lines,
            });
        }
        TurnBlock::Error(e) => {
            let lines = error_block_lines(e, width, &pad);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Error(e.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
        TurnBlock::Notice(msg) => {
            let lines = notice_block_lines(msg, width, &pad);
            let height = compute_block_height(&lines, width as u16);
            blocks.push(CachedBlock {
                kind: crate::tui::state::BlockKind::Notice(msg.clone()),
                wrapped_height: height,
                subagent_id: None,
                lines,
            });
        }
    }
}

/// Build the block list for the subagent detail view.
pub fn subagent_detail_blocks(
    subagent_id: &str,
    sa: Option<&SubagentState>,
    width: u16,
) -> Vec<CachedBlock> {
    let mut blks: Vec<CachedBlock> = Vec::new();

    // Header breadcrumb
    let header_lines = vec![
        Line::from(vec![
            Span::styled(" ← ", Style::default().fg(SUCCESS_COLOR).add_modifier(Modifier::BOLD)),
            Span::styled(format!("subagent: {}", subagent_id), Style::default().fg(SUBAGENT_COLOR).add_modifier(Modifier::BOLD)),
            Span::styled("  [Esc] back", Style::default().fg(Color::DarkGray)),
        ]),
        Line::from(Span::styled(
            "─".repeat(width as usize),
            Style::default().fg(Color::Rgb(60, 63, 70)),
        )),
    ];
    let header_height = compute_block_height(&header_lines, width);
    blks.push(CachedBlock {
        kind: crate::tui::state::BlockKind::System(String::new()),
        wrapped_height: header_height,
        subagent_id: None,
        lines: header_lines,
    });

    if let Some(sa) = sa {
        for (i, child) in sa.children.iter().enumerate() {
            if i > 0 {
                blks.push(CachedBlock::spacing());
            }
            turn_block_to_blocks(child, &mut blks, width as usize, 0);
        }

        blks.push(CachedBlock::spacing());
        let status_lines = if sa.done {
            let elapsed = format_duration(sa.elapsed_ms);
            let (icon, text, color) = if sa.success {
                ("✓", format!("Completed ({} iterations) {}", sa.iterations, elapsed), SUCCESS_COLOR)
            } else {
                ("✗", format!("Incomplete ({} iterations) {}", sa.iterations, elapsed), ERROR_COLOR)
            };
            vec![Line::from(vec![
                Span::styled(format!(" {} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(text, Style::default().fg(color)),
            ])]
        } else {
            vec![Line::from(vec![
                Span::styled(" ⏳ ", Style::default().fg(WARN_COLOR).add_modifier(Modifier::BOLD)),
                Span::styled("Running...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ])]
        };
        let status_height = compute_block_height(&status_lines, width);
        blks.push(CachedBlock {
            kind: crate::tui::state::BlockKind::Notice(String::new()),
            wrapped_height: status_height,
            subagent_id: None,
            lines: status_lines,
        });
    } else {
        let err_lines = vec![Line::from(Span::styled(
            format!("Subagent '{}' not found", subagent_id),
            Style::default().fg(ERROR_COLOR),
        ))];
        let err_height = compute_block_height(&err_lines, width);
        blks.push(CachedBlock {
            kind: crate::tui::state::BlockKind::Error(String::new()),
            wrapped_height: err_height,
            subagent_id: None,
            lines: err_lines,
        });
    }

    blks
}

// ── Widgets ─────────────────────────────────────────────────────────

pub struct SystemBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
}

impl<'a> SystemBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize) -> Self {
        Self { lines, skip: skip as u16 }
    }
}

impl<'a> Widget for SystemBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct UserBlock<'a> {
    text: &'a str,
    skip: u16,
}

impl<'a> UserBlock<'a> {
    pub fn new(text: &'a str, skip: usize) -> Self {
        Self { text, skip: skip as u16 }
    }
}

impl<'a> Widget for UserBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        buf.set_style(area, Style::default().bg(USER_BG).fg(Color::White));
        let lines: Vec<Line> = self.text.lines().map(|l| Line::raw(l.to_string())).collect();
        Paragraph::new(Text::from(lines))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct ThoughtBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
}

impl<'a> ThoughtBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize) -> Self {
        Self { lines, skip: skip as u16 }
    }
}

impl<'a> Widget for ThoughtBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct ResponseBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
}

impl<'a> ResponseBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize) -> Self {
        Self { lines, skip: skip as u16 }
    }
}

impl<'a> Widget for ResponseBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct ToolBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
    name: &'a str,
    args: &'a str,
    result: &'a Option<ToolResult>,
    frame_count: u64,
}

impl<'a> ToolBlock<'a> {
    pub fn new(
        lines: &'a [Line<'static>],
        name: &'a str,
        args: &'a str,
        result: &'a Option<ToolResult>,
        frame_count: u64,
        skip: usize,
    ) -> Self {
        Self {
            lines,
            skip: skip as u16,
            name,
            args,
            result,
            frame_count,
        }
    }
}

impl<'a> Widget for ToolBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let title_bg = match self.result {
            Some(r) if r.is_error => Color::Rgb(80, 40, 40),
            Some(_) => Color::Rgb(40, 70, 50),
            None => TOOL_COLOR,
        };
        let title_fg = match self.result {
            None => Color::Black,
            Some(r) if r.is_error => Color::Rgb(255, 150, 150),
            Some(_) => Color::Rgb(150, 255, 150),
        };

        let gear = GEAR_FRAMES[self.frame_count as usize % GEAR_FRAMES.len()];
        let icon = match self.result {
            None => gear,
            Some(r) if r.is_error => "✗",
            Some(_) => "✓",
        };

        let args_summary = tool_args_summary(self.args);

        // Determine how many rows of the title are still visible given scroll offset.
        let title_visible_h = if self.skip < TOOL_TITLE_HEIGHT as u16 {
            (TOOL_TITLE_HEIGHT as u16 - self.skip).min(area.height)
        } else {
            0
        };

        let chunks = Layout::vertical([
            Constraint::Length(title_visible_h),
            Constraint::Min(0),
        ])
        .split(area);

        let title_area = chunks[0];
        let content_area = chunks[1];

        // Title background
        buf.set_style(title_area, Style::default().bg(title_bg).fg(title_fg).add_modifier(Modifier::BOLD));
        // Content background
        buf.set_style(content_area, Style::default().bg(CODE_BG));

        // Render dynamic title independently
        let title_lines = vec![Line::raw(format!("  {} {}{}", icon, self.name, args_summary))];
        Paragraph::new(Text::from(title_lines))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(title_area, buf);

        // Render cached content directly without merging into a giant vector
        let content_skip = self.skip.saturating_sub(TOOL_TITLE_HEIGHT as u16);
        Paragraph::new(Text::from(self.lines.to_vec()))
            .scroll((content_skip, 0))
            .render(content_area, buf);
    }
}

static GEAR_FRAMES: &[&str] = &["◐", "◓", "◑", "◒"];

pub struct SubagentBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
    hovered: bool,
}

impl<'a> SubagentBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize, hovered: bool) -> Self {
        Self { lines, skip: skip as u16, hovered }
    }
}

impl<'a> Widget for SubagentBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bg = if self.hovered { HOVER_BG } else { CODE_BG };
        buf.set_style(area, Style::default().bg(bg));
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct NoticeBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
}

impl<'a> NoticeBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize) -> Self {
        Self { lines, skip: skip as u16 }
    }
}

impl<'a> Widget for NoticeBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct ErrorBlock<'a> {
    lines: &'a [Line<'static>],
    skip: u16,
}

impl<'a> ErrorBlock<'a> {
    pub fn new(lines: &'a [Line<'static>], skip: usize) -> Self {
        Self { lines, skip: skip as u16 }
    }
}

impl<'a> Widget for ErrorBlock<'a> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        Paragraph::new(Text::from(self.lines.to_vec()))
            .wrap(Wrap { trim: false })
            .scroll((self.skip, 0))
            .render(area, buf);
    }
}

pub struct WorkingBlock {
    frame_count: u64,
}

impl WorkingBlock {
    pub fn new(frame_count: u64) -> Self {
        Self { frame_count }
    }
}

impl Widget for WorkingBlock {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let spinner = SPINNER_FRAMES[self.frame_count as usize % SPINNER_FRAMES.len()];
        let line = Line::from(vec![
            Span::styled(format!("  {}  ", spinner), Style::default().fg(Color::Rgb(229, 192, 123)).add_modifier(Modifier::BOLD)),
            Span::styled(
                "Working...",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ),
        ]);
        Paragraph::new(line).render(area, buf);
    }
}

static SPINNER_FRAMES: &[&str] = &[
    "⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏",
];
