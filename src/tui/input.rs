use super::state::{AppState, TurnBlock};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    match key.code {
        // ── Quit ──────────────────────────────────────────────────
        KeyCode::Esc => {
            if state.autocomplete.active {
                state.autocomplete.active = false;
                return Ok(());
            }
            state.should_quit = true;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.should_quit = true;
        }

        // ── Submit ────────────────────────────────────────────────
        KeyCode::Tab => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index < state.autocomplete.filtered_options.len() {
                    let selected = &state.autocomplete.filtered_options[state.autocomplete.selected_index];
                    state.input = format!("{} ", selected);
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
                return Ok(());
            }
        }
        KeyCode::Enter => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index < state.autocomplete.filtered_options.len() {
                    let selected = &state.autocomplete.filtered_options[state.autocomplete.selected_index];
                    state.input = format!("{} ", selected);
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
                return Ok(());
            }

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
            if state.autocomplete.active {
                if state.autocomplete.selected_index > 0 {
                    state.autocomplete.selected_index -= 1;
                } else {
                    state.autocomplete.selected_index = state.autocomplete.filtered_options.len().saturating_sub(1);
                }
                return Ok(());
            }
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
            if state.autocomplete.active {
                if state.autocomplete.selected_index + 1 < state.autocomplete.filtered_options.len() {
                    state.autocomplete.selected_index += 1;
                } else {
                    state.autocomplete.selected_index = 0;
                }
                return Ok(());
            }
            if state.focus_index.is_some() {
                let idx = state.focus_index.unwrap();
                state.focus_index = Some(idx + 1);
            } else {
                state.scroll = state.scroll.saturating_sub(1);
            }
        }

        // ── Input editing ─────────────────────────────────────────
        KeyCode::Char(c) => {
            state.input.insert(state.cursor_pos, c);
            state.cursor_pos += c.len_utf8();
            state.update_autocomplete();
        }
        KeyCode::Backspace => {
            if state.cursor_pos > 0 {
                state.cursor_pos = prev_char_boundary(&state.input, state.cursor_pos);
                state.input.remove(state.cursor_pos);
                state.update_autocomplete();
            }
        }
        KeyCode::Delete => {
            if state.cursor_pos < state.input.len() {
                let next = next_char_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..next, "");
                state.update_autocomplete();
            }
        }
        KeyCode::Left => {
            state.cursor_pos = prev_char_boundary(&state.input, state.cursor_pos);
        }
        KeyCode::Right => {
            state.cursor_pos = next_char_boundary(&state.input, state.cursor_pos);
        }
        KeyCode::Home => {
            state.cursor_pos = 0;
        }
        KeyCode::End => {
            state.cursor_pos = state.input.len();
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

use super::state::Entry;

fn prev_char_boundary(text: &str, pos: usize) -> usize {
    text[..pos].char_indices().last().map_or(0, |(idx, _)| idx)
}

fn next_char_boundary(text: &str, pos: usize) -> usize {
    text[pos..]
        .char_indices()
        .nth(1)
        .map_or(text.len(), |(idx, _)| pos + idx)
}
