use super::state::{AppState, CommandMode, Entry, ModalState, SubagentState, ToolResult, TurnBlock};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Margin, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Scrollbar, ScrollbarOrientation,
        ScrollbarState, Wrap,
    },
};
use std::sync::LazyLock;
use std::time::{Duration, Instant};
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── Theme ───────────────────────────────────────────────────────────

const USER_BG: Color = Color::Rgb(28, 30, 42);
const CODE_BG: Color = Color::Rgb(22, 24, 29);
const HOVER_BG: Color = Color::Rgb(38, 42, 55);
const INLINE_CODE_BG: Color = Color::Rgb(35, 35, 50);
const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const SUBAGENT_COLOR: Color = Color::Rgb(198, 120, 221);
const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);
const ERROR_COLOR: Color = Color::Rgb(224, 108, 117);
const WARN_COLOR: Color = Color::Rgb(229, 192, 123);

/// Minimum time between cache rebuilds during streaming (milliseconds).
/// Prevents every single token from triggering a full markdown+syntect reparse.
const STREAMING_REBUILD_THROTTLE: Duration = Duration::from_millis(80);

// ── Syntect globals (loaded once) ────────────────────────────────────

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SYNTAX_THEME: LazyLock<syntect::highlighting::Theme> = LazyLock::new(|| {
    let ts = ThemeSet::load_defaults();
    ts.themes["base16-ocean.dark"].clone()
});

// ── Layout ──────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, state: &mut AppState) {
    state.frame_count = state.frame_count.wrapping_add(1);
    let area = frame.area();

    // Reserve space for autocomplete dropdown above input (if active)
    let dropdown_h = if state.autocomplete.active {
        (state.autocomplete.filtered_options.len().min(8) + 2) as u16
    } else {
        0
    };

    let main_bottom = area.height.saturating_sub(3 + dropdown_h);
    let input_top = area.height.saturating_sub(3);

    let status_area = Rect::new(area.x, area.y, area.width, 3);
    let main_area = Rect::new(area.x, area.y + 3, area.width, main_bottom.saturating_sub(3));
    let dropdown_area = if dropdown_h > 0 {
        Rect::new(area.x, input_top.saturating_sub(dropdown_h), area.width, dropdown_h)
    } else {
        Rect::default()
    };
    let input_area = Rect::new(area.x, input_top, area.width, 3);

    render_status(frame, state, status_area);

    if state.subagent_view.is_some() {
        render_subagent_detail(frame, state, main_area);
    } else {
        render_conversation(frame, state, main_area);
    }

    if dropdown_h > 0 {
        render_dropdown(frame, state, dropdown_area);
    }
    render_input(frame, state, input_area);
    render_modal(frame, state, area);
}

// ── Status bar ──────────────────────────────────────────────────────

fn render_status(frame: &mut Frame, state: &AppState, area: Rect) {
    let (state_text, state_style) = match state.agent_state.as_str() {
        "streaming" => {
            // "Working..." — waiting for first token from model
            (
                "working... [esc]",
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD | Modifier::SLOW_BLINK),
            )
        }
        "thinking" => {
            // Model is thinking
            (
                "thinking... [esc]",
                Style::default()
                    .fg(Color::Rgb(198, 120, 221))
                    .add_modifier(Modifier::BOLD),
            )
        }
        "responding" => {
            // Model is generating text
            (
                "responding... [esc]",
                Style::default()
                    .fg(Color::Rgb(86, 182, 194))
                    .add_modifier(Modifier::BOLD),
            )
        }
        "running tools" => {
            // Executing tools
            (
                "running tools... [esc]",
                Style::default()
                    .fg(Color::Rgb(229, 192, 123))
                    .add_modifier(Modifier::BOLD),
            )
        }
        "idle" => (
            "idle",
            Style::default().fg(SUCCESS_COLOR),
        ),
        other => (
            other,
            Style::default().fg(Color::White),
        ),
    };

    let mut status_spans = vec![
        Span::styled(
            " 🤖 Agent Core ",
            Style::default()
                .fg(Color::Rgb(97, 175, 239))
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("Model: {} ", state.model),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("Tokens: {} ", state.tokens),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" │ "),
        Span::styled(
            format!("State: {} ", state_text),
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

    let para = Paragraph::new(status_text).block(block);
    frame.render_widget(para, area);
}

// ── Input bar ───────────────────────────────────────────────────────

fn render_input(frame: &mut Frame, state: &AppState, area: Rect) {
    // ── Command-mode prompt ─────────────────────────────────────────
    if !matches!(state.command_mode, CommandMode::None) {
        let hint = state.command_mode.prompt();
        let prompt = Span::styled(
            format!(" {hint} "),
            Style::default()
                .fg(Color::Rgb(229, 192, 123))
                .add_modifier(Modifier::BOLD),
        );
        let text = Span::raw(&state.input);
        let cursor = if state.input.is_empty() {
            Span::styled(" ", Style::default().bg(Color::White))
        } else {
            Span::raw("")
        };

        let line = Line::from(vec![prompt, text, cursor]);
        let block = Block::default()
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(Color::Rgb(229, 192, 123)));

        let para = Paragraph::new(line).block(block);
        frame.render_widget(para, area);

        let hint_w = hint.len() + 3;
        let cursor_w = state.input[..state.cursor_pos.min(state.input.len())].width();
        let cursor_x = (hint_w + cursor_w) as u16;
        let max_x = area.width.saturating_sub(1);
        frame.set_cursor_position((area.x + cursor_x.min(max_x), area.y + 1));
        return;
    }

    // ── Normal prompt ──────────────────────────────────────────────
    let prompt = Span::styled(
        " ❯ ",
        Style::default()
            .fg(SUCCESS_COLOR)
            .add_modifier(Modifier::BOLD),
    );
    let text = Span::raw(&state.input);
    let cursor = if state.input.is_empty() {
        Span::styled(" ", Style::default().bg(Color::White))
    } else {
        Span::raw("")
    };

    let line = Line::from(vec![prompt, text, cursor]);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

    let para = Paragraph::new(line).block(block);
    frame.render_widget(para, area);

    let cursor_width = state.input[..state.cursor_pos.min(state.input.len())].width();
    let cursor_x = 4 + cursor_width.min(area.width.saturating_sub(6) as usize) as u16;
    frame.set_cursor_position((area.x + cursor_x, area.y + 1));
}

// ── Conversation (with split cache + throttle + window rendering) ─────

fn render_conversation(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let width = area.width;
    let visible_height = area.height as usize;

    // Store area info for mouse hover/click mapping
    state.main_area_y = area.y;
    state.main_area_height = area.height;

    // ── Streaming throttle ──────────────────────────────────────────
    // During streaming, only rebuild the cache at most every
    // STREAMING_REBUILD_THROTTLE ms. This prevents every single token
    // from triggering a full markdown+syntect reparse.
    let needs_rebuild = state.cache_dirty || state.cache.width != width;
    if needs_rebuild {
        let should_rebuild_now = if state.agent_running {
            // Throttle during streaming
            match state.cache.last_rebuild {
                Some(last) => Instant::now().duration_since(last) >= STREAMING_REBUILD_THROTTLE,
                None => true,
            }
        } else {
            // No throttle when idle — always rebuild immediately
            true
        };

        if should_rebuild_now {
            rebuild_cache(state, width);
        }
        // If throttled, cache_dirty stays true and we'll rebuild next time
    }

    let total = state.cache.wrapped_height;
    let max_scroll = total.saturating_sub(visible_height);
    state.scroll = state.scroll.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(state.scroll);

    // ── Window rendering ────────────────────────────────────────────
    // Instead of cloning ALL lines and using .scroll() (which makes
    // ratatui wrap ALL lines), we only render the visible window.
    // We use row_offsets to binary-search which logical lines are visible.
    let (window_start, visible_lines) = get_visible_lines(
        &state.cache.lines,
        &state.cache.row_offsets,
        scroll_from_top,
        visible_height,
    );

    // ── Apply hover highlight ──────────────────────────────────────
    let mut final_lines = visible_lines;
    if let Some(ref hovered_id) = state.hovered_subagent {
        for &(start, end, ref id) in &state.cache.subagent_line_ranges {
            if id == hovered_id {
                let vis_start = start.saturating_sub(window_start);
                let vis_end = end.saturating_sub(window_start).min(final_lines.len());
                for i in vis_start..vis_end {
                    if i < final_lines.len() {
                        let mut line = std::mem::take(&mut final_lines[i]);
                        for span in line.spans.iter_mut() {
                            span.style.bg = Some(HOVER_BG);
                        }
                        final_lines[i] = line;
                    }
                }
            }
        }
    }

    let para = Paragraph::new(Text::from(final_lines))
        .wrap(Wrap { trim: false });
    frame.render_widget(para, area);

    // ── Scrollbar ───────────────────────────────────────────────────
    if max_scroll > 0 {
        let scrollbar_area = area.inner(Margin {
            vertical: 0,
            horizontal: 0,
        });
        let mut scrollbar_state = ScrollbarState::new(max_scroll)
            .position(scroll_from_top);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .track_symbol(Some("│"))
                .end_symbol(Some("▼"))
                .thumb_style(Style::default().fg(Color::Rgb(92, 99, 112)))
                .track_style(Style::default().fg(Color::Rgb(40, 44, 52))),
            scrollbar_area,
            &mut scrollbar_state,
        );
    }
}

/// Given the full line list, row_offsets, scroll position, and visible height,
/// return only the lines that are visible on screen along with the start index
/// in the cache. This avoids cloning the entire line array and avoids ratatui
/// processing off-screen lines.
fn get_visible_lines(
    lines: &[Line<'static>],
    row_offsets: &[usize],
    scroll_from_top: usize,
    visible_height: usize,
) -> (usize, Vec<Line<'static>>) {
    if lines.is_empty() || row_offsets.is_empty() {
        return (0, Vec::new());
    }

    // Find the first logical line that contains the scroll row
    // row_offsets[i] = total wrapped rows for lines[0..i]
    let start_line_idx = row_offsets.partition_point(|&r| r <= scroll_from_top)
        .saturating_sub(1) // include the line that starts before scroll_from_top
        .min(lines.len() - 1);

    // Find the last visible logical line
    let end_row = scroll_from_top + visible_height;
    let end_line_idx = row_offsets.partition_point(|&r| r < end_row)
        .min(lines.len());

    // Add a small buffer for safety (wrapping might need context)
    let start = start_line_idx.saturating_sub(1);
    let end = (end_line_idx + 1).min(lines.len());

    let window = &lines[start..end];

    // Clone only the visible window — much cheaper than cloning all lines.
    // The key win: we're NOT processing all 1000+ lines — just ~50 visible ones.
    (start, window.to_vec())
}

/// Rebuild the cached lines from entries + streaming data.
///
/// Key optimization: Split into `entry_lines` (completed entries, cached
/// across streaming updates) and `streaming_lines` (rebuilt every time
/// but usually small). Only re-render entries when their count changes.
fn rebuild_cache(state: &mut AppState, width: u16) {
    let w = width as usize;
    let entry_count = state.entries.len();

    // ── Track subagent line ranges ────────────────────────────────
    let mut entry_sa_ranges: Vec<(usize, usize, String)> = Vec::new();
    let mut streaming_sa_ranges: Vec<(usize, usize, String)> = Vec::new();

    // ── Only rebuild entry_lines if entries changed ────────────────
    if state.cache.rendered_entry_count != entry_count || state.cache.width != width {
        let mut entry_lines: Vec<Line<'static>> = Vec::new();
        let mut first = true;
        for entry in &state.entries {
            if !first {
                entry_lines.push(Line::raw(""));
            }
            first = false;
            render_entry_cloned(entry, &mut entry_lines, w, &mut entry_sa_ranges);
        }
        state.cache.entry_lines = entry_lines;
        state.cache.rendered_entry_count = entry_count;
        // Cache entry subagent ranges for reuse
        state.cache.entry_subagent_ranges = entry_sa_ranges.clone();
    } else {
        // Reuse cached entry subagent ranges
        entry_sa_ranges = state.cache.entry_subagent_ranges.clone();
    }

    // ── Always rebuild streaming lines ─────────────────────────────
    let mut streaming_lines: Vec<Line<'static>> = Vec::new();
    if let Some(ref streaming) = state.streaming {
        if !streaming.blocks.is_empty() {
            for (i, block) in streaming.blocks.iter().enumerate() {
                if i > 0 {
                    let is_approval_notice = matches!(block, TurnBlock::Notice(msg) if msg.contains("[APPROVAL]"));
                    if !is_approval_notice {
                        streaming_lines.push(Line::raw(""));
                    }
                }
                render_turn_block_cloned(block, &mut streaming_lines, w, 0, &mut streaming_sa_ranges);
            }
        }
    }
    state.cache.streaming_lines = streaming_lines;

    // ── Combine into final lines ──────────────────────────────────
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Entry lines (cached)
    lines.extend(state.cache.entry_lines.iter().cloned());

    // Streaming lines
    let streaming_offset = lines.len();
    let _separator_count = if !state.cache.streaming_lines.is_empty() && !lines.is_empty() {
        lines.push(Line::raw(""));
        1
    } else {
        0
    };
    lines.extend(state.cache.streaming_lines.iter().cloned());

    // "Working..." indicator — shown between submit and first token.
    // Once we get a Thought or Response block, the state will be
    // set to "thinking" or "streaming" by handle_agent_event,
    // and this indicator disappears because streaming.blocks is no longer empty.
    let streaming_empty = state.streaming.as_ref().map_or(true, |s| s.blocks.is_empty());
    if state.agent_running && streaming_empty {
        if !lines.is_empty() {
            lines.push(Line::raw(""));
        }
        let load_style = Style::default()
            .fg(Color::Rgb(229, 192, 123))
            .add_modifier(Modifier::BOLD);
        lines.push(Line::from(vec![
            Span::styled("  ⏳  ", load_style),
            Span::styled(
                "Working...",
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC),
            ),
        ]));
    }
    lines.push(Line::raw(""));

    // ── Compute wrapped height and row offsets ─────────────────────
    let row_offsets = compute_row_offsets(&lines, width);
    let wrapped_height = row_offsets.last().copied().unwrap_or(0);

    state.cache.lines = lines;
    state.cache.width = width;
    state.cache.wrapped_height = wrapped_height;
    state.cache.row_offsets = row_offsets;
    state.cache.version = state.content_version;
    state.cache.last_rebuild = Some(Instant::now());
    state.cache_dirty = false;

    // ── Combine subagent ranges ────────────────────────────────────
    // Entry ranges are already relative to the start of lines.
    // Streaming ranges need to be offset by streaming_offset.
    let mut combined_ranges = entry_sa_ranges;
    for (start, end, id) in streaming_sa_ranges {
        combined_ranges.push((start + streaming_offset, end + streaming_offset, id));
    }
    state.cache.subagent_line_ranges = combined_ranges;
}

/// Compute cumulative wrapped row count for each line.
/// row_offsets[i] = total wrapped rows for lines[0..i].
/// This enables binary-search-based window rendering.
fn compute_row_offsets(lines: &[Line<'_>], width: u16) -> Vec<usize> {
    let width = usize::from(width).max(1);
    let mut offsets = Vec::with_capacity(lines.len() + 1);
    offsets.push(0);
    let mut cumulative = 0usize;
    for line in lines {
        let lw = line.width();
        let rows = if lw == 0 { 1 } else { lw.div_ceil(width) };
        cumulative += rows;
        offsets.push(cumulative);
    }
    offsets
}

/// Same as render_entry but outputs Line<'static> by cloning string data.
fn render_entry_cloned(
    entry: &Entry,
    lines: &mut Vec<Line<'static>>,
    width: usize,
    sa_ranges: &mut Vec<(usize, usize, String)>,
) {
    match entry {
        Entry::System { text } => {
            let style = Style::default().fg(Color::Cyan);
            for line in text.lines() {
                lines.push(Line::from(vec![Span::styled(line.to_string(), style)]));
            }
        }
        Entry::User { text } => {
            render_user_block(text, lines, width);
        }
        Entry::Turn { blocks, .. } => {
            for (i, block) in blocks.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                render_turn_block_cloned(block, lines, width, 0, sa_ranges);
            }
        }
    }
}

/// Same as render_turn_block but outputs Line<'static> by cloning string data.
/// Also tracks subagent block line ranges for hover detection.
fn render_turn_block_cloned(
    block: &TurnBlock,
    lines: &mut Vec<Line<'static>>,
    width: usize,
    indent: usize,
    sa_ranges: &mut Vec<(usize, usize, String)>,
) {
    let pad = " ".repeat(indent * 2);
    let inner_width = width.saturating_sub(pad.width());

    match block {
        TurnBlock::Thought(text) => {
            let style = Style::default()
                .fg(Color::DarkGray)
                .add_modifier(Modifier::ITALIC);
            let mut first = true;
            for line in text.lines() {
                let line = line.trim_start();
                if line.is_empty() {
                    lines.push(Line::raw(""));
                    continue;
                }
                if first {
                    lines.push(Line::from(vec![
                        Span::raw(pad.clone()),
                        Span::styled("💭 Thought: ".to_string(), style.add_modifier(Modifier::BOLD)),
                        Span::styled(line.to_string(), style),
                    ]));
                    first = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(pad.clone()),
                        Span::styled(line.to_string(), style),
                    ]));
                }
            }
        }
        TurnBlock::Response(text) => {
            let md_lines = markdown_to_lines(text, inner_width);
            for mut line in md_lines {
                if !pad.is_empty() {
                    line.spans.insert(0, Span::raw(pad.clone()));
                }
                lines.push(line);
            }
        }
        TurnBlock::Tool { name, args, result } => {
            render_tool_block(name, args, result, lines, inner_width, &pad);
        }
        TurnBlock::Subagent(sa) => {
            let range_start = lines.len();
            render_subagent_block(sa, lines, inner_width, &pad);
            let range_end = lines.len();
            sa_ranges.push((range_start, range_end, sa.id.clone()));
        }
        TurnBlock::Error(e) => {
            lines.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(
                    format!("✗  {e}"),
                    Style::default()
                        .fg(ERROR_COLOR)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        TurnBlock::Notice(msg) => {
            let (icon, style) = if msg.contains("Failed") || msg.contains("Error") || msg.contains("nknown command") {
                ("✗", Style::default().fg(ERROR_COLOR).add_modifier(Modifier::BOLD))
            } else if msg.contains("registered") || msg.contains("Switched") || msg.contains("cleared") {
                ("✓", Style::default().fg(SUCCESS_COLOR).add_modifier(Modifier::BOLD))
            } else if msg.contains("Available") || msg.contains("help") || msg.contains("Registered") {
                ("ℹ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            } else {
                ("⚠", Style::default().fg(WARN_COLOR).add_modifier(Modifier::BOLD))
            };
            for (i, line_text) in msg.lines().enumerate() {
                if i > 0 {
                    lines.push(Line::from(vec![Span::raw(pad.clone())]));
                }
                lines.push(Line::from(vec![
                    Span::raw(pad.clone()),
                    Span::styled(
                        format!("{icon}  {line_text}"),
                        style,
                    ),
                ]));
            }
        }
    }
}

// ── User block ──────────────────────────────────────────────────────

fn render_user_block(text: &str, lines: &mut Vec<Line<'static>>, width: usize) {
    let inner_width = width.saturating_sub(4);
    let bg_style = Style::default().bg(USER_BG);

    // Top padding
    lines.push(Line::from(Span::styled(" ".repeat(width), bg_style)));

    for line in text.lines() {
        let line_str = line.to_string();
        let lw = line_str.width();
        let fill_len = inner_width.saturating_sub(lw);
        let fill = " ".repeat(fill_len);

        lines.push(Line::from(vec![
            Span::styled("  ", bg_style),
            Span::styled(line_str, Style::default().fg(Color::White).bg(USER_BG)),
            Span::styled(fill, bg_style),
            Span::styled("  ", bg_style),
        ]));
    }

    lines.push(Line::from(Span::styled(" ".repeat(width), bg_style)));
}

// ── Tool block ──────────────────────────────────────────────────────

fn render_tool_block(
    name: &str,
    args: &str,
    result: &Option<ToolResult>,
    lines: &mut Vec<Line<'static>>,
    width: usize,
    pad: &str,
) {
    // Account for pad + "  " left + "  " right = pad.width() + 4
    let inner_width = width.saturating_sub(4 + pad.width());
    let bg = Style::default().bg(CODE_BG);
    let label_style = Style::default()
        .fg(TOOL_COLOR)
        .bg(CODE_BG)
        .add_modifier(Modifier::BOLD);

    // Top padding — fill full width so no terminal default background shows
    let top_fill = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(top_fill), bg),
    ]));

    // Label line
    let label = format!("⚙  {}  ", name);
    let label_fill = " ".repeat(inner_width.saturating_sub(label.width()));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(label, label_style),
        Span::styled(label_fill, bg),
        Span::styled("  ", bg),
    ]));

    // Separator
    let sep = "─".repeat(inner_width.min(40));
    let sep_fill = " ".repeat(inner_width.saturating_sub(sep.width()));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(
            sep,
            Style::default().fg(Color::Rgb(92, 99, 112)).bg(CODE_BG),
        ),
        Span::styled(sep_fill, bg),
        Span::styled("  ", bg),
    ]));

    // Args
    if !args.trim().is_empty() {
        for line in args.lines() {
            let arg_str = truncate_str_w(line, inner_width);
            let fill = " ".repeat(inner_width.saturating_sub(arg_str.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("  ", bg),
                Span::styled(arg_str, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
                Span::styled(fill, bg),
                Span::styled("  ", bg),
            ]));
        }
    }

    // Output
    if let Some(r) = result {
        let (prefix, style) = if r.is_error {
            ("✗ ", Style::default().fg(ERROR_COLOR))
        } else {
            ("→ ", Style::default().fg(SUCCESS_COLOR))
        };
        let out_style = style.bg(CODE_BG);

        let mut shown = 0;
        for line in r.text.lines().take(8) {
            let prefix_w = prefix.width();
            let trunc_line = truncate_str_w(line, inner_width.saturating_sub(prefix_w));
            let fill = " ".repeat(inner_width.saturating_sub(prefix_w + trunc_line.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("  ", bg),
                Span::styled(prefix.to_string(), out_style.add_modifier(Modifier::BOLD)),
                Span::styled(trunc_line, out_style),
                Span::styled(fill, bg),
                Span::styled("  ", bg),
            ]));
            shown += 1;
        }

        let total = r.text.lines().count();
        if total > shown {
            let msg = format!("… {} more lines", total - shown);
            let fill = " ".repeat(inner_width.saturating_sub(msg.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("  ", bg),
                Span::styled(msg, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
                Span::styled(fill, bg),
                Span::styled("  ", bg),
            ]));
        }
    } else {
        let msg = "Waiting for result…";
        let fill = " ".repeat(inner_width.saturating_sub(msg.width()));
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("  ", bg),
            Span::styled(msg.to_string(), Style::default().fg(Color::DarkGray).bg(CODE_BG)),
            Span::styled(fill, bg),
            Span::styled("  ", bg),
        ]));
    }

    // Bottom padding — fill full width
    let bot_fill = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(bot_fill), bg),
    ]));
}

// ── Subagent block (summary in main conversation) ───────────────────
// In the main conversation view, subagents are shown as compact summary
// boxes. The user can press Enter to drill into the detail view.
// All children are hidden in the main view — they're shown in the
// subagent detail view instead.

fn render_subagent_block(
    sa: &SubagentState,
    lines: &mut Vec<Line<'static>>,
    width: usize,
    pad: &str,
) {
    let inner_width = width.saturating_sub(4 + pad.width());
    let bg = Style::default().bg(CODE_BG);
    let label_style = Style::default()
        .fg(SUBAGENT_COLOR)
        .bg(CODE_BG)
        .add_modifier(Modifier::BOLD);
    let border_fg = Color::Rgb(92, 99, 112);

    // ── Top padding ──
    let top_fill = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(top_fill), bg),
    ]));

    // ── Top separator ──
    let top_sep = "─".repeat(inner_width.min(40));
    let top_fill2 = inner_width.saturating_sub(top_sep.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(top_sep, Style::default().fg(border_fg).bg(CODE_BG)),
        Span::styled(" ".repeat(top_fill2), bg),
        Span::styled("  ", bg),
    ]));

    // ── Label + status badge (single line) ──
    let label_text = format!("⚡ {}", sa.id);
    let badge = if sa.done {
        if sa.success {
            ("✓", SUCCESS_COLOR)
        } else {
            ("✗", ERROR_COLOR)
        }
    } else if sa.turn_index > 0 {
        ("⏳", Color::Rgb(229, 192, 123))
    } else {
        ("⏳", Color::DarkGray)
    };
    let badge_text = if sa.done {
        if sa.success {
            format!("{} done ({}i)", badge.0, sa.iterations)
        } else {
            format!("{} fail ({}i)", badge.0, sa.iterations)
        }
    } else if sa.turn_index > 0 {
        format!("{} turn {}", badge.0, sa.turn_index + 1)
    } else {
        format!("{} starting", badge.0)
    };
    let badge_width_est = badge_text.width();
    let gap = inner_width.saturating_sub(label_text.width()).saturating_sub(badge_width_est);
    let label_line = format!("{}{}{}", label_text, " ".repeat(gap), badge_text);
    let label_fill = " ".repeat(inner_width.saturating_sub(label_line.width()));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(label_text, label_style),
        Span::styled(" ".repeat(gap), bg),
        Span::styled(badge_text, Style::default().fg(badge.1).bg(CODE_BG).add_modifier(Modifier::BOLD)),
        Span::styled(label_fill, bg),
        Span::styled("  ", bg),
    ]));

    // ── Task line ──
    let task_label = "Task: ";
    let task_trunc = truncate_str_w(&sa.task, inner_width.saturating_sub(task_label.width()));
    let task_fill = " ".repeat(inner_width.saturating_sub(task_label.width() + task_trunc.width()));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(task_label, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
        Span::styled(task_trunc, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
        Span::styled(task_fill, bg),
        Span::styled("  ", bg),
    ]));

    // ── Activity lines (live, dynamically updated) ──
    const MAX_ACTIVITY_LINES: usize = 5;
    let log = &sa.activity_log;
    let show_from = log.len().saturating_sub(MAX_ACTIVITY_LINES);
    let visible = &log[show_from..];
    let is_last = |idx: usize| idx + show_from + 1 == log.len();
    for (i, entry) in visible.iter().enumerate() {
        let act_str = truncate_str_w(entry, inner_width);
        let base_color = if sa.done {
            if sa.success { SUCCESS_COLOR } else { ERROR_COLOR }
        } else if entry.starts_with("🔧") {
            TOOL_COLOR
        } else if entry.starts_with("💭") {
            Color::Rgb(180, 180, 200)
        } else if entry.starts_with("📝") {
            Color::Rgb(160, 200, 160)
        } else {
            Color::Rgb(229, 192, 123)
        };
        let fg = if !is_last(i) && !sa.done {
            Color::DarkGray
        } else {
            base_color
        };
        let act_fill = " ".repeat(inner_width.saturating_sub(act_str.width()));
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("  ", bg),
            Span::styled(act_str, Style::default().fg(fg).bg(CODE_BG)),
            Span::styled(act_fill, bg),
            Span::styled("  ", bg),
        ]));
    }
    for _ in visible.len()..MAX_ACTIVITY_LINES {
        let empty_fill = " ".repeat(inner_width);
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("  ", bg),
            Span::styled(empty_fill, bg),
            Span::styled("  ", bg),
        ]));
    }

    // ── [Click] details ──
    let hint = "[Click] details";
    let hint_fill = " ".repeat(inner_width.saturating_sub(hint.width()));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(hint, Style::default().fg(SUBAGENT_COLOR).bg(CODE_BG)),
        Span::styled(hint_fill, bg),
        Span::styled("  ", bg),
    ]));

    // ── Bottom separator ──
    let bot_sep = "─".repeat(inner_width.min(40));
    let bot_fill = inner_width.saturating_sub(bot_sep.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(bot_sep, Style::default().fg(border_fg).bg(CODE_BG)),
        Span::styled(" ".repeat(bot_fill), bg),
        Span::styled("  ", bg),
    ]));

    // ── Bottom padding ──
    let bot_fill2 = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(bot_fill2), bg),
    ]));
}

// ── Subagent detail view ────────────────────────────────────────────
// When the user presses Enter on a subagent box, we switch to a detail
// view showing the subagent's full conversation (thoughts, responses,
// tool calls). This is like the main view but scoped to one subagent.

fn render_subagent_detail(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let width = area.width;
    let visible_height = area.height as usize;

    let subagent_id = match &state.subagent_view {
        Some(id) => id.clone(),
        None => return,
    };

    // Find the subagent and render its children as the conversation
    let mut lines: Vec<Line<'static>> = Vec::new();

    // Header breadcrumb
    let header_style = Style::default()
        .fg(SUBAGENT_COLOR)
        .add_modifier(Modifier::BOLD);
    let back_hint = Style::default().fg(Color::DarkGray);
    lines.push(Line::from(vec![
        Span::styled(" ← ", Style::default().fg(SUCCESS_COLOR).add_modifier(Modifier::BOLD)),
        Span::styled(format!("subagent: {}", subagent_id), header_style),
        Span::styled("  [Esc] back", back_hint),
    ]));
    lines.push(Line::from(Span::styled(
        "─".repeat(width as usize),
        Style::default().fg(Color::Rgb(60, 63, 70)),
    )));

    // Render the subagent's children like a normal conversation
    if let Some(sa) = state.find_subagent(&subagent_id) {
        for (i, child) in sa.children.iter().enumerate() {
            if i > 0 {
                lines.push(Line::raw(""));
            }
            render_turn_block_cloned(child, &mut lines, width as usize, 0, &mut Vec::new());
        }

        // Status at the bottom
        if sa.done {
            let (icon, text, color) = if sa.success {
                ("✓", format!("Completed ({} iterations)", sa.iterations), SUCCESS_COLOR)
            } else {
                ("✗", format!("Incomplete ({} iterations)", sa.iterations), ERROR_COLOR)
            };
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(format!(" {} ", icon), Style::default().fg(color).add_modifier(Modifier::BOLD)),
                Span::styled(text, Style::default().fg(color)),
            ]));
        } else {
            lines.push(Line::raw(""));
            lines.push(Line::from(vec![
                Span::styled(" ⏳ ", Style::default().fg(WARN_COLOR).add_modifier(Modifier::BOLD)),
                Span::styled("Running...", Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)),
            ]));
        }
    } else {
        lines.push(Line::from(Span::styled(
            format!("Subagent '{}' not found", subagent_id),
            Style::default().fg(ERROR_COLOR),
        )));
    }

    lines.push(Line::raw(""));

    // Compute scrolling
    let row_offsets = compute_row_offsets(&lines, width);
    let total = row_offsets.last().copied().unwrap_or(0);
    let max_scroll = total.saturating_sub(visible_height);
    state.subagent_scroll = state.subagent_scroll.min(max_scroll);
    let scroll_from_top = max_scroll.saturating_sub(state.subagent_scroll);

    let (_, visible_lines) = get_visible_lines(&lines, &row_offsets, scroll_from_top, visible_height);

    let para = Paragraph::new(Text::from(visible_lines)).wrap(Wrap { trim: false });
    frame.render_widget(para, area);

    // Scrollbar
    if max_scroll > 0 {
        let scrollbar_area = area.inner(Margin {
            vertical: 0,
            horizontal: 0,
        });
        let mut scrollbar_state = ScrollbarState::new(max_scroll).position(scroll_from_top);
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(Some("▲"))
                .track_symbol(Some("│"))
                .end_symbol(Some("▼"))
                .thumb_style(Style::default().fg(Color::Rgb(92, 99, 112)))
                .track_style(Style::default().fg(Color::Rgb(40, 44, 52))),
            scrollbar_area,
            &mut scrollbar_state,
        );
    }
}

// ── Markdown → Ratatui (pulldown-cmark) ──────────────────────────────

fn markdown_to_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let parser = Parser::new_ext(text, Options::all());
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut cur_spans: Vec<Span<'static>> = Vec::new();
    let mut style_stack: Vec<Style> = vec![Style::default().fg(Color::White)];

    let mut heading_level: Option<HeadingLevel> = None;
    let mut in_blockquote = false;
    let mut code_lang = String::new();
    let mut code_content = String::new();
    let mut in_code_block = false;
    let mut ordered_num: u64 = 0;
    let mut is_ordered = false;

    for event in parser {
        match event {
            Event::Start(tag) => match tag {
                Tag::Paragraph => {}
                Tag::Heading { level, .. } => heading_level = Some(level),
                Tag::BlockQuote(_) => in_blockquote = true,
                Tag::CodeBlock(kind) => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    in_code_block = true;
                    code_lang = match kind {
                        CodeBlockKind::Fenced(lang) => lang.to_string(),
                        _ => String::new(),
                    };
                }
                Tag::List(num) => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    is_ordered = num.is_some();
                    if let Some(n) = num {
                        ordered_num = n;
                    }
                }
                Tag::Item => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    let prefix = if is_ordered {
                        ordered_num += 1;
                        Span::styled(
                            format!(" {}. ", ordered_num - 1),
                            Style::default().fg(TOOL_COLOR),
                        )
                    } else {
                        Span::styled("  • ", Style::default().fg(TOOL_COLOR))
                    };
                    cur_spans.push(prefix);
                }
                Tag::Emphasis => push_md_style(
                    &mut style_stack,
                    Style::default().add_modifier(Modifier::ITALIC),
                ),
                Tag::Strong => push_md_style(
                    &mut style_stack,
                    Style::default().add_modifier(Modifier::BOLD),
                ),
                Tag::Strikethrough => push_md_style(
                    &mut style_stack,
                    Style::default().add_modifier(Modifier::CROSSED_OUT),
                ),
                _ => {}
            },
            Event::End(tag) => match tag {
                TagEnd::Paragraph => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    lines.push(Line::raw(""));
                }
                TagEnd::Heading(_) => {
                    heading_level = None;
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    lines.push(Line::raw(""));
                }
                TagEnd::BlockQuote(_) => {
                    flush_md_line(&mut cur_spans, &mut lines, true);
                    in_blockquote = false;
                }
                TagEnd::CodeBlock => {
                    in_code_block = false;
                    lines.extend(render_syntect_block(&code_lang, &code_content, width));
                    code_content.clear();
                    code_lang.clear();
                    lines.push(Line::raw(""));
                }
                TagEnd::List(_) => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    lines.push(Line::raw(""));
                    is_ordered = false;
                    ordered_num = 0;
                }
                TagEnd::Item => flush_md_line(&mut cur_spans, &mut lines, false),
                TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough => {
                    style_stack.pop();
                }
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    code_content.push_str(&text);
                } else {
                    let style = *style_stack.last().unwrap();
                    let mut final_style = style;
                    if let Some(level) = heading_level {
                        final_style = final_style
                            .add_modifier(Modifier::BOLD)
                            .fg(heading_color(level));
                    }
                    if in_blockquote {
                        final_style = final_style.add_modifier(Modifier::ITALIC);
                    }
                    cur_spans.push(Span::styled(text.to_string(), final_style));
                }
            }
            Event::Code(text) => {
                if !in_code_block {
                    cur_spans.push(Span::styled(
                        text.to_string(),
                        Style::default().fg(Color::Yellow).bg(INLINE_CODE_BG),
                    ));
                }
            }
            Event::SoftBreak => {
                if !in_code_block {
                    cur_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_code_block {
                    flush_md_line(&mut cur_spans, &mut lines, in_blockquote);
                }
            }
            Event::Rule => {
                flush_md_line(&mut cur_spans, &mut lines, false);
                lines.push(Line::from(Span::styled(
                    "─".repeat(40),
                    Style::default().fg(Color::DarkGray),
                )));
            }
            _ => {}
        }
    }
    flush_md_line(&mut cur_spans, &mut lines, false);
    lines
}

/// Push a style onto the stack, merging with the current top.
fn push_md_style(stack: &mut Vec<Style>, addition: Style) {
    let base = *stack.last().unwrap();
    stack.push(merge_style(base, addition));
}

/// Flush accumulated inline spans into a Line. Optionally add a blockquote prefix.
fn flush_md_line(spans: &mut Vec<Span<'static>>, lines: &mut Vec<Line<'static>>, blockquote: bool) {
    if spans.is_empty() {
        return;
    }
    if blockquote {
        let mut final_spans = vec![Span::styled("│ ", Style::default().fg(Color::DarkGray))];
        final_spans.append(spans);
        lines.push(Line::from(final_spans));
    } else {
        lines.push(Line::from(std::mem::take(spans)));
    }
}

fn merge_style(base: Style, addition: Style) -> Style {
    Style {
        fg: addition.fg.or(base.fg),
        bg: addition.bg.or(base.bg),
        add_modifier: base.add_modifier | addition.add_modifier,
        sub_modifier: base.sub_modifier | addition.sub_modifier,
        underline_color: addition.underline_color.or(base.underline_color),
    }
}

fn heading_color(level: pulldown_cmark::HeadingLevel) -> Color {
    use pulldown_cmark::HeadingLevel;
    match level {
        HeadingLevel::H1 => Color::Rgb(97, 175, 239),   // blue
        HeadingLevel::H2 => Color::Rgb(86, 182, 194),   // cyan
        HeadingLevel::H3 => Color::Rgb(152, 195, 121),  // green
        HeadingLevel::H4 => Color::Rgb(229, 192, 123),  // yellow
        HeadingLevel::H5 => Color::Rgb(198, 120, 221),  // magenta
        HeadingLevel::H6 => Color::Rgb(171, 178, 191),  // gray
    }
}

// ── Code blocks (syntect) ──────────────────────────────────────────

fn render_syntect_block(language: &str, code: &str, width: usize) -> Vec<Line<'static>> {
    let syntax = find_syntax(language);
    let theme = &SYNTAX_THEME;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let fill_width = width.max(40);

    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let line = line.trim_end_matches(['\n', '\r']);
        if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
            // Collect highlighted spans
            let mut spans: Vec<Span> = vec![
                Span::styled("  ", Style::default().bg(CODE_BG)),
            ];
            for (style, text) in ranges {
                let fg = syntect_color_to_ratatui(style.foreground);
                spans.push(Span::styled(text.to_string(), Style::default().fg(fg).bg(CODE_BG)));
            }
            let line_w: usize = spans.iter().map(|s| s.width()).sum();
            let fill = fill_width.saturating_sub(line_w);
            if fill > 0 {
                spans.push(Span::styled(" ".repeat(fill), Style::default().bg(CODE_BG)));
            }
            lines.push(Line::from(spans));
        }
    }

    // Label bar — like tool block, with CODE_BG full width, no border
    let title = format!(" {}", if language.is_empty() { "code" } else { language });
    let title_line = {
        let title_fill = fill_width.saturating_sub(title.width() + 4);
        vec![
            Span::styled("  📋", Style::default().fg(Color::Yellow).bg(CODE_BG).add_modifier(Modifier::BOLD)),
            Span::styled(title, Style::default().fg(Color::Yellow).bg(CODE_BG).add_modifier(Modifier::BOLD)),
            Span::styled(" ".repeat(title_fill), Style::default().bg(CODE_BG)),
        ]
    };

    let sep_line = {
        let sep = "─".repeat(30.min(fill_width));
        let sep_fill = fill_width.saturating_sub(sep.width() + 4);
        vec![
            Span::styled("  ", Style::default().bg(CODE_BG)),
            Span::styled(sep, Style::default().fg(Color::Rgb(92, 99, 112)).bg(CODE_BG)),
            Span::styled(" ".repeat(sep_fill), Style::default().bg(CODE_BG)),
        ]
    };

    let top_line = Span::styled(" ".repeat(fill_width), Style::default().bg(CODE_BG));
    let bot_line = Span::styled(" ".repeat(fill_width), Style::default().bg(CODE_BG));

    let mut result = vec![Line::from(top_line)];
    result.push(Line::from(title_line));
    result.push(Line::from(sep_line));
    result.append(&mut lines);
    result.push(Line::from(bot_line));
    result
}

fn find_syntax(language: &str) -> &'static syntect::parsing::SyntaxReference {
    let lang = language.trim().to_ascii_lowercase();
    // Map common names to syntect names
    let token = match lang.as_str() {
        "" => "Plain Text",
        "rs" | "rust" => "Rust",
        "py" | "python" => "Python",
        "js" | "javascript" => "JavaScript",
        "ts" | "typescript" => "TypeScript",
        "json" => "JSON",
        "toml" => "TOML",
        "yaml" | "yml" => "YAML",
        "html" => "HTML",
        "css" => "CSS",
        "sql" => "SQL",
        "sh" | "bash" | "shell" => "Bash",
        "c" => "C",
        "cpp" | "c++" | "cc" => "C++",
        "java" => "Java",
        "go" | "golang" => "Go",
        "rb" | "ruby" => "Ruby",
        "php" => "PHP",
        "swift" => "Swift",
        "kt" | "kotlin" => "Kotlin",
        "scala" => "Scala",
        "r" => "R",
        "lua" => "Lua",
        "pl" | "perl" => "Perl",
        "ex" | "exs" | "elixir" => "Elixir",
        "erl" | "erlang" => "Erlang",
        "hs" | "haskell" => "Haskell",
        "clj" | "clojure" => "Clojure",
        "dart" => "Dart",
        "proto" | "protobuf" => "Protocol Buffer",
        "xml" => "XML",
        "md" | "markdown" => "Markdown",
        "make" | "makefile" => "Makefile",
        "cmake" => "CMake",
        "dockerfile" | "docker" => "Dockerfile",
        "diff" | "patch" => "Diff",
        "git" => "Git Commit",
        "ini" | "cfg" | "conf" => "INI",
        _ => &language,
    };
    SYNTAX_SET
        .find_syntax_by_token(token)
        .unwrap_or_else(|| SYNTAX_SET.find_syntax_plain_text())
}

fn syntect_color_to_ratatui(color: syntect::highlighting::Color) -> Color {
    Color::Rgb(color.r, color.g, color.b)
}

// ── Utility ──────────────────────────────────────────────────────────

fn truncate_str_w(s: &str, max_width: usize) -> String {
    if s.width() <= max_width {
        return s.to_string();
    }
    let mut result = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 3 > max_width {
            result.push_str("...");
            break;
        }
        w += cw;
        result.push(c);
    }
    result
}

// ── Autocomplete dropdown ───────────────────────────────────────────

fn render_dropdown(frame: &mut Frame, state: &AppState, area: Rect) {
    let options = &state.autocomplete.filtered_options;
    let sel = state.autocomplete.selected_index;
    let max_h = (area.height as usize).saturating_sub(2); // border takes 2
    let max_w = (area.width as usize).saturating_sub(4);

    let mut lines: Vec<Line> = Vec::new();
    for (i, opt) in options.iter().enumerate().take(max_h) {
        let truncated: String = opt.chars().take(max_w).collect();
        let fill = " ".repeat(max_w.saturating_sub(truncated.width()));

        if i == sel {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().bg(TOOL_COLOR)),
                Span::styled(truncated, Style::default().fg(Color::Black).bg(TOOL_COLOR).add_modifier(Modifier::BOLD)),
                Span::styled(fill, Style::default().bg(TOOL_COLOR)),
                Span::styled("  ", Style::default().bg(TOOL_COLOR)),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled("  ", Style::default().bg(CODE_BG)),
                Span::styled(truncated, Style::default().fg(Color::White).bg(CODE_BG)),
                Span::styled(fill, Style::default().bg(CODE_BG)),
                Span::styled("  ", Style::default().bg(CODE_BG)),
            ]));
        }
    }

    // Fill remaining lines
    for _ in lines.len()..max_h {
        lines.push(Line::from(Span::styled(" ".repeat(max_w + 4), Style::default().bg(CODE_BG))));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

    let para = Paragraph::new(Text::from(lines)).block(block);
    frame.render_widget(para, area);
}

// ── Modal overlay ───────────────────────────────────────────────────

fn render_modal(frame: &mut Frame, state: &AppState, screen: Rect) {
    match &state.modal {
        ModalState::ModelPicker { models, selected } => {
            let modal_w = 50u16;
            let modal_h = (models.len() + 4).min(16) as u16;
            let x = screen.x + (screen.width.saturating_sub(modal_w)) / 2;
            let y = screen.y + (screen.height.saturating_sub(modal_h)) / 2;
            let area = Rect::new(x, y, modal_w, modal_h);
            frame.render_widget(Clear, area);

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
                    lines.push(Line::from(Span::styled(line_str, Style::default().fg(Color::Black).bg(TOOL_COLOR))));
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

            frame.render_widget(&block, area);
            let inner = block.inner(area);
            frame.render_widget(Paragraph::new(Text::from(lines)), inner);
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
            let x = screen.x + (screen.width.saturating_sub(modal_w)) / 2;
            let y = screen.y + (screen.height.saturating_sub(modal_h)) / 2;
            let area = Rect::new(x, y, modal_w, modal_h);
            frame.render_widget(Clear, area);

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
                    lines.push(Line::from(Span::styled(display, Style::default().fg(Color::Black).bg(TOOL_COLOR))));
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

            frame.render_widget(&block, area);
            let inner = block.inner(area);
            frame.render_widget(Paragraph::new(Text::from(lines)), inner);
        }
        ModalState::None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_inline() {
        let spans = parse_inline_spans("🌤️ **Shenzhen**");
        for span in &spans {
            println!(
                "SPAN: '{}' BOLD: {}",
                span.content,
                span.style.add_modifier.contains(Modifier::BOLD)
            );
        }
        assert_eq!(spans.len(), 2);
        assert_eq!(spans[0].content, "🌤️ ");
        assert_eq!(spans[1].content, "Shenzhen");
        assert!(spans[1].style.add_modifier.contains(Modifier::BOLD));
    }
}
