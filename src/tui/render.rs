use super::state::{AppState, Entry, SubagentState, ToolResult, TurnBlock};
use ratatui::{
    Frame,
    buffer::Buffer,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span, Text},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Widget, Wrap},
};

// ── Layout ──────────────────────────────────────────────────────────

pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();

    let [status_area, main_area, input_area] = Layout::vertical([
        Constraint::Length(1),
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
    let status_text = Line::from(vec![
        Span::styled(
            " Agent Core ",
            Style::default().bg(Color::DarkGray).fg(Color::Black),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" Model: {} ", state.model),
            Style::default().fg(Color::Cyan),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" {} tokens ", state.tokens),
            Style::default().fg(Color::Yellow),
        ),
        Span::raw("  "),
        Span::styled(
            format!(" State: {} ", state.agent_state),
            match state.agent_state.as_str() {
                "idle" => Style::default().fg(Color::Green),
                "streaming" => Style::default().fg(Color::Yellow),
                _ => Style::default().fg(Color::White),
            },
        ),
    ]);

    let para = Paragraph::new(status_text).style(Style::default().bg(Color::Rgb(18, 18, 24)));
    frame.render_widget(para, area);
}

// ── Input bar ───────────────────────────────────────────────────────

fn render_input(frame: &mut Frame, state: &AppState, area: Rect) {
    let prompt = Span::styled(
        "> ",
        Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
    );
    let text = Span::raw(&state.input);
    let cursor = if state.input.is_empty() {
        Span::styled(" ", Style::default().bg(Color::White).fg(Color::Black))
    } else {
        Span::raw("")
    };

    let line = Line::from(vec![prompt, text, cursor]);
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Rgb(18, 18, 24)));
    let para = Paragraph::new(line).block(block);
    frame.render_widget(para, area);

    let cursor_width = unicode_width::UnicodeWidthStr::width(
        &state.input[..state.cursor_pos.min(state.input.len())],
    );
    let cursor_x = 2 + cursor_width.min(area.width.saturating_sub(1) as usize) as u16;
    frame.set_cursor_position((area.x + cursor_x, area.y + 1));
}

// ── Conversation ────────────────────────────────────────────────────

fn render_conversation(frame: &mut Frame, state: &AppState, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    let width = area.width as usize;

    // Render committed entries
    let mut first = true;
    for entry in &state.entries {
        if !first {
            lines.push(Line::raw(""));
        }
        first = false;
        render_entry(entry, &mut lines, width);
    }

    // Render streaming content
    if let Some(ref streaming) = state.streaming {
        if !streaming.blocks.is_empty() {
            if !first {
                lines.push(Line::raw(""));
            }
            for block in &streaming.blocks {
                render_turn_block(block, &mut lines, width, 0);
            }
        }
    }
    lines.push(Line::raw(""));

    let text = Text::from(lines);
    let max_scroll = wrapped_line_count(&text, area.width).saturating_sub(area.height as usize);
    let para = Paragraph::new(text).wrap(Wrap { trim: false });
    let scroll = max_scroll.saturating_sub(state.scroll);
    let para = para.scroll((scroll.min(u16::MAX as usize) as u16, 0));

    frame.render_widget(para, area);
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

fn render_turn_block<'a>(
    block: &'a TurnBlock,
    lines: &mut Vec<Line<'a>>,
    width: usize,
    indent: usize,
) {
    let pad = "  ".repeat(indent);
    let inner = width.saturating_sub(indent * 2);

    match block {
        TurnBlock::Thought(text) => {
            let style = Style::default()
                .fg(Color::Gray)
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
                        Span::styled("Thought: ", style.add_modifier(Modifier::BOLD)),
                        Span::styled(line, style),
                    ]));
                    first = false;
                } else {
                    lines.push(Line::from(vec![
                        Span::raw(pad.clone()),
                        Span::styled(line, style),
                    ]));
                }
            }
            // Margin after thought
            lines.push(Line::raw(""));
        }
        TurnBlock::Response(text) => {
            let md_lines = markdown_to_lines(text, inner);
            for line in md_lines {
                let mut spans = vec![Span::raw(pad.clone())];
                spans.extend(line.spans);
                lines.push(Line::from(spans));
            }
        }
        TurnBlock::Tool { name, args, result } => {
            render_tool_block(name, args, result, lines, width, indent);
        }
        TurnBlock::Subagent(sa) => {
            render_subagent_block(sa, lines, width, indent);
        }
        TurnBlock::Error(e) => {
            lines.push(Line::from(vec![
                Span::raw(pad.clone()),
                Span::styled(
                    format!("✗ {e}"),
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                ),
            ]));
        }
    }
}

// ── User block ──────────────────────────────────────────────────────
fn render_user_block<'a>(text: &str, lines: &mut Vec<Line<'a>>, width: usize) {
    let bg = Color::Rgb(18, 18, 26);
    let inner_width = width.saturating_sub(4);
    let pad_right = inner_width;
    // Top padding
    lines.push(Line::from(Span::styled(
        " ".repeat(width),
        Style::default().bg(bg),
    )));
    // Content lines
    for line in text.lines() {
        let line_str = line.to_string();
        let fill = pad_right.saturating_sub(line_str.len() + 2);
        lines.push(Line::from(vec![
            Span::styled("  ", Style::default().bg(bg)),
            Span::styled(line_str, Style::default().fg(Color::White).bg(bg)),
            Span::styled(" ".repeat(fill), Style::default().bg(bg)),
        ]));
    }
    // Bottom padding
    lines.push(Line::from(Span::styled(
        " ".repeat(width),
        Style::default().bg(bg),
    )));
}

// ── Tool block ──────────────────────────────────────────────────────
fn render_tool_block<'a>(
    name: &str,
    args: &str,
    result: &Option<ToolResult>,
    lines: &mut Vec<Line<'a>>,
    width: usize,
    indent: usize,
) {
    let pad = "  ".repeat(indent);
    let tool_width = width.saturating_sub(pad.len()).max(12);
    let inner_width = tool_width.saturating_sub(4);
    let border_style = Style::default().fg(Color::Cyan);
    let title = format!(" ⚙ {name} ");
    let mut body = Vec::new();

    if !args.trim().is_empty() {
        body.push(Line::from(Span::styled(
            truncate_str_w(args, inner_width),
            Style::default().fg(Color::DarkGray),
        )));
        body.push(Line::raw(""));
    }

    if let Some(r) = result {
        body.push(Line::from(Span::styled(
            "Output",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));

        let (prefix, style) = if r.is_error {
            ("✗ ", Style::default().fg(Color::Red))
        } else {
            ("→ ", Style::default().fg(Color::Green))
        };
        let mut shown = 0usize;
        for line in r.text.lines().take(8) {
            let prefix_width = unicode_width::UnicodeWidthStr::width(prefix);
            body.push(Line::from(vec![
                Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                Span::styled(
                    truncate_str_w(line, inner_width.saturating_sub(prefix_width)),
                    style,
                ),
            ]));
            shown += 1;
        }

        let total = r.text.lines().count();
        if total > shown {
            body.push(Line::from(Span::styled(
                format!("… {} more lines", total - shown),
                Style::default().fg(Color::DarkGray),
            )));
        }
    }

    if body.is_empty() {
        body.push(Line::from(Span::styled(
            "Waiting for result…",
            Style::default().fg(Color::DarkGray),
        )));
    }

    let area = Rect::new(
        0,
        0,
        tool_width.min(u16::MAX as usize) as u16,
        body.len() as u16 + 2,
    );
    let mut buf = Buffer::empty(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ));

    Paragraph::new(Text::from(body))
        .block(block)
        .render(area, &mut buf);

    for y in 0..area.height {
        let mut row = buffer_row_to_line(&buf, y);
        if !pad.is_empty() {
            row.spans.insert(0, Span::raw(pad.clone()));
        }
        lines.push(row);
    }
}
// ── Subagent block ──────────────────────────────────────────────────

fn render_subagent_block<'a>(
    sa: &'a SubagentState,
    lines: &mut Vec<Line<'a>>,
    width: usize,
    indent: usize,
) {
    let pad = "  ".repeat(indent);
    let block_width = width.saturating_sub(pad.len()).max(12);
    let inner_width = block_width.saturating_sub(4);
    let toggle_hint = if sa.collapsed {
        "[Enter] expand"
    } else {
        "[Enter] collapse"
    };
    let border_style = if sa.focused {
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Yellow)
    };

    let mut body = vec![Line::from(vec![
        Span::styled(
            truncate_str_w(&sa.task, inner_width.saturating_sub(toggle_hint.len() + 1)),
            Style::default().fg(Color::DarkGray),
        ),
        Span::raw(" "),
        Span::styled(toggle_hint, border_style),
    ])];

    if !sa.collapsed {
        body.push(Line::raw(""));

        let mut child_lines = Vec::new();
        for child in &sa.children {
            render_turn_block(child, &mut child_lines, inner_width, 0);
        }
        body.extend(child_lines);

        if sa.done {
            let (status, status_style) = if sa.success {
                (
                    "✓ done",
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD),
                )
            } else {
                (
                    "✗ incomplete",
                    Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
                )
            };
            body.push(Line::from(Span::styled(
                format!("{status} ({n} iterations)", n = sa.iterations),
                status_style,
            )));
        }
    }

    let area = Rect::new(
        0,
        0,
        block_width.min(u16::MAX as usize) as u16,
        body.len() as u16 + 2,
    );
    let mut buf = Buffer::empty(area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            format!(" ⚡ subagent: {} ", sa.id),
            border_style.add_modifier(Modifier::BOLD),
        ));

    Paragraph::new(Text::from(body))
        .block(block)
        .render(area, &mut buf);

    for y in 0..area.height {
        let mut row = buffer_row_to_line(&buf, y);
        if !pad.is_empty() {
            row.spans.insert(0, Span::raw(pad.clone()));
        }
        lines.push(row);
    }
}

// ── Inline markdown → ratatui ───────────────────────────────────────

/// Convert a markdown string to ratatui `Line`s with basic formatting.
/// Handles: **bold**, *italic*, `code`, # headers, - lists, > blockquotes, ``` code blocks.
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
            lines.push(Line::from(Span::styled(
                content.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            idx += 1;
            continue;
        }
        if let Some(content) = trimmed.strip_prefix("## ") {
            lines.push(Line::from(Span::styled(
                content.to_string(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )));
            idx += 1;
            continue;
        }
        if let Some(content) = trimmed.strip_prefix("# ") {
            lines.push(Line::from(Span::styled(
                content.to_string(),
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            idx += 1;
            continue;
        }

        // Blockquotes
        if let Some(content) = trimmed.strip_prefix("> ") {
            lines.push(Line::from(vec![
                Span::styled("│ ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    content.to_string(),
                    Style::default()
                        .fg(Color::Gray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
            idx += 1;
            continue;
        }

        // Unordered list
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
            let content = &trimmed[2..];
            let bullet = Span::styled(" • ", Style::default().fg(Color::DarkGray));
            let mut spans = vec![bullet];
            spans.extend(parse_inline_spans(content));
            lines.push(Line::from(spans));
            idx += 1;
            continue;
        }

        // Ordered list
        if let Some(dot_pos) = trimmed.find(". ") {
            let prefix = &trimmed[..dot_pos];
            if !prefix.is_empty() && prefix.chars().all(|c| c.is_ascii_digit()) {
                let content = &trimmed[dot_pos + 2..];
                let num_span =
                    Span::styled(format!(" {prefix}. "), Style::default().fg(Color::DarkGray));
                let mut spans = vec![num_span];
                spans.extend(parse_inline_spans(content));
                lines.push(Line::from(spans));
                idx += 1;
                continue;
            }
        }

        // Empty line
        if trimmed.is_empty() {
            lines.push(Line::raw(""));
            idx += 1;
            continue;
        }

        // Regular paragraph line with inline formatting
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
    if lines.len() < 2 {
        return None;
    }

    let headers = split_table_row(lines[0])?;
    if !is_table_separator(lines[1], headers.len()) {
        return None;
    }

    let mut rows = Vec::new();
    let mut consumed = 2;
    while consumed < lines.len() {
        let Some(cells) = split_table_row(lines[consumed]) else {
            break;
        };
        if cells.len() != headers.len() {
            break;
        }
        rows.push(cells);
        consumed += 1;
    }

    let table_width = width.max(8);
    let table_height = rows.len().saturating_add(1).min(u16::MAX as usize) as u16;
    let area = Rect::new(
        0,
        0,
        table_width.min(u16::MAX as usize) as u16,
        table_height,
    );
    let mut buf = Buffer::empty(area);
    let widths = table_column_widths(&headers, &rows, table_width);
    let header = Row::new(
        headers
            .into_iter()
            .map(|cell| Cell::from(cell).style(Style::default().add_modifier(Modifier::BOLD))),
    )
    .style(Style::default().fg(Color::White));
    let body = rows.into_iter().map(|row| {
        Row::new(
            row.into_iter()
                .map(|cell| Cell::from(cell).style(Style::default().fg(Color::White))),
        )
    });

    Table::new(body, widths)
        .header(header)
        .column_spacing(1)
        .render(area, &mut buf);

    let mut rendered = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        rendered.push(buffer_row_to_line(&buf, y));
    }

    Some((rendered, consumed))
}

fn render_code_block(language: &str, code: &[&str], width: usize) -> Vec<Line<'static>> {
    let code_width = width.max(12);
    let inner_width = code_width.saturating_sub(4);
    let area = Rect::new(
        0,
        0,
        code_width.min(u16::MAX as usize) as u16,
        code.len().saturating_add(2).min(u16::MAX as usize) as u16,
    );
    let mut buf = Buffer::empty(area);
    let language = normalize_language(language);
    let title = if language.is_empty() {
        " code ".to_string()
    } else {
        format!(" {language} ")
    };
    let body = code.iter().map(|line| {
        Line::from(highlight_code_line(
            &truncate_str_w(line, inner_width),
            language,
        ))
    });

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .style(Style::default().bg(Color::Rgb(23, 25, 32)))
        .title(Span::styled(
            title,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));

    Paragraph::new(Text::from_iter(body))
        .block(block)
        .render(area, &mut buf);

    let mut rendered = Vec::with_capacity(area.height as usize);
    for y in 0..area.height {
        rendered.push(buffer_row_to_line(&buf, y));
    }

    rendered
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
            chars.next();
            comment.extend(chars);
            spans.push(Span::styled(
                comment,
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ));
            return spans;
        }

        if ch == '"' || ch == '\'' {
            flush_code_token(&mut spans, &mut token, language);
            let quote = ch;
            let mut quoted = String::from(ch);
            let mut escaped = false;
            for next in chars.by_ref() {
                quoted.push(next);
                if escaped {
                    escaped = false;
                } else if next == '\\' {
                    escaped = true;
                } else if next == quote {
                    break;
                }
            }
            spans.push(Span::styled(quoted, Style::default().fg(Color::Green)));
            continue;
        }

        if ch.is_ascii_alphanumeric() || ch == '_' {
            token.push(ch);
        } else {
            flush_code_token(&mut spans, &mut token, language);
            let style = if "{}[]();,.".contains(ch) {
                Style::default().fg(Color::Gray)
            } else if "+-*/=%!<>:&|".contains(ch) {
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::White)
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
    }

    flush_code_token(&mut spans, &mut token, language);
    spans
}

fn flush_code_token(spans: &mut Vec<Span<'static>>, token: &mut String, language: &str) {
    if token.is_empty() {
        return;
    }

    let style = if is_code_keyword(token, language) {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if token.chars().all(|ch| ch.is_ascii_digit()) {
        Style::default().fg(Color::Yellow)
    } else if token
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        Style::default().fg(Color::LightBlue)
    } else {
        Style::default().fg(Color::White)
    };
    spans.push(Span::styled(std::mem::take(token), style));
}

fn is_code_keyword(token: &str, language: &str) -> bool {
    let common = matches!(
        token,
        "class"
            | "struct"
            | "enum"
            | "public"
            | "private"
            | "protected"
            | "return"
            | "if"
            | "else"
            | "for"
            | "while"
            | "true"
            | "false"
            | "null"
            | "nullptr"
            | "let"
            | "mut"
            | "fn"
            | "async"
            | "await"
            | "const"
            | "static"
            | "void"
            | "int"
            | "long"
            | "double"
            | "float"
            | "bool"
            | "auto"
            | "using"
            | "namespace"
            | "def"
            | "self"
            | "import"
            | "from"
            | "try"
            | "catch"
            | "except"
    );
    common
        || matches!(
            (language, token),
            ("rust", "impl" | "trait" | "match" | "pub" | "crate" | "use")
                | (
                    "cpp",
                    "include" | "template" | "typename" | "vector" | "string"
                )
                | (
                    "js",
                    "function" | "const" | "var" | "new" | "this" | "export"
                )
        )
}

fn split_table_row(line: &str) -> Option<Vec<String>> {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return None;
    }

    let trimmed = trimmed.trim_matches('|');
    let cells: Vec<String> = trimmed
        .split('|')
        .map(|cell| strip_inline_markers(cell.trim()))
        .collect();

    if cells.len() >= 2 { Some(cells) } else { None }
}

fn is_table_separator(line: &str, columns: usize) -> bool {
    let Some(cells) = split_table_row(line) else {
        return false;
    };
    cells.len() == columns
        && cells.iter().all(|cell| {
            let marker = cell.trim();
            marker.len() >= 3
                && marker.chars().all(|ch| matches!(ch, '-' | ':' | ' '))
                && marker.chars().any(|ch| ch == '-')
        })
}

fn table_column_widths(
    headers: &[String],
    rows: &[Vec<String>],
    table_width: usize,
) -> Vec<Constraint> {
    let columns = headers.len().max(1);
    let spacing = columns.saturating_sub(1);
    let available = table_width.saturating_sub(spacing).max(columns);
    let min_width = 4;
    let mut widths: Vec<usize> = (0..columns)
        .map(|idx| {
            let header_width = unicode_width::UnicodeWidthStr::width(headers[idx].as_str());
            let row_width = rows
                .iter()
                .filter_map(|row| row.get(idx))
                .map(|cell| unicode_width::UnicodeWidthStr::width(cell.as_str()))
                .max()
                .unwrap_or(0);
            header_width.max(row_width).max(min_width)
        })
        .collect();

    let mut total: usize = widths.iter().sum();
    while total > available {
        let Some((idx, _)) = widths
            .iter()
            .enumerate()
            .filter(|(_, width)| **width > min_width)
            .max_by_key(|(_, width)| **width)
        else {
            break;
        };
        widths[idx] -= 1;
        total -= 1;
    }

    widths
        .into_iter()
        .map(|width| Constraint::Length(width.min(u16::MAX as usize) as u16))
        .collect()
}

fn strip_inline_markers(text: &str) -> String {
    text.replace("**", "").replace('`', "")
}

fn wrapped_line_count(text: &Text<'_>, width: u16) -> usize {
    let width = usize::from(width).max(1);
    text.lines
        .iter()
        .map(|line| line.width().max(1).div_ceil(width))
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_table_renders_without_separator_row() {
        let rendered = markdown_to_lines(
            "| Metric | Value |\n| --- | --- |\n| Condition | Thunderstorm |\n| Humidity | 97% |",
            40,
        );
        let text = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("Metric"));
        assert!(text.contains("Thunderstorm"));
        assert!(!text.contains("---"));
    }

    #[test]
    fn fenced_code_renders_as_code_block() {
        let rendered = markdown_to_lines("```cpp\nclass Solution {\npublic:\n};\n```", 40);
        let text = rendered
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");

        assert!(text.contains("cpp"));
        assert!(text.contains("class Solution"));
        assert!(!text.contains("```"));
    }

    #[test]
    fn wrapped_line_count_includes_visual_wraps() {
        let text = Text::from(Line::raw("0123456789abcdefghij"));
        assert_eq!(wrapped_line_count(&text, 10), 2);
    }
}

/// Parse inline markdown: **bold**, *italic*, `code` into ratatui Spans.
fn parse_inline_spans(text: &str) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut remaining = text;
    let normal = Style::default().fg(Color::White);

    while !remaining.is_empty() {
        // Bold: **text**
        if let Some(rest) = remaining.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                spans.push(Span::styled(
                    rest[..end].to_string(),
                    normal.add_modifier(Modifier::BOLD),
                ));
                remaining = &rest[end + 2..];
                continue;
            }
        }

        // Italic: *text* (but not **)
        if remaining.starts_with('*') && !remaining.starts_with("**") {
            let rest = &remaining[1..];
            if let Some(end) = rest.find('*') {
                spans.push(Span::styled(
                    rest[..end].to_string(),
                    normal.add_modifier(Modifier::ITALIC),
                ));
                remaining = &rest[end + 1..];
                continue;
            }
        }

        // Inline code: `text`
        if let Some(rest) = remaining.strip_prefix('`') {
            if let Some(end) = rest.find('`') {
                spans.push(Span::styled(
                    rest[..end].to_string(),
                    Style::default()
                        .fg(Color::Yellow)
                        .bg(Color::Rgb(30, 30, 40)),
                ));
                remaining = &rest[end + 1..];
                continue;
            }
        }

        // Regular text up to the next special char
        let next_special = remaining
            .find(|c| c == '*' || c == '`')
            .unwrap_or(remaining.len());
        if next_special > 0 {
            spans.push(Span::styled(remaining[..next_special].to_string(), normal));
            remaining = &remaining[next_special..];
        } else if let Some(ch) = remaining.chars().next() {
            spans.push(Span::styled(ch.to_string(), normal));
            remaining = &remaining[ch.len_utf8()..];
        } else {
            break;
        }
    }

    // If no spans were created, add the raw text
    if spans.is_empty() && !text.is_empty() {
        spans.push(Span::styled(text.to_string(), normal));
    }

    spans
}

// ── Helpers ─────────────────────────────────────────────────────────

fn truncate_str_w(s: &str, max_width: usize) -> String {
    use unicode_width::UnicodeWidthStr;
    if UnicodeWidthStr::width(s) <= max_width {
        return s.to_string();
    }
    let mut result = String::new();
    let mut w = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if w + cw + 3 > max_width {
            result.push_str("...");
            break;
        }
        w += cw;
        result.push(c);
    }
    result
}

fn buffer_row_to_line(buf: &Buffer, y: u16) -> Line<'static> {
    let mut spans = Vec::new();
    let mut current = String::new();
    let mut current_style: Option<Style> = None;

    for x in 0..buf.area.width {
        let Some(cell) = buf.cell((x, y)) else {
            continue;
        };
        let style = Style::default()
            .fg(cell.fg)
            .bg(cell.bg)
            .add_modifier(cell.modifier);
        if current_style.is_some_and(|s| s == style) {
            current.push_str(cell.symbol());
        } else {
            if !current.is_empty() {
                spans.push(Span::styled(
                    std::mem::take(&mut current),
                    current_style.unwrap(),
                ));
            }
            current_style = Some(style);
            current.push_str(cell.symbol());
        }
    }

    if !current.is_empty() {
        spans.push(Span::styled(current, current_style.unwrap_or_default()));
    }

    Line::from(spans)
}
