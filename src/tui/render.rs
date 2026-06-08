use super::state::{AppState, Entry, SubagentState, ToolResult, TurnBlock};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Paragraph, Wrap},
};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── Theme ───────────────────────────────────────────────────────────

const USER_BG: Color = Color::Rgb(28, 30, 42);
const CODE_BG: Color = Color::Rgb(22, 24, 29);
const INLINE_CODE_BG: Color = Color::Rgb(35, 35, 50);
const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const SUBAGENT_COLOR: Color = Color::Rgb(198, 120, 221);
const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);
const ERROR_COLOR: Color = Color::Rgb(224, 108, 117);
const WARN_COLOR: Color = Color::Rgb(229, 192, 123);

// ── Syntect globals (loaded once) ────────────────────────────────────

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SYNTAX_THEME: LazyLock<syntect::highlighting::Theme> = LazyLock::new(|| {
    let ts = ThemeSet::load_defaults();
    ts.themes["base16-ocean.dark"].clone()
});

// ── Layout ──────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let [status_area, main_area, input_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .areas(area);

    render_status(frame, state, status_area);
    render_conversation(frame, state, main_area);
    render_input(frame, state, input_area);
    render_autocomplete(frame, state, input_area);
}

// ── Status bar ──────────────────────────────────────────────────────

fn render_status(frame: &mut Frame, state: &AppState, area: Rect) {
    let state_color = match state.agent_state.as_str() {
        "idle" => SUCCESS_COLOR,
        "streaming" => Color::Rgb(229, 192, 123),
        _ => Color::White,
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
            format!("State: {} ", state.agent_state),
            Style::default().fg(state_color),
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

// ── Autocomplete ────────────────────────────────────────────────────

fn render_autocomplete(frame: &mut Frame, state: &AppState, input_area: Rect) {
    if !state.autocomplete.active || state.autocomplete.filtered_options.is_empty() {
        return;
    }

    let max_height = 10;
    let total_options = state.autocomplete.filtered_options.len();

    let start = if total_options > (max_height - 2) {
        state
            .autocomplete
            .selected_index
            .saturating_sub(max_height - 3)
            .min(total_options.saturating_sub(max_height - 2))
    } else {
        0
    };
    let end = (start + max_height - 2).min(total_options);

    let display_options = &state.autocomplete.filtered_options[start..end];

    let items: Vec<ListItem> = display_options
        .iter()
        .enumerate()
        .map(|(i, opt)| {
            let actual_idx = start + i;
            let style = if actual_idx == state.autocomplete.selected_index {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            ListItem::new(Span::styled(format!(" {} ", opt), style))
        })
        .collect();

    let height = items.len() as u16 + 2; // +2 for borders
    let height = height.min(max_height as u16);

    // Calculate Rect above the input area
    let x = input_area.x;
    let y = input_area.y.saturating_sub(height);
    let width = 30.min(input_area.width);

    let area = Rect::new(x, y, width, height);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Cyan))
        .title(" Commands ");

    let list = List::new(items).block(block);

    frame.render_widget(Clear, area);
    frame.render_widget(list, area);
}

// ── Conversation ────────────────────────────────────────────────────

fn render_conversation(frame: &mut Frame, state: &mut AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let width = area.width as usize;

    let mut first = true;
    for entry in &state.entries {
        if !first {
            lines.push(Line::raw(""));
        }
        first = false;
        render_entry(entry, &mut lines, width);
    }

    if let Some(ref streaming) = state.streaming {
        if !streaming.blocks.is_empty() {
            if !first {
                lines.push(Line::raw(""));
            }
            for (i, block) in streaming.blocks.iter().enumerate() {
                if i > 0 {
                    lines.push(Line::raw(""));
                }
                render_turn_block(block, &mut lines, width, 0);
            }
        }
    }
    lines.push(Line::raw(""));

    let text = Text::from(lines);
    let max_scroll = wrapped_line_count(&text, area.width).saturating_sub(area.height as usize);

    // Clamp scroll offset so it never exceeds the actual scrollable range.
    // This prevents jumps when content first exceeds the viewport.
    state.scroll = state.scroll.min(max_scroll);

    let scroll = max_scroll.saturating_sub(state.scroll);

    let para = Paragraph::new(text)
        .wrap(Wrap { trim: false })
        .scroll((scroll.min(u16::MAX as usize) as u16, 0));

    frame.render_widget(para, area);
}

fn wrapped_line_count(text: &Text<'_>, width: u16) -> usize {
    let width = usize::from(width).max(1);
    text.lines
        .iter()
        .map(|line| {
            let lw = line.width();
            if lw == 0 { 1 } else { lw.div_ceil(width) }
        })
        .sum()
}

fn render_entry<'a>(entry: &'a Entry, lines: &mut Vec<Line<'a>>, width: usize) {
    match entry {
        Entry::System { text } => {
            let style = Style::default().fg(Color::Cyan);
            for line in text.lines() {
                lines.push(Line::from(vec![Span::styled(line, style)]));
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
                render_turn_block(block, lines, width, 0);
            }
        }
    }
}

fn render_turn_block<'a>(
    block: &'a TurnBlock,
    lines: &mut Vec<Line<'a>>,
    width: usize,
    indent: usize,
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
                        Span::raw(pad.to_string()),
                        Span::styled("💭 Thought: ", style.add_modifier(Modifier::BOLD)),
                        Span::styled(line, style),
                    ]));
                    first = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(pad.to_string()),
                        Span::styled(line, style),
                    ]));
                }
            }
        }
        TurnBlock::Response(text) => {
            let md_lines = markdown_to_lines(text);
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
            render_subagent_block(sa, lines, inner_width, &pad);
        }
        TurnBlock::Error(e) => {
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled(
                    format!("✗ {e}"),
                    Style::default()
                        .fg(ERROR_COLOR)
                        .add_modifier(Modifier::BOLD),
                ),
            ]));
        }
        TurnBlock::Notice(msg) => {
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled(
                    format!("⚠ {msg}"),
                    Style::default().fg(WARN_COLOR).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
}

// ── User block ──────────────────────────────────────────────────────

fn render_user_block<'a>(text: &str, lines: &mut Vec<Line<'a>>, width: usize) {
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

fn render_tool_block<'a>(
    name: &'a str,
    args: &'a str,
    result: &'a Option<ToolResult>,
    lines: &mut Vec<Line<'a>>,
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
    let label = format!("  ⚙  {}  ", name);
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
                Span::styled(prefix, out_style.add_modifier(Modifier::BOLD)),
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
            Span::styled(msg, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
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

// ── Subagent block ──────────────────────────────────────────────────

fn render_subagent_block<'a>(
    sa: &'a SubagentState,
    lines: &mut Vec<Line<'a>>,
    width: usize,
    pad: &str,
) {
    // Account for pad + "  " left + "  " right = pad.width() + 4
    let inner_width = width.saturating_sub(4 + pad.width());
    let bg = Style::default().bg(CODE_BG);
    let label_style = Style::default()
        .fg(SUBAGENT_COLOR)
        .bg(CODE_BG)
        .add_modifier(Modifier::BOLD);

    // Top padding — fill full width
    let top_fill = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(top_fill), bg),
    ]));

    // Label line
    let label = format!("  ⚡  subagent: {}  ", sa.id);
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

    // Task line + toggle hint
    let toggle_hint = if sa.collapsed {
        "[Enter] expand"
    } else {
        "[Enter] collapse"
    };
    let task_avail = inner_width.saturating_sub(toggle_hint.width() + 1);
    let task_str = truncate_str_w(&sa.task, task_avail);
    let fill = " ".repeat(inner_width.saturating_sub(task_str.width() + toggle_hint.width() + 1));
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", bg),
        Span::styled(task_str, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
        Span::styled(fill, bg),
        Span::raw(" "),
        Span::styled(toggle_hint, Style::default().fg(SUBAGENT_COLOR).bg(CODE_BG)),
        Span::styled("  ", bg),
    ]));

    if !sa.collapsed {
        let mut child_lines = Vec::new();
        for child in &sa.children {
            render_turn_block(child, &mut child_lines, inner_width, 0);
        }

        for child_line in child_lines {
            let lw = child_line.width();
            let fill = " ".repeat(inner_width.saturating_sub(lw));
            let mut final_spans = vec![Span::raw(pad.to_string()), Span::styled("  ", bg)];
            final_spans.extend(child_line.spans);
            final_spans.push(Span::raw(fill));
            final_spans.push(Span::styled("  ", bg));
            lines.push(Line::from(final_spans));
        }

        if sa.done {
            let (status, status_style) = if sa.success {
                (
                    "✓ done",
                    Style::default()
                        .fg(SUCCESS_COLOR)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "✗ incomplete",
                    Style::default()
                        .fg(ERROR_COLOR)
                        .add_modifier(Modifier::BOLD),
                )
            };
            let status_str = format!("{} ({} iterations)", status, sa.iterations);
            let fill = " ".repeat(inner_width.saturating_sub(status_str.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("  ", bg),
                Span::styled(status_str, status_style.bg(CODE_BG)),
                Span::styled(fill, bg),
                Span::styled("  ", bg),
            ]));
        }
    }

    // Bottom padding — fill full width
    let bot_fill = width.saturating_sub(pad.width());
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(" ".repeat(bot_fill), bg),
    ]));
}

// ── Markdown → Ratatui (pulldown-cmark) ──────────────────────────────

fn markdown_to_lines(text: &str) -> Vec<Line<'static>> {
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
                TagEnd::Paragraph => flush_md_line(&mut cur_spans, &mut lines, false),
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
                    lines.extend(render_syntect_block(&code_lang, &code_content));
                    code_content.clear();
                    code_lang.clear();
                    lines.push(Line::raw(""));
                }
                TagEnd::List(_) => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
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
                    if heading_level.is_some() {
                        final_style = final_style.add_modifier(Modifier::BOLD);
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

// ── Code blocks (syntect) ──────────────────────────────────────────

fn render_syntect_block(language: &str, code: &str) -> Vec<Line<'static>> {
    let syntax = find_syntax(language);
    let theme = &SYNTAX_THEME;
    let mut highlighter = HighlightLines::new(syntax, theme);

    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let line = line.trim_end_matches(['\n', '\r']);
        if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
            let spans: Vec<Span> = ranges
                .iter()
                .map(|(style, text)| {
                    let fg = syntect_color_to_ratatui(style.foreground);
                    Span::styled(text.to_string(), Style::default().fg(fg).bg(CODE_BG))
                })
                .collect();
            lines.push(Line::from(spans));
        }
    }

    // Wrap in top label + background padding
    let border_style = Style::default().fg(Color::Rgb(92, 99, 112)).bg(CODE_BG);
    let label_style = Style::default()
        .fg(Color::Yellow)
        .bg(CODE_BG)
        .add_modifier(Modifier::BOLD);

    let title = format!(
        " {} ",
        if language.is_empty() {
            "code"
        } else {
            language
        }
    );
    let mut result = vec![
        Line::from(Span::styled(" ", border_style)),
        Line::from(vec![
            Span::styled("  ", border_style),
            Span::styled(title, label_style),
        ]),
        Line::from(vec![
            Span::styled("  ", border_style),
            Span::styled("─".repeat(30), border_style),
        ]),
    ];
    result.append(&mut lines);
    result.push(Line::from(Span::styled(" ", border_style)));
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
