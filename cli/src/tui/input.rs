use super::state::{AppState, CommandMode, Entry, ModalState, TurnBlock};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    // ── Modal navigation ──────────────────────────────────────────
    if !matches!(state.modal, ModalState::None) {
        return handle_modal_key(key, state);
    }

    match key.code {
        // ── Quit / Back ───────────────────────────────────────────
        KeyCode::Esc => {
            if state.autocomplete.active {
                state.autocomplete.active = false;
                return Ok(());
            }
            if state.subagent_view.is_some() {
                // Exit subagent detail view back to main conversation
                state.subagent_view = None;
                state.subagent_scroll = 0;
                state.mark_dirty();
                return Ok(());
            }
            if !matches!(state.command_mode, CommandMode::None) {
                state.cancel_command();
                state.input.clear();
                state.cursor_pos = 0;
                return Ok(());
            }
            if matches!(state.modal, ModalState::None) && state.agent_running {
                state.pending_command = Some("abort".to_string());
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
                    let selected =
                        &state.autocomplete.filtered_options[state.autocomplete.selected_index];
                    state.input = format!("{} ", selected);
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
                return Ok(());
            }
        }
        KeyCode::Enter => {
            // If autocomplete dropdown is active, execute the selected option directly
            if state.autocomplete.active {
                if state.autocomplete.selected_index < state.autocomplete.filtered_options.len() {
                    let selected = &state.autocomplete.filtered_options[state.autocomplete.selected_index];
                    state.input = selected.clone();
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
            }

            let text = state.input.trim().to_string();
            if text.is_empty() {
                return Ok(());
            }

            // Record in input history regardless of whether it's a command or message
            state.push_input_history(text.clone());

            // Handle slash commands with optional modal popup
            if text == "/models" || text == "/model" {
                state.pending_command = Some("show_model_picker".to_string());
                state.input.clear();
                state.cursor_pos = 0;
                return Ok(());
            }
            if text.starts_with("/model ") {
                let name = text.strip_prefix("/model ").unwrap().trim().to_string();
                state.pending_command = Some(format!("switch_model:{}", name));
                state.input.clear();
                state.cursor_pos = 0;
                return Ok(());
            }
            if text == "/models new" {
                state.pending_command = Some("show_model_form".to_string());
                state.input.clear();
                state.cursor_pos = 0;
                return Ok(());
            }

            // Dispatch other commands
            if text.starts_with('/') || !matches!(state.command_mode, CommandMode::None) {
                let notice = state.handle_command(&text);
                state.input.clear();
                state.cursor_pos = 0;

                if let Some(msg) = notice {
                    let notice_block = TurnBlock::Notice(msg);
                    if let Some(ref mut s) = state.streaming {
                        s.blocks.insert(0, notice_block);
                    } else {
                        state.entries.push(Entry::Turn {
                            turn: 0,
                            blocks: vec![notice_block],
                        });
                    }
                    state.mark_dirty();
                }
            } else {
                state.submit(text);
                state.input.clear();
                state.cursor_pos = 0;
            }
        }
        KeyCode::PageUp => {
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_add(5);
            } else {
                state.scroll = state.scroll.saturating_add(5);
            }
        }
        KeyCode::PageDown => {
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_sub(5);
            } else {
                state.scroll = state.scroll.saturating_sub(5);
            }
        }
        KeyCode::Up => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index > 0 {
                    state.autocomplete.selected_index -= 1;
                } else {
                    state.autocomplete.selected_index =
                        state.autocomplete.filtered_options.len().saturating_sub(1);
                }
                return Ok(());
            }
            // Navigate command history (Up = older)
            state.history_up();
        }
        KeyCode::Down => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index + 1 < state.autocomplete.filtered_options.len()
                {
                    state.autocomplete.selected_index += 1;
                } else {
                    state.autocomplete.selected_index = 0;
                }
                return Ok(());
            }
            // Navigate command history (Down = newer)
            state.history_down();
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
            // Exit history navigation on editing
            state.history_index = None;
            state.input_snapshot.clear();
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
            state.update_autocomplete();
        }
        KeyCode::Delete => {
            // Exit history navigation on editing
            state.history_index = None;
            state.input_snapshot.clear();
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                // Ctrl+Delete: delete next word
                let end = next_word_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..end, "");
            } else if state.cursor_pos < state.input.len() {
                let next = next_char_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..next, "");
                state.update_autocomplete();
            }
        }

        // ── Kill line / word ──────────────────────────────────────
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill to end of line — exit history nav
            state.history_index = None;
            state.input_snapshot.clear();
            if state.cursor_pos < state.input.len() {
                state.input.truncate(state.cursor_pos);
            }
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill to start of line — exit history nav
            state.history_index = None;
            state.input_snapshot.clear();
            if state.cursor_pos > 0 {
                state.input.replace_range(0..state.cursor_pos, "");
                state.cursor_pos = 0;
            }
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            // Kill previous word — exit history nav
            state.history_index = None;
            state.input_snapshot.clear();
            let start = prev_word_boundary(&state.input, state.cursor_pos);
            state.input.replace_range(start..state.cursor_pos, "");
            state.cursor_pos = start;
        }

        // ── Character input ───────────────────────────────────────
        KeyCode::Char(c) => {
            // Exit history navigation on any new typing
            state.history_index = None;
            state.input_snapshot.clear();
            state.input.insert(state.cursor_pos, c);
            state.cursor_pos += c.len_utf8();
            state.update_autocomplete();
        }

        _ => {}
    }

    Ok(())
}

// ── Character boundary helpers ──────────────────────────────────────

fn prev_char_boundary(text: &str, pos: usize) -> usize {
    text[..pos].char_indices().last().map_or(0, |(idx, _)| idx)
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

// ── Modal key handler ───────────────────────────────────────────────

fn handle_modal_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    match &mut state.modal {
        ModalState::ModelPicker { models, selected } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                if *selected + 1 < models.len() {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if *selected < models.len() {
                    let name = models[*selected].clone();
                    state.modal = ModalState::None;
                    state.pending_command = Some(format!("switch_model:{}", name));
                }
            }
            _ => {}
        },
        ModalState::ModelForm {
            provider,
            base_url,
            api_key,
            model_id,
            active_field,
        } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *active_field = active_field.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => *active_field = (*active_field + 1).min(3),
            KeyCode::Enter => {
                let cmd = format!(
                    "register_model:{}|{}|{}|{}",
                    provider.trim(),
                    base_url.trim(),
                    api_key.trim(),
                    model_id.trim()
                );
                state.modal = ModalState::None;
                state.pending_command = Some(cmd);
            }
            KeyCode::Backspace => {
                let field: &mut String = match *active_field {
                    0 => provider,
                    1 => base_url,
                    2 => api_key,
                    _ => model_id,
                };
                field.pop();
            }
            KeyCode::Char(c) => {
                let field: &mut String = match *active_field {
                    0 => provider,
                    1 => base_url,
                    2 => api_key,
                    _ => model_id,
                };
                field.push(c);
            }
            _ => {}
        },
        ModalState::None => {}
    }
    Ok(())
}
