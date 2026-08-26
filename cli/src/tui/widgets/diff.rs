use ratatui::{
    style::{Color, Style},
    text::{Line, Span},
};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const CODE_BG: Color = Color::Rgb(22, 24, 29);

/// Render a unified diff string with side-by-side line numbers, colour-coded
/// backgrounds, and an additions/deletions footer.
pub fn render_diff_output(
    diff_text: &str,
    lines: &mut Vec<Line<'static>>,
    width: usize,
    pad: &str,
) {
    let inner = width.saturating_sub(4 + pad.width());
    let prefix_w = 6; // "    5 │"
    let sign_w = 1;
    let content_width = inner.saturating_sub(prefix_w + sign_w).max(1);
    let empty_prefix = "     │"; // same width as "    5 │"

    let mut old_line = 0usize;
    let mut new_line = 0usize;
    let mut additions = 0usize;
    let mut deletions = 0usize;
    let mut pending_context: Vec<(String, String)> = Vec::new();

    for raw in diff_text.lines() {
        // --- / +++ file headers
        if raw.starts_with("--- ") {
            flush_context(
                &mut pending_context,
                lines,
                pad,
                inner,
                empty_prefix,
                content_width,
            );
            let path = raw.strip_prefix("--- ").unwrap_or(raw);
            lines.push(diff_line(
                pad,
                &format!("--- {}", path),
                inner,
                Style::default().fg(Color::Red).bg(CODE_BG),
            ));
            continue;
        }
        if raw.starts_with("+++ ") {
            flush_context(
                &mut pending_context,
                lines,
                pad,
                inner,
                empty_prefix,
                content_width,
            );
            let path = raw.strip_prefix("+++ ").unwrap_or(raw);
            lines.push(diff_line(
                pad,
                &format!("+++ {}", path),
                inner,
                Style::default().fg(Color::Green).bg(CODE_BG),
            ));
            continue;
        }
        // @@ hunk header
        if raw.starts_with("@@") {
            flush_context(
                &mut pending_context,
                lines,
                pad,
                inner,
                empty_prefix,
                content_width,
            );
            if let Some((o, n)) = parse_hunk_header(raw) {
                old_line = o;
                new_line = n;
            }
            lines.push(diff_line(
                pad,
                raw,
                inner,
                Style::default().fg(Color::DarkGray).bg(CODE_BG),
            ));
            continue;
        }

        if raw.is_empty() {
            continue;
        }

        let sign = &raw[..1];
        let content = &raw[1..];

        match sign {
            "-" => {
                flush_context(
                    &mut pending_context,
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                );
                deletions += 1;
                let num = format!("{:>4}", old_line);
                let style = Style::default().fg(Color::White).bg(Color::Rgb(60, 30, 30));
                push_wrapped_diff_line(
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                    &num,
                    sign,
                    content,
                    style,
                );
                old_line += 1;
            }
            "+" => {
                flush_context(
                    &mut pending_context,
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                );
                additions += 1;
                let num = format!("{:>4}", new_line);
                let style = Style::default().fg(Color::White).bg(Color::Rgb(30, 60, 30));
                push_wrapped_diff_line(
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                    &num,
                    sign,
                    content,
                    style,
                );
                new_line += 1;
            }
            " " => {
                let num = format!("{:>4}", old_line);
                pending_context.push((num, content.to_string()));
                old_line += 1;
                new_line += 1;
            }
            _ => {
                flush_context(
                    &mut pending_context,
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                );
                lines.push(diff_line(
                    pad,
                    raw,
                    inner,
                    Style::default().fg(Color::White).bg(CODE_BG),
                ));
            }
        }
    }

    flush_context(
        &mut pending_context,
        lines,
        pad,
        inner,
        empty_prefix,
        content_width,
    );

    // Stats footer
    if additions > 0 || deletions > 0 {
        let stats = format!("  +{} additions, -{} deletions", additions, deletions);
        lines.push(Line::from(vec![
            Span::raw(pad.to_string()),
            Span::styled("  ", Style::default().bg(CODE_BG)),
            Span::styled(stats, Style::default().fg(Color::DarkGray).bg(CODE_BG)),
        ]));
    }
}

/// Flush accumulated context lines, folding large runs into "... N lines ...".
fn flush_context(
    pending: &mut Vec<(String, String)>,
    lines: &mut Vec<Line<'static>>,
    pad: &str,
    inner: usize,
    empty_prefix: &str,
    content_width: usize,
) {
    if pending.is_empty() {
        return;
    }
    const MAX_CTX: usize = 3;
    let total = pending.len();
    let style = Style::default().fg(Color::White).bg(CODE_BG);
    let sign = " ";

    if total <= MAX_CTX * 2 {
        for (num, content) in pending.drain(..) {
            push_wrapped_diff_line(
                lines,
                pad,
                inner,
                empty_prefix,
                content_width,
                &num,
                sign,
                &content,
                style,
            );
        }
    } else {
        for (idx, (num, content)) in pending.drain(..).enumerate() {
            if idx < MAX_CTX || idx >= total - MAX_CTX {
                push_wrapped_diff_line(
                    lines,
                    pad,
                    inner,
                    empty_prefix,
                    content_width,
                    &num,
                    sign,
                    &content,
                    style,
                );
            } else if idx == MAX_CTX {
                let folded = format!("  ... {} unchanged lines ...", total - MAX_CTX * 2);
                lines.push(diff_line(
                    pad,
                    &folded,
                    inner,
                    Style::default().fg(Color::DarkGray).bg(CODE_BG),
                ));
            }
        }
    }
}

/// Push a diff line (possibly wrapped) into the output buffer.
fn push_wrapped_diff_line(
    lines: &mut Vec<Line<'static>>,
    pad: &str,
    inner: usize,
    empty_prefix: &str,
    content_width: usize,
    num_prefix: &str,
    sign: &str,
    content: &str,
    style: Style,
) {
    let wrapped = wrap_diff_content(content, content_width);
    for (i, part) in wrapped.iter().enumerate() {
        let prefix = if i == 0 {
            format!("{} │", num_prefix)
        } else {
            empty_prefix.to_string()
        };
        let text = format!("{}{}{}", prefix, sign, part);
        lines.push(diff_line(pad, &text, inner, style));
    }
}

/// Wrap diff content text into chunks that fit within max_width.
/// Tries to break at word boundaries first, falls back to character boundary.
fn wrap_diff_content(text: &str, max_width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }
    if max_width == 0 {
        return text.chars().map(|c| c.to_string()).collect();
    }

    let mut out = Vec::new();
    let mut cur = String::new();
    let mut cur_w = 0usize;

    for word in text.split_inclusive(' ') {
        let word_w = word.width();
        if word_w > max_width {
            // Word itself is too long — flush current first, then break the word
            if !cur.is_empty() {
                out.push(cur);
                cur = String::new();
                cur_w = 0;
            }
            let mut chunk = String::new();
            let mut chunk_w = 0usize;
            for ch in word.chars() {
                let cw = ch.width().unwrap_or(0);
                if chunk_w + cw > max_width && !chunk.is_empty() {
                    out.push(chunk);
                    chunk = String::new();
                    chunk_w = 0;
                }
                chunk.push(ch);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                cur = chunk;
                cur_w = chunk_w;
            }
        } else if cur_w + word_w > max_width && !cur.is_empty() {
            out.push(cur);
            cur = word.to_string();
            cur_w = word_w;
        } else {
            cur.push_str(word);
            cur_w += word_w;
        }
    }

    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Build a single diff line with consistent padding and fill.
fn diff_line(pad: &str, text: &str, inner: usize, style: Style) -> Line<'static> {
    let tw = text.width();
    let fill = inner.saturating_sub(tw);
    let bg = style.bg.unwrap_or(CODE_BG);
    Line::from(vec![
        Span::raw(pad.to_string()),
        Span::styled("  ", Style::default().bg(bg)),
        Span::styled(text.to_string(), style),
        Span::styled(" ".repeat(fill), Style::default().bg(bg)),
        Span::styled("  ", Style::default().bg(bg)),
    ])
}

/// Parse a unified-diff hunk header like `@@ -12,5 +13,5 @@`.
fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let inner = line.trim_start_matches("@@").trim_end_matches("@@").trim();
    let parts: Vec<&str> = inner.split_whitespace().collect();
    if parts.len() < 2 {
        return None;
    }
    let old_start: usize = parts[0]
        .trim_start_matches('-')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    let new_start: usize = parts[1]
        .trim_start_matches('+')
        .split(',')
        .next()?
        .parse()
        .ok()?;
    Some((old_start, new_start))
}
