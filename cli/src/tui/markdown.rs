use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
};
use std::sync::LazyLock;
use syntect::easy::HighlightLines;
use syntect::highlighting::ThemeSet;
use syntect::parsing::SyntaxSet;
use syntect::util::LinesWithEndings;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CODE_BG: Color = Color::Rgb(22, 24, 29);
const INLINE_CODE_BG: Color = Color::Rgb(35, 35, 50);
const TOOL_COLOR: Color = Color::Rgb(86, 182, 194);

static SYNTAX_SET: LazyLock<SyntaxSet> = LazyLock::new(SyntaxSet::load_defaults_newlines);
static SYNTAX_THEME: LazyLock<syntect::highlighting::Theme> = LazyLock::new(|| {
    let ts = ThemeSet::load_defaults();
    ts.themes["base16-ocean.dark"].clone()
});

pub fn markdown_to_lines(text: &str, width: usize) -> Vec<Line<'static>> {
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

    // ── Table state ─────────────────────────────────────────────────
    let mut in_table_cell = false;
    let mut table_rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut header_row_count = 0; // how many rows are in <thead>

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
                // ── Table handling ──────────────────────────────────────
                Tag::Table(_) => {
                    flush_md_line(&mut cur_spans, &mut lines, false);
                    table_rows.clear();
                    header_row_count = 0;
                }
                Tag::TableHead => {}
                Tag::TableRow => {
                    current_row.clear();
                }
                Tag::TableCell => {
                    in_table_cell = true;
                    current_cell.clear();
                }
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
                // ── Table handling ──────────────────────────────────────
                TagEnd::TableCell => {
                    in_table_cell = false;
                    current_row.push(current_cell.trim().to_string());
                }
                TagEnd::TableRow => {
                    if !current_row.is_empty() {
                        table_rows.push(current_row.clone());
                    }
                }
                TagEnd::TableHead => {
                    header_row_count = table_rows.len();
                }
                TagEnd::Table => {
                    lines.extend(render_md_table(&table_rows, header_row_count, width));
                    lines.push(Line::raw(""));
                }
                _ => {}
            },
            Event::Text(text) => {
                if in_code_block {
                    code_content.push_str(&text);
                } else if in_table_cell {
                    current_cell.push_str(&text);
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
                if in_table_cell {
                    current_cell.push('`');
                    current_cell.push_str(&text);
                    current_cell.push('`');
                } else if !in_code_block {
                    cur_spans.push(Span::styled(
                        text.to_string(),
                        Style::default().fg(Color::Yellow).bg(INLINE_CODE_BG),
                    ));
                }
            }
            Event::SoftBreak => {
                if in_table_cell {
                    current_cell.push(' ');
                } else if !in_code_block {
                    cur_spans.push(Span::raw(" "));
                }
            }
            Event::HardBreak => {
                if !in_code_block && !in_table_cell {
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

/// Render a markdown table as aligned terminal text.
fn render_md_table(
    rows: &[Vec<String>],
    header_row_count: usize,
    width: usize,
) -> Vec<Line<'static>> {
    if rows.is_empty() {
        return Vec::new();
    }

    let col_count = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return Vec::new();
    }

    // Compute max width per column (using display width)
    let mut col_widths = vec![0usize; col_count];
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            let w = cell.width();
            if w > col_widths[i] {
                col_widths[i] = w;
            }
        }
    }

    // Row format: "│ " + cell1 + " │ " + cell2 + " │ " + ... + "│"
    // Total width = 2 (left "│ ") + sum(col_widths) + col_count * 3 (" │ ")
    let mut actual_total = 2 + col_widths.iter().sum::<usize>() + col_count * 3;

    // Shrink column widths proportionally if the table is too wide.
    if actual_total > width && col_count > 0 {
        let min_per_col = 4usize;
        let available_for_cells = width.saturating_sub(2 + col_count * 3);
        let total_needed: usize = col_widths.iter().sum();

        if total_needed > available_for_cells && available_for_cells > 0 {
            let scale = available_for_cells as f64 / total_needed as f64;
            for w in &mut col_widths {
                let scaled = ((*w as f64 * scale) as usize).max(min_per_col);
                *w = scaled.min(*w);
            }
        }

        // Iteratively shrink the widest columns until we fit.
        loop {
            actual_total = 2 + col_widths.iter().sum::<usize>() + col_count * 3;
            if actual_total <= width {
                break;
            }
            let max_idx = col_widths
                .iter()
                .enumerate()
                .max_by_key(|(_, w)| *w)
                .map(|(i, _)| i)
                .unwrap_or(0);
            if col_widths[max_idx] <= min_per_col {
                break;
            }
            col_widths[max_idx] -= 1;
        }
    }

    let border_fg = Color::Rgb(92, 99, 112);
    let mut out = Vec::new();

    // Separator line — must exactly match the rendered row width so that
    // ratatui doesn't wrap it on a different boundary than the row text.
    let sep = "─".repeat(actual_total);

    for (row_idx, row) in rows.iter().enumerate() {
        let is_header = row_idx < header_row_count;

        let mut spans: Vec<Span<'static>> = Vec::new();
        spans.push(Span::styled("│ ", Style::default().fg(border_fg)));

        for (col_idx, cell) in row.iter().enumerate() {
            let max_w = col_widths.get(col_idx).copied().unwrap_or(10);
            let padded = pad_cell(cell, max_w);
            let cell_style = if is_header {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            spans.push(Span::styled(padded, cell_style));
            spans.push(Span::styled(" │ ", Style::default().fg(border_fg)));
        }
        out.push(Line::from(spans));

        // Separator after header rows
        if is_header && row_idx + 1 == header_row_count {
            out.push(Line::from(Span::styled(
                sep.clone(),
                Style::default().fg(border_fg),
            )));
        }
    }

    out
}

/// Pad or truncate a cell to fit within max_width characters (display width).
fn pad_cell(text: &str, max_width: usize) -> String {
    let text_width = text.width();
    if text_width > max_width {
        // Truncate, leaving room for the ellipsis if possible.
        let mut w = 0usize;
        let mut chars = 0usize;
        for c in text.chars() {
            let cw = c.width().unwrap_or(0);
            if w + cw > max_width.saturating_sub(1) {
                break;
            }
            w += cw;
            chars += 1;
        }
        let mut s: String = text.chars().take(chars).collect();
        if w + 1 <= max_width {
            s.push('…');
        }
        s
    } else {
        // Pad with spaces on the right
        let pad = max_width - text_width;
        format!("{}{}", text, " ".repeat(pad))
    }
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
        HeadingLevel::H1 => Color::Rgb(97, 175, 239),  // blue
        HeadingLevel::H2 => Color::Rgb(86, 182, 194),  // cyan
        HeadingLevel::H3 => Color::Rgb(152, 195, 121), // green
        HeadingLevel::H4 => Color::Rgb(229, 192, 123), // yellow
        HeadingLevel::H5 => Color::Rgb(198, 120, 221), // magenta
        HeadingLevel::H6 => Color::Rgb(171, 178, 191), // gray
    }
}

// ── Code blocks (syntect) ──────────────────────────────────────────

pub fn render_syntect_block(language: &str, code: &str, width: usize) -> Vec<Line<'static>> {
    let syntax = find_syntax(language);
    let theme = &SYNTAX_THEME;
    let mut highlighter = HighlightLines::new(syntax, theme);
    let fill_width = width;

    let mut lines = Vec::new();
    for line in LinesWithEndings::from(code) {
        let line = line.trim_end_matches(['\n', '\r']);
        if let Ok(ranges) = highlighter.highlight_line(line, &SYNTAX_SET) {
            // Collect highlighted spans
            let mut spans: Vec<Span> = vec![Span::styled("  ", Style::default().bg(CODE_BG))];
            for (style, text) in ranges {
                let fg = syntect_color_to_ratatui(style.foreground);
                spans.push(Span::styled(
                    text.to_string(),
                    Style::default().fg(fg).bg(CODE_BG),
                ));
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
    let title = format!(
        " {}",
        if language.is_empty() {
            "code"
        } else {
            language
        }
    );
    let title_line = {
        let title_fill = fill_width.saturating_sub(title.width() + 4);
        vec![
            Span::styled(
                "  📋",
                Style::default()
                    .fg(Color::Yellow)
                    .bg(CODE_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                title,
                Style::default()
                    .fg(Color::Yellow)
                    .bg(CODE_BG)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" ".repeat(title_fill), Style::default().bg(CODE_BG)),
        ]
    };

    let sep_line = {
        let sep = "─".repeat(30.min(fill_width));
        let sep_fill = fill_width.saturating_sub(sep.width() + 4);
        vec![
            Span::styled("  ", Style::default().bg(CODE_BG)),
            Span::styled(
                sep,
                Style::default().fg(Color::Rgb(92, 99, 112)).bg(CODE_BG),
            ),
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
