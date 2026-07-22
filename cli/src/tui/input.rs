use super::state::{AppState, ModalState, APPROVAL_CHOICES};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

pub fn handle_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    if !matches!(state.modal, ModalState::None) {
        return handle_modal_key(key, state);
    }

    match key.code {
        KeyCode::Esc => {
            if state.autocomplete.active {
                state.autocomplete.active = false;
                return Ok(());
            }
            if state.subagent_view.is_some() {
                state.subagent_view = None;
                state.subagent_scroll = 0;
                state.mark_dirty();
                return Ok(());
            }
            if state.focused_block_id.is_some() {
                state.focused_block_id = None;
                return Ok(());
            }
            if state.agent_running {
                state.pending_abort = true;
                return Ok(());
            }
            if state.input.trim().is_empty() {
                state.modal = ModalState::QuitConfirm;
            }
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.agent_running {
                state.pending_abort = true;
            } else {
                state.modal = ModalState::QuitConfirm;
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.input.is_empty() {
                state.modal = ModalState::QuitConfirm;
            }
        }
        KeyCode::Char('l') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.mark_dirty_force();
        }
        KeyCode::Char('?') if state.input.is_empty() => {
            state.open_help();
        }
        KeyCode::Char('y')
            if state.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            if let Some(text) = state.yank_text() {
                state.pending_yank = Some(text);
                state.push_notice("Yanked to clipboard.");
            }
        }
        KeyCode::Char('t')
            if state.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            state.toggle_thought_expanded();
        }
        KeyCode::Char('p')
            if state.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            state.open_focused_or_last_pager();
        }
        KeyCode::Char('g')
            if state.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            // top
            let max = state.cache.wrapped_height.saturating_sub(state.viewport_h);
            state.scroll = max;
            state.paused = true;
        }
        KeyCode::Char('G')
            if state.input.is_empty() && !key.modifiers.contains(KeyModifiers::CONTROL) =>
        {
            state.scroll = 0;
            state.paused = false;
        }
        KeyCode::Char('[') if state.input.is_empty() => {
            state.focus_prev_subagent();
        }
        KeyCode::Char(']') if state.input.is_empty() => {
            state.focus_next_subagent();
        }
        KeyCode::Tab => {
            if state.autocomplete.active {
                if let Some((cmd, _)) = state
                    .autocomplete
                    .filtered
                    .get(state.autocomplete.selected_index)
                {
                    state.input = format!("{cmd} ");
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
                return Ok(());
            }
            if state.input.is_empty() {
                state.open_focused_subagent_detail();
            }
        }
        KeyCode::Enter => {
            if key.modifiers.contains(KeyModifiers::SHIFT)
                || key.modifiers.contains(KeyModifiers::CONTROL)
            {
                // newline
                state.history_index = None;
                state.input.insert(state.cursor_pos, '\n');
                state.cursor_pos += 1;
                state.update_autocomplete();
                return Ok(());
            }
            if state.autocomplete.active {
                if let Some((cmd, _)) = state
                    .autocomplete
                    .filtered
                    .get(state.autocomplete.selected_index)
                {
                    state.input = (*cmd).to_string();
                    state.cursor_pos = state.input.len();
                    state.autocomplete.active = false;
                }
            }
            let text = state.input.trim().to_string();
            if text.is_empty() {
                return Ok(());
            }
            state.push_input_history(text.clone());
            if text.starts_with('/') {
                state.pending_command = Some(text);
                state.input.clear();
                state.cursor_pos = 0;
            } else {
                state.submit(text);
                state.input.clear();
                state.cursor_pos = 0;
            }
        }
        KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.input.insert(state.cursor_pos, '\n');
            state.cursor_pos += 1;
        }
        KeyCode::PageUp => {
            let step = (state.viewport_h * 8 / 10).max(1);
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_add(step);
            } else {
                state.scroll = state.scroll.saturating_add(step);
                state.paused = true;
            }
        }
        KeyCode::PageDown => {
            let step = (state.viewport_h * 8 / 10).max(1);
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_sub(step);
            } else {
                state.scroll = state.scroll.saturating_sub(step);
                if state.scroll == 0 {
                    state.paused = false;
                }
            }
        }
        KeyCode::Up => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index > 0 {
                    state.autocomplete.selected_index -= 1;
                } else {
                    state.autocomplete.selected_index =
                        state.autocomplete.filtered.len().saturating_sub(1);
                }
                return Ok(());
            }
            // multiline: move within input first
            if let Some(prev_nl) = state.input[..state.cursor_pos].rfind('\n') {
                let col = state.cursor_pos - prev_nl - 1;
                let line_start = state.input[..prev_nl].rfind('\n').map(|i| i + 1).unwrap_or(0);
                let prev_len = prev_nl - line_start;
                state.cursor_pos = line_start + col.min(prev_len);
            } else {
                state.history_up();
            }
        }
        KeyCode::Down => {
            if state.autocomplete.active {
                if state.autocomplete.selected_index + 1 < state.autocomplete.filtered.len() {
                    state.autocomplete.selected_index += 1;
                } else {
                    state.autocomplete.selected_index = 0;
                }
                return Ok(());
            }
            let after = &state.input[state.cursor_pos..];
            if let Some(rel) = after.find('\n') {
                let col = state.input[..state.cursor_pos]
                    .rsplit('\n')
                    .next()
                    .map(|s| s.len())
                    .unwrap_or(0);
                let next_start = state.cursor_pos + rel + 1;
                let next_line_end = state.input[next_start..]
                    .find('\n')
                    .map(|i| next_start + i)
                    .unwrap_or(state.input.len());
                let next_len = next_line_end - next_start;
                state.cursor_pos = next_start + col.min(next_len);
            } else {
                state.history_down();
            }
        }
        KeyCode::Left => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.cursor_pos = prev_word_boundary(&state.input, state.cursor_pos);
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                state.focus_prev_block();
            } else {
                state.cursor_pos = prev_char_boundary(&state.input, state.cursor_pos);
            }
        }
        KeyCode::Right => {
            if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.cursor_pos = next_word_boundary(&state.input, state.cursor_pos);
            } else if key.modifiers.contains(KeyModifiers::ALT) {
                state.focus_next_block();
            } else {
                state.cursor_pos = next_char_boundary(&state.input, state.cursor_pos);
            }
        }
        KeyCode::Home => {
            if key.modifiers.contains(KeyModifiers::ALT) {
                let max = state.cache.wrapped_height.saturating_sub(state.viewport_h);
                state.scroll = max;
                state.paused = true;
            } else {
                // line start
                let line_start = state.input[..state.cursor_pos]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                state.cursor_pos = line_start;
            }
        }
        KeyCode::End => {
            if key.modifiers.contains(KeyModifiers::ALT) {
                state.scroll = 0;
                state.paused = false;
            } else if key.modifiers.contains(KeyModifiers::CONTROL) {
                state.scroll = 0;
                state.paused = false;
            } else {
                let line_end = state.input[state.cursor_pos..]
                    .find('\n')
                    .map(|i| state.cursor_pos + i)
                    .unwrap_or(state.input.len());
                state.cursor_pos = line_end;
            }
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = 0;
        }
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.cursor_pos = state.input.len();
        }
        KeyCode::Backspace => {
            state.history_index = None;
            if key.modifiers.contains(KeyModifiers::CONTROL) {
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
            state.history_index = None;
            if state.cursor_pos < state.input.len() {
                let next = next_char_boundary(&state.input, state.cursor_pos);
                state.input.replace_range(state.cursor_pos..next, "");
                state.update_autocomplete();
            }
        }
        KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.history_index = None;
            state.input.truncate(state.cursor_pos);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.history_index = None;
            state.input.replace_range(0..state.cursor_pos, "");
            state.cursor_pos = 0;
        }
        KeyCode::Char('w') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.history_index = None;
            let start = prev_word_boundary(&state.input, state.cursor_pos);
            state.input.replace_range(start..state.cursor_pos, "");
            state.cursor_pos = start;
        }
        KeyCode::Char(c) => {
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

fn handle_modal_key(key: KeyEvent, state: &mut AppState) -> Result<()> {
    match &mut state.modal {
        ModalState::QuitConfirm => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                state.should_quit = true;
            }
            KeyCode::Char('n') | KeyCode::Char('N') => state.modal = ModalState::None,
            _ => {}
        },
        ModalState::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('?')) {
                state.modal = ModalState::None;
            }
        }
        ModalState::Pager { scroll, lines, .. } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::PageUp | KeyCode::Up => *scroll = scroll.saturating_sub(5),
            KeyCode::PageDown | KeyCode::Down => {
                *scroll = (*scroll + 5).min(lines.len().saturating_sub(1));
            }
            _ => {}
        },
        ModalState::Approval {
            selected,
            prompt_id,
            ..
        } => match key.code {
            KeyCode::Esc => {
                let id = prompt_id.clone();
                state.pending_approval = Some((id, "deny".into()));
                state.modal = ModalState::None;
            }
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                if *selected + 1 < APPROVAL_CHOICES.len() {
                    *selected += 1;
                }
            }
            KeyCode::Char(c @ '1'..='5') => {
                let idx = (c as u8 - b'1') as usize;
                *selected = idx;
                let id = prompt_id.clone();
                let key = APPROVAL_CHOICES[idx].0.to_string();
                state.pending_approval = Some((id, key));
                state.modal = ModalState::None;
            }
            KeyCode::Enter => {
                let idx = (*selected).min(APPROVAL_CHOICES.len() - 1);
                let id = prompt_id.clone();
                let key = APPROVAL_CHOICES[idx].0.to_string();
                state.pending_approval = Some((id, key));
                state.modal = ModalState::None;
            }
            _ => {}
        },
        ModalState::Answer {
            prompt_id,
            input,
            cursor,
            ..
        } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Enter => {
                let id = prompt_id.clone();
                let ans = input.clone();
                state.pending_answer = Some((id, ans));
                state.modal = ModalState::None;
            }
            KeyCode::Backspace => {
                if *cursor > 0 {
                    let prev = prev_char_boundary(input, *cursor);
                    input.remove(prev);
                    *cursor = prev;
                }
            }
            KeyCode::Char(c) => {
                input.insert(*cursor, c);
                *cursor += c.len_utf8();
            }
            _ => {}
        },
        ModalState::ModelPicker {
            models,
            selected,
            filter,
            ..
        } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                let n = models
                    .iter()
                    .filter(|m| filter.is_empty() || m.contains(filter.as_str()))
                    .count();
                if *selected + 1 < n {
                    *selected += 1;
                }
            }
            KeyCode::Backspace => {
                filter.pop();
                *selected = 0;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                filter.push(c);
                *selected = 0;
            }
            KeyCode::Enter => {
                let filtered: Vec<String> = models
                    .iter()
                    .filter(|m| filter.is_empty() || m.contains(filter.as_str()))
                    .cloned()
                    .collect();
                if let Some(name) = filtered.get(*selected) {
                    state.pending_command = Some(format!("/model {name}"));
                    state.modal = ModalState::None;
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
            cursor,
            ..
        } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *active_field = active_field.saturating_sub(1),
            KeyCode::Down | KeyCode::Tab => *active_field = (*active_field + 1).min(3),
            KeyCode::Left => *cursor = cursor.saturating_sub(1),
            KeyCode::Right => {
                let field = match *active_field {
                    0 => provider.len(),
                    1 => base_url.len(),
                    2 => api_key.len(),
                    _ => model_id.len(),
                };
                *cursor = (*cursor + 1).min(field);
            }
            KeyCode::Enter => {
                let cmd = format!(
                    "register_model:{}|{}|{}|{}",
                    provider.trim(),
                    base_url.trim(),
                    api_key.trim(),
                    model_id.trim()
                );
                state.pending_command = Some(cmd);
                state.modal = ModalState::None;
            }
            KeyCode::Backspace => {
                let field: &mut String = match *active_field {
                    0 => provider,
                    1 => base_url,
                    2 => api_key,
                    _ => model_id,
                };
                if *cursor > 0 && *cursor <= field.len() {
                    let prev = prev_char_boundary(field, *cursor);
                    field.remove(prev);
                    *cursor = prev;
                }
            }
            KeyCode::Char(c) => {
                let field: &mut String = match *active_field {
                    0 => provider,
                    1 => base_url,
                    2 => api_key,
                    _ => model_id,
                };
                field.insert(*cursor, c);
                *cursor += c.len_utf8();
            }
            _ => {}
        },
        ModalState::SessionList {
            sessions,
            selected,
            filter: _,
        } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                if *selected + 1 < sessions.len() {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some((id, _)) = sessions.get(*selected) {
                    state.pending_command = Some(format!("/session resume {id}"));
                    state.modal = ModalState::None;
                }
            }
            _ => {}
        },
        ModalState::RewindList { points, selected } => match key.code {
            KeyCode::Esc => state.modal = ModalState::None,
            KeyCode::Up => *selected = selected.saturating_sub(1),
            KeyCode::Down => {
                if *selected + 1 < points.len() {
                    *selected += 1;
                }
            }
            KeyCode::Enter => {
                if let Some((idx, _)) = points.get(*selected) {
                    state.pending_command = Some(format!("/rewind {idx}"));
                    state.modal = ModalState::None;
                }
            }
            _ => {}
        },
        ModalState::None => {}
    }
    Ok(())
}

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
    let trimmed_end = s.trim_end();
    let ws_len = s.len() - trimmed_end.len();
    let non_ws_end = pos.saturating_sub(ws_len);
    text[..non_ws_end]
        .rfind(char::is_whitespace)
        .map_or(0, |i| i + 1)
}

fn next_word_boundary(text: &str, pos: usize) -> usize {
    let s = &text[pos..];
    let trimmed_start = s.trim_start();
    let ws_len = s.len() - trimmed_start.len();
    let non_ws_start = pos + ws_len;
    text[non_ws_start..]
        .find(char::is_whitespace)
        .map_or(text.len(), |i| non_ws_start + i)
}
