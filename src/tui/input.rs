use super::state::{AppState, Entry, TurnBlock};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    match key.code {
        // ── Quit ──────────────────────────────────────────────────
        KeyCode::Esc => {
            state.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }

        // ── Submit ────────────────────────────────────────────────
        KeyCode::Enter => {
            // Check if a subagent block is focused — toggle it
            if let Some(idx) = state.focus_index {
                toggle_focus(state, idx);
                return Ok(());
            }

            // Submit input
            let text = state.input.trim().to_string();
            if !text.is_empty() {
                state.submit(text);
                state.input.clear();
                state.cursor_pos = 0;
            }
        }

        // ── Scroll ────────────────────────────────────────────────
        KeyCode::PageUp => {
            state.scroll = state.scroll.saturating_add(5);
        }
        KeyCode::PageDown => {
            state.scroll = state.scroll.saturating_sub(5);
        }
        KeyCode::Up => {
            if state.focus_index.is_some() {
                let idx = state.focus_index.unwrap();
                if idx > 0 {
                    state.focus_index = Some(idx - 1);
                }
            } else {
                state.scroll = state.scroll.saturating_add(1);
            }
        }
        KeyCode::Down => {
            if state.focus_index.is_some() {
                let idx = state.focus_index.unwrap();
                state.focus_index = Some(idx + 1);
            } else {
                state.scroll = state.scroll.saturating_sub(1);
            }
        }

        // ── Cursor movement ───────────────────────────────────────
        KeyCode::Left => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.cursor_pos = prev_word_boundary(&state.input, state.cursor_pos);
            } else {
                state.cursor_pos = prev_char_boundary(&state.input, state.cursor_pos);
            }
        }
        KeyCode::Right => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.cursor_pos = next_word_boundary(&state.input, state.cursor_pos);
            } else {
                state.cursor_pos = next_char_boundary(&state.input, state.cursor_pos);
            }
        }
        KeyCode::Home | KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = 0;
        }
        KeyCode::End | KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = state.input.len();
        }

        // ── Deletion ──────────────────────────────────────────────
        KeyCode::Backspace => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+Backspace: delete previous word
                let start = prev_word_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(start..state.cursor_pos, "");
                state.cursor_pos = start;
            } else if state.cursor_pos > 0 {
                let prev = prev_char_boundary(&state.input, state.cursor_pos);
                state.input.remove(prev);
                state.cursor_pos = prev;
            }
        }
        KeyCode::Delete => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+Delete: delete next word
                let end = next_word_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..end, "");
            } else if state.cursor_pos < state.input.len() {
                let next = next_char_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..next, "");
            }
        }

        // ── Kill line / word ──────────────────────────────────────
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill to end of line
            if state.cursor_pos < state.input.len() {
                state.input.truncate(state.cursor_pos);
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill to start of line
            if state.cursor_pos > 0 {
                state.input.replace_range(0..state.cursor_pos, "");
                state.cursor_pos = 0;
            }
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill previous word
            let start = prev_word_boundary(&state.input, state.cursor_pos);
            state.input.replace_range(start..state.cursor_pos, "");
            state.cursor_pos = start;
        }

        // ── Character input ───────────────────────────────────────
        KeyCode::Char(c) => {
            state.input.insert(state.cursor_pos, c);
            state.cursor_pos += c.len_utf8();
        }

        _ => {}
    }

    Ok(())
}

/// Toggle a subagent block at the given focus index.
fn toggle_focus(state: &mut AppState, idx: usize) {
    let mut count = 0;

    if let Some(ref mut streaming) = state.streaming {
        for block in &mut streaming.blocks {
            if let TurnBlock::Subagent(sa) = block {
                if count == idx {
                    sa.collapsed = !sa.collapsed;
                    return;
                }
                count += 1;
            }
        }
    }

    for entry in &mut state.entries {
        if let Entry::Turn { blocks, .. } = entry {
            for block in blocks {
                if let TurnBlock::Subagent(sa) = block {
                    if count == idx {
                        sa.collapsed = !sa.collapsed;
                        return;
                    }
                    count += 1;
                }
            }
        }
    }
}

// ── Character boundary helpers ──────────────────────────────────────

fn prev_char_boundary(text: &str, pos: usize) -> usize {
    text[..pos]
        .char_indices()
        .last()
        .map_or(0, |(idx, _)| idx)
}

fn next_char_boundary(text: &str, pos: usize) -> usize {
    text[pos..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(idx, _)| pos + idx)
}

fn prev_word_boundary(text: &str, pos: usize) -> usize {
    let s = &text[..pos];
    // Skip trailing whitespace
    let trimmed_end = s.trim_end();
    let ws_len = s.len() - trimmed_end.len();
    let non_ws_end = pos.saturating_sub(ws_len);
    // Find previous whitespace boundary
    text[..non_ws_end]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1)
}

fn next_word_boundary(text: &str, pos: usize) -> usize {
    let s = &text[pos..];
    // Skip leading whitespace
    let trimmed_start = s.trim_start();
    let ws_len = s.len() - trimmed_start.len();
    let non_ws_start = pos + ws_len;
    // Find next whitespace
    text[non_ws_start..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| non_ws_start + i)
}
