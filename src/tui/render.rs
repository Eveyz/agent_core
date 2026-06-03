use super::state::{AppState, Entry, SubagentState, ToolResult, TurnBlock};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, BorderType, Paragraph, Wrap},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// ── Theme ───────────────────────────────────────────────────────────

const USER_BG: Color = Color::Rgb(28, 30, 42);
const CODE_BG: Color = Color::Rgb(22, 24, 29);
const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);
const SUBAGENT_COLOR: Color = Color::Rgb(198, 120, 221);
const SUCCESS_COLOR: Color = Color::Rgb(152, 195, 121);
const ERROR_COLOR: Color = Color::Rgb(224, 108, 117);

// ── Layout ──────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, state: &AppState) {
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
}

// ── Status bar ──────────────────────────────────────────────────────

fn render_status(frame: &mut Frame, state: &AppState, area: Rect) {
    let state_color = match state.agent_state.as_str() {
        "idle" => SUCCESS_COLOR,
        "streaming" => Color::Rgb(229, 192, 123),
        _ => Color::White,
    };

    let status_text = Line::from(vec![
        Span::styled(
            " 🤖 Agent Core ",
            Style::default().fg(Color::Rgb(97, 175, 239)).add_modifier(Modifier::BOLD),
        ),
        Span::raw(" │ "),
        Span::styled(format!("Model: {} ", state.model), Style::default().fg(Color::DarkGray)),
        Span::raw(" │ "),
        Span::styled(format!("Tokens: {} ", state.tokens), Style::default().fg(Color::DarkGray)),
        Span::raw(" │ "),
        Span::styled(format!("State: {} ", state.agent_state), Style::default().fg(state_color)),
    ]);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(Color::Rgb(92, 99, 112)));

    let para = Paragraph::new(status_text).block(block);
    frame.render_widget(para, area);
}

// ── Input bar ───────────────────────────────────────────────────────

fn render_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let prompt = Span::styled(" ❯ ", Style::default().fg(SUCCESS_COLOR).add_modifier(Modifier::BOLD));
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

// ── Conversation ────────────────────────────────────────────────────

fn render_conversation(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let width = area.width as usize;

    let mut first = true;
    for entry in &state.entries {
        if !first { lines.push(Line::raw("")); }
        first = false;
        render_entry(entry, &mut lines, width);
    }

    if let Some(ref streaming) = state.streaming {
        if !streaming.blocks.is_empty() {
            if !first { lines.push(Line::raw("")); }
            for block in &streaming.blocks {
                render_turn_block(block, &mut lines, width, 0);
            }
        }
    }
    lines.push(Line::raw(""));

    let text = Text::from(lines);
    // Use an estimated wrapped line count instead of drawing to buffer
    let max_scroll = wrapped_line_count(&text, area.width).saturating_sub(area.height as usize);
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
        Entry::User { text } => {
            render_user_block(text, lines, width);
        }
        Entry::Turn { blocks, .. } => {
            for block in blocks {
                render_turn_block(block, lines, width, 0);
            }
        }
    }
}

fn render_turn_block<'a>(block: &'a TurnBlock, lines: &mut Vec<Line<'a>>, width: usize, indent: usize) {
    let pad = " ".repeat(indent * 2);
    let inner_width = width.saturating_sub(pad.width());

    match block {
        TurnBlock::Thought(text) => {
            let style = Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC);
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
                    lines.push(Line::from(vec![Span::raw(pad.to_string()), Span::styled(line, style)]));
                }
            }
            lines.push(Line::raw(""));
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
            render_subagent_block(sa, lines, inner_width, &pad);
        }
        TurnBlock::Error(e) => {
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled(format!("✗ {e}"), Style::default().fg(ERROR_COLOR).add_modifier(Modifier::BOLD)),
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
    
    // Content lines
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
    
    // Bottom padding
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
    let block_width = width.min(100).max(12);
    let inner_width = block_width.saturating_sub(4);
    let border_style = Style::default().fg(TOOL_COLOR);
    
    let title = format!(" ⚙ {} ", name);
    
    // Title Line
    let mut title_spans = vec![Span::raw(pad.to_string()), Span::styled("╭─", border_style)];
    title_spans.push(Span::styled(title, border_style.add_modifier(Modifier::BOLD)));
    let remaining_len = block_width.saturating_sub(name.width() + 6);
    title_spans.push(Span::styled(format!("{}╮", "─".repeat(remaining_len)), border_style));
    lines.push(Line::from(title_spans));

    // Args
    if !args.trim().is_empty() {
        let arg_str = truncate_str_w(args, inner_width);
        let fill = " ".repeat(inner_width.saturating_sub(arg_str.width()));
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("│ ", border_style),
            Span::styled(arg_str, Style::default().fg(Color::DarkGray)),
            Span::raw(fill),
            Span::styled(" │", border_style),
        ]));
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("│", border_style),
            Span::raw(" ".repeat(inner_width + 2)),
            Span::styled("│", border_style),
        ]));
    }

    // Output
    if let Some(r) = result {
        let (prefix, style) = if r.is_error {
            ("✗ ", Style::default().fg(ERROR_COLOR))
        } else {
            ("→ ", Style::default().fg(SUCCESS_COLOR))
        };
        
        let mut shown = 0;
        for line in r.text.lines().take(8) {
            let prefix_w = prefix.width();
            let trunc_line = truncate_str_w(line, inner_width.saturating_sub(prefix_w));
            let fill = " ".repeat(inner_width.saturating_sub(prefix_w + trunc_line.width()));
            
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("│ ", border_style),
                Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                Span::styled(trunc_line, style),
                Span::raw(fill),
                Span::styled(" │", border_style),
            ]));
            shown += 1;
        }

        let total = r.text.lines().count();
        if total > shown {
            let msg = format!("… {} more lines", total - shown);
            let fill = " ".repeat(inner_width.saturating_sub(msg.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("│ ", border_style),
                Span::styled(msg, Style::default().fg(Color::DarkGray)),
                Span::raw(fill),
                Span::styled(" │", border_style),
            ]));
        }
    } else {
        let msg = "Waiting for result…";
        let fill = " ".repeat(inner_width.saturating_sub(msg.width()));
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("│ ", border_style),
            Span::styled(msg, Style::default().fg(Color::DarkGray)),
            Span::raw(fill),
            Span::styled(" │", border_style),
        ]));
    }

    // Bottom Line
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(format!("╰{}╯", "─".repeat(block_width.saturating_sub(2))), border_style),
    ]));
}

// ── Subagent block ──────────────────────────────────────────────────

fn render_subagent_block<'a>(sa: &'a SubagentState, lines: &mut Vec<Line<'a>>, width: usize, pad: &str) {
    let block_width = width.min(100).max(12);
    let inner_width = block_width.saturating_sub(4);
    
    let border_style = if sa.focused {
        Style::default().fg(SUBAGENT_COLOR).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(SUBAGENT_COLOR)
    };

    let title = format!(" ⚡ subagent: {} ", sa.id);
    let mut title_spans = vec![Span::raw(pad.to_string()), Span::styled("╭─", border_style)];
    title_spans.push(Span::styled(title, border_style.add_modifier(Modifier::BOLD)));
    let remaining_len = block_width.saturating_sub(sa.id.width() + 18);
    title_spans.push(Span::styled(format!("{}╮", "─".repeat(remaining_len)), border_style));
    lines.push(Line::from(title_spans));

    let toggle_hint = if sa.collapsed { "[Enter] expand" } else { "[Enter] collapse" };
    let task_str = truncate_str_w(&sa.task, inner_width.saturating_sub(toggle_hint.width() + 1));
    let fill = " ".repeat(inner_width.saturating_sub(task_str.width() + toggle_hint.width() + 1));
    
    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("│ ", border_style),
        Span::styled(task_str, Style::default().fg(Color::DarkGray)),
        Span::raw(fill),
        Span::raw(" "),
        Span::styled(toggle_hint, border_style),
        Span::styled(" │", border_style),
    ]));

    if !sa.collapsed {
        // Child border lines
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled(format!("├{}┤", "─".repeat(block_width.saturating_sub(2))), border_style),
        ]));

        let mut child_lines = Vec::new();
        for child in &sa.children {
            render_turn_block(child, &mut child_lines, inner_width, 0);
        }
        
        for mut child_line in child_lines {
            let lw = child_line.width();
            let fill = " ".repeat(inner_width.saturating_sub(lw));
            let mut final_spans = vec![Span::raw(pad.to_string()), Span::styled("│ ", border_style)];
            final_spans.extend(child_line.spans);
            final_spans.push(Span::raw(fill));
            final_spans.push(Span::styled(" │", border_style));
            lines.push(Line::from(final_spans));
        }

        if sa.done {
            let (status, status_style) = if sa.success {
                ("✓ done", Style::default().fg(SUCCESS_COLOR).add_modifier(Modifier::BOLD))
            } else {
                ("✗ incomplete", Style::default().fg(ERROR_COLOR).add_modifier(Modifier::BOLD))
            };
            let status_str = format!("{} ({} iterations)", status, sa.iterations);
            let fill = " ".repeat(inner_width.saturating_sub(status_str.width()));
            lines.push(Line::from(vec![
                Span::raw(pad.to_string()),
                Span::styled("│ ", border_style),
                Span::styled(status_str, status_style),
                Span::raw(fill),
                Span::styled(" │", border_style),
            ]));
        }
    }

    lines.push(Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled(format!("╰{}╯", "─".repeat(block_width.saturating_sub(2))), border_style),
    ]));
}

// ── Inline markdown → ratatui ───────────────────────────────────────

fn markdown_to_lines(text: &str, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Line<'static>> = Vec::new();
    let raw_lines: Vec<&str> = text.lines().collect();
    let mut idx = 0;

    while idx < raw_lines.len() {
        let raw_line = raw_lines[idx];
        let trimmed = raw_line.trim();

        if trimmed.starts_with("```") {
            let language = trimmed.trim_start_matches("```").trim();
            let mut code = Vec::new();
            idx += 1;
            while idx < raw_lines.len() {
                if raw_lines[idx].trim().starts_with("```") {
                    idx += 1;
                    break;
                }
                code.push(raw_lines[idx]);
                idx += 1;
            }
            lines.extend(render_code_block(language, &code, width));
            continue;
        }

        if let Some((table_lines, consumed)) = parse_markdown_table(&raw_lines[idx..], width) {
            lines.extend(table_lines);
            idx += consumed;
            continue;
        }

        // Headers
        if let Some(content) = trimmed.strip_prefix("### ") {
            lines.push(Line::from(Span::styled(content.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
            idx += 1; continue;
        }
        if let Some(content) = trimmed.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(content.to_string(), Style::default().fg(Color::White).add_modifier(Modifier::BOLD))));
            idx += 1; continue;
        }
        if let Some(content) = trimmed.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(content.to_string(), Style::default().fg(TOOL_COLOR).add_modifier(Modifier::BOLD))));
            idx += 1; continue;
        }

        // Blockquotes
        if let Some(content) = trimmed.strip_prefix("> ") {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                Span::styled(content.to_string(), Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)),
            ]));
            idx += 1; continue;
        }

        // Unordered list
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let content = &trimmed[2..];
            let mut spans = vec![Span::styled(" • ", Style::default().fg(TOOL_COLOR))];
            spans.extend(parse_inline_spans(content));
            lines.push(Line::from(spans));
            idx += 1; continue;
        }

        // Ordered list
        if let Some(dot_pos) = trimmed.find(". ") {
            let prefix = &trimmed[..dot_pos];
            if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                let content = &trimmed[dot_pos + 2..];
                let mut spans = vec![Span::styled(format!(" {prefix}. "), Style::default().fg(TOOL_COLOR))];
                spans.extend(parse_inline_spans(content));
                lines.push(Line::from(spans));
                idx += 1; continue;
            }
        }

        if trimmed.is_empty() {
            lines.push(Line::raw(""));
            idx += 1; continue;
        }

        let spans = parse_inline_spans(trimmed);
        if spans.is_empty() {
            lines.push(Line::raw(""));
        } else {
            lines.push(Line::from(spans));
        }
        idx += 1;
    }

    lines
}

fn parse_markdown_table(lines: &[&str], width: usize) -> Option<(Vec<Line<'static>>, usize)> {
    if lines.len() < 2 { return None; }
    let headers = split_table_row(lines[0])?;
    if !is_table_separator(lines[1], headers.len()) { return None; }

    let mut rows = Vec::new();
    let mut consumed = 2;
    while consumed < lines.len() {
        let Some(cells) = split_table_row(lines[consumed]) else { break; };
        if cells.len() != headers.len() { break; }
        rows.push(cells);
        consumed += 1;
    }

    let columns = headers.len();
    let spacing = columns.saturating_sub(1);
    let table_width = width.max(8);
    let available = table_width.saturating_sub(spacing).max(columns);
    
    let mut column_widths: Vec<usize> = (0..columns).map(|idx| {
        let hw = headers[idx].width();
        let rw = rows.iter().filter_map(|r| r.get(idx)).map(|c| c.width()).max().unwrap_or(0);
        hw.max(rw).max(4)
    }).collect();
    
    let mut total: usize = column_widths.iter().sum();
    while total > available {
        if let Some((idx, _)) = column_widths.iter().enumerate().filter(|(_, w)| **w > 4).max_by_key(|(_, w)| **w) {
            column_widths[idx] -= 1;
            total -= 1;
        } else {
            break;
        }
    }

    let mut output = Vec::new();
    
    // Header
    let mut header_spans = Vec::new();
    for (i, header) in headers.iter().enumerate() {
        let cw = column_widths[i];
        let trunc = truncate_str_w(header, cw);
        let fill = " ".repeat(cw.saturating_sub(trunc.width()));
        header_spans.push(Span::styled(format!("{}{}", trunc, fill), Style::default().add_modifier(Modifier::BOLD)));
        if i < columns - 1 { header_spans.push(Span::raw(" ")); }
    }
    output.push(Line::from(header_spans));

    // Separator
    let mut sep_spans = Vec::new();
    for (i, &cw) in column_widths.iter().enumerate() {
        sep_spans.push(Span::styled("─".repeat(cw), Style::default().fg(Color::DarkGray)));
        if i < columns - 1 { sep_spans.push(Span::raw(" ")); }
    }
    output.push(Line::from(sep_spans));

    // Rows
    for row in rows {
        let mut row_spans = Vec::new();
        for (i, cell) in row.iter().enumerate() {
            let cw = column_widths[i];
            let trunc = truncate_str_w(cell, cw);
            let fill = " ".repeat(cw.saturating_sub(trunc.width()));
            row_spans.push(Span::raw(format!("{}{}", trunc, fill)));
            if i < columns - 1 { row_spans.push(Span::raw(" ")); }
        }
        output.push(Line::from(row_spans));
    }

    Some((output, consumed))
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') { return None; }
    let trimmed = trimmed.trim_matches('|');
    let cells: Vec<String> = trimmed.split('|').map(|cell| cell.trim().replace("**", "").replace('`', "")).collect();
    if cells.len() >= 2 { Some(cells) } else { None }
}

fn is_table_separator(line: &str, columns: usize) -> bool {
    let Some(cells) = split_table_row(line) else { return false; };
    cells.len() == columns && cells.iter().all(|c| {
        let m = c.trim();
        m.len() >= 3 && m.chars().all(|ch| matches!(ch, '-' | ':' | ' ')) && m.chars().any(|ch| ch == '-')
    })
}

fn render_code_block(language: &str, code: &[&str], width: usize) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    let block_width = width.min(100).max(12);
    let inner_width = block_width.saturating_sub(4);
    
    let border_style = Style::default().fg(Color::Rgb(92, 99, 112)).bg(CODE_BG);
    let language = normalize_language(language);
    let title = if language.is_empty() { " code ".to_string() } else { format!(" {} ", language) };
    
    // Top Border
    let mut top_spans = vec![Span::styled("╭─", border_style)];
    top_spans.push(Span::styled(title, border_style.add_modifier(Modifier::BOLD).fg(Color::Yellow)));
    let rem = block_width.saturating_sub(top_spans.iter().map(|s| s.width()).sum());
    top_spans.push(Span::styled(format!("{}╮", "─".repeat(rem)), border_style));
    lines.push(Line::from(top_spans));

    for line in code {
        let trunc = truncate_str_w(line, inner_width);
        let mut spans = vec![Span::styled("│ ", border_style)];
        spans.extend(highlight_code_line(&trunc, language));
        
        let spans_w: usize = spans.iter().map(|s| s.width()).sum();
        let fill = " ".repeat(block_width.saturating_sub(spans_w + 1));
        spans.push(Span::styled(fill, Style::default().bg(CODE_BG)));
        spans.push(Span::styled("│", border_style));
        
        lines.push(Line::from(spans));
    }

    lines.push(Line::from(Span::styled(format!("╰{}╯", "─".repeat(block_width.saturating_sub(2))), border_style)));
    lines
}

fn normalize_language(language: &str) -> &str {
    match language.trim().to_ascii_lowercase().as_str() {
        "c++" | "cpp" | "cc" | "cxx" => "cpp",
        "rs" | "rust" => "rust",
        "py" | "python" => "python",
        "js" | "javascript" | "ts" | "typescript" => "js",
        "json" => "json",
        _ => language.trim(),
    }
}

fn highlight_code_line(line: &str, language: &str) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut token = String::new();
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '/' && chars.peek() == Some(&'/') {
            flush_code_token(&mut spans, &mut token, language);
            let mut comment = String::from("//");
            comment.extend(chars);
            spans.push(Span::styled(comment, Style::default().fg(Color::DarkGray).bg(CODE_BG).add_modifier(Modifier::ITALIC)));
            return spans;
        }

        if ch == '"' || ch == '\'' {
            flush_code_token(&mut spans, &mut token, language);
            let quote = ch;
            let mut quoted = String::from(ch);
            let mut escaped = false;
            for next in chars.by_ref() {
                quoted.push(next);
                if escaped { escaped = false; } else if next == '\\' { escaped = true; } else if next == quote { break; }
            }
            spans.push(Span::styled(quoted, Style::default().fg(SUCCESS_COLOR).bg(CODE_BG)));
            continue;
        }

        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            flush_code_token(&mut spans, &mut token, language);
            let style = if "{}[]();,.".contains(ch) { Style::default().fg(Color::Gray).bg(CODE_BG) }
                        else if "+-*/=%!<>:&|".contains(ch) { Style::default().fg(Color::Magenta).bg(CODE_BG) }
                        else { Style::default().fg(Color::White).bg(CODE_BG) };
            spans.push(Span::styled(ch.to_string(), style));
        }
    }
    flush_code_token(&mut spans, &mut token, language);
    spans
}

fn flush_code_token(spans: &mut Vec<Span<'static>>, token: &mut String, language: &str) {
    if token.is_empty() { return; }
    let style = if is_code_keyword(token, language) { Style::default().fg(TOOL_COLOR).bg(CODE_BG).add_modifier(Modifier::BOLD) }
                else if token.chars().all(|ch| ch.is_ascii_digit()) { Style::default().fg(Color::Yellow).bg(CODE_BG) }
                else if token.chars().next().is_some_and(|ch| ch.is_ascii_uppercase()) { Style::default().fg(Color::LightBlue).bg(CODE_BG) }
                else { Style::default().fg(Color::White).bg(CODE_BG) };
    spans.push(Span::styled(std::mem::take(token), style));
}

fn is_code_keyword(token: &str, language: &str) -> bool {
    let common = matches!(token, "class" | "struct" | "enum" | "public" | "private" | "protected" | "return" | "if" | "else" | "for" | "while" | "true" | "false" | "null" | "let" | "mut" | "fn" | "async" | "await" | "const" | "static" | "void" | "int" | "bool" | "def" | "self" | "import" | "from" | "try" | "catch");
    common || matches!((language, token), ("rust", "impl" | "trait" | "match" | "pub" | "crate" | "use") | ("cpp", "include" | "template" | "typename" | "vector" | "string") | ("js", "function" | "const" | "var" | "new" | "this" | "export"))
}

fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;
    let normal = Style::default().fg(Color::White);

    while !remaining.is_empty() {
        if let Some(rest) = remaining.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                spans.push(Span::styled(rest[..end].to_string(), normal.add_modifier(Modifier::BOLD)));
                remaining = &rest[end + 2..]; continue;
            }
        }
        if remaining.starts_with('*') && !remaining.starts_with("**") {
            let rest = &remaining[1..];
            if let Some(end) = rest.find('*') {
                spans.push(Span::styled(rest[..end].to_string(), normal.add_modifier(Modifier::ITALIC)));
                remaining = &rest[end + 1..]; continue;
            }
        }
        if let Some(rest) = remaining.strip_prefix('`') {
            if let Some(end) = rest.find('`') {
                spans.push(Span::styled(rest[..end].to_string(), Style::default().fg(Color::Yellow).bg(Color::Rgb(30, 30, 40))));
                remaining = &rest[end + 1..]; continue;
            }
        }

        let next_special = remaining.find(|c| c == '*' || c == '`').unwrap_or(remaining.len());
        if next_special > 0 {
            spans.push(Span::styled(remaining[..next_special].to_string(), normal));
            remaining = &remaining[next_special..];
        } else if let Some(ch) = remaining.chars().next() {
            spans.push(Span::styled(ch.to_string(), normal));
            remaining = &remaining[ch.len_utf8()..];
        } else { break; }
    }

    if spans.is_empty() && !text.is_empty() { spans.push(Span::styled(text.to_string(), normal)); }
    spans
}

fn truncate_str_w(s: &str, max_width: usize) -> String {
    if s.width() <= max_width { return s.to_string(); }
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
