pub mod input;
pub mod render;
pub mod state;

use agent_core::Agent;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind, MouseButton, MouseEvent, MouseEventKind};
use state::{AppState, Entry, EventPump, TurnBlock};
use std::sync::Arc;
use std::time::{Duration, Instant};

pub async fn run_tui(agent: Arc<tokio::sync::Mutex<Agent>>) -> Result<()> {
    // Enable mouse capture so crossterm can receive scroll wheel events
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    // Also enable mouse motion tracking (hover) — CSI 1003h
    // This lets us detect when the mouse hovers over subagent boxes.
    {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        write!(stdout, "\x1B[?1003h")?;
        stdout.flush()?;
    }
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, agent).await;
    ratatui::restore();
    // Disable mouse motion tracking and capture on cleanup
    {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        write!(stdout, "\x1B[?1003l")?;
        stdout.flush()?;
    }
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    result
}

async fn run_app(
    terminal: &mut ratatui::DefaultTerminal,
    agent: Arc<tokio::sync::Mutex<Agent>>,
) -> Result<()> {
    let mut state = AppState::new();

    // Populate status info + share approvals Arc with state
    {
        let a = agent.lock().await;
        state.model = a.current_model().to_string();
        state.tool_mode = format!("{:?}", a.tool_execution_mode());
        state.pending_approvals = Some(a.pending_approvals_clone());
    }
    let mut pump = EventPump::new();
    let mut last_draw = Instant::now();
    let min_frame = Duration::from_millis(16); // ~60 fps cap while streaming/input is active
    let mut needs_draw = true;
    loop {
        // ── 1. Drain agent events (non-blocking) ────────────────────
        let had_events = pump.drain(&mut state);

        // Refresh token count from agent (non-blocking)
        if had_events {
            if let Ok(a) = agent.try_lock() {
                state.tokens = a.context_token_count();
            }
        }

        // ── 2. Handle ALL pending input events before rendering ─────
        // This is critical: we must drain all queued key/mouse events
        // before any render call, otherwise input appears unresponsive
        // when rendering is slow (during streaming rebuilds).
        let mut had_input = false;
        loop {
            if event::poll(Duration::from_millis(0))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        input::handle_key(key, &mut state)?;
                        had_input = true;
                    }
                    Event::Mouse(mouse) => {
                        handle_mouse(mouse, &mut state);
                        had_input = true;
                    }
                    Event::Resize(_, _) => {
                        state.cache_dirty = true;
                        had_input = true;
                    }
                    _ => {}
                }
            } else {
                break; // No more pending input
            }
        }

        needs_draw |= had_events || had_input || state.cache_dirty;
        if state.should_quit {
            break Ok(());
        }

        // ── 3. Process pending slash commands ───────────────────────
        if let Some(cmd) = state.take_pending_command() {
            needs_draw = true;
            if cmd == "show_model_picker" {
                let a = agent.lock().await;
                let models: Vec<String> = a.list_models().into_iter().map(|(n, _)| n.to_string()).collect();
                state.open_model_picker(models);
            } else if cmd == "show_model_form" {
                state.open_model_form();
            } else if cmd == "abort" {
                let a = agent.lock().await;
                a.abort_flag.store(true, std::sync::atomic::Ordering::SeqCst);
            } else {
                let notice = process_command(&mut state, &agent, &cmd).await;
                if let Some(msg) = notice {
                    let block = TurnBlock::Notice(msg);
                    if let Some(ref mut s) = state.streaming {
                        s.blocks.insert(0, block);
                    } else {
                        state.entries.push(Entry::Turn {
                            turn: 0,
                            blocks: vec![block],
                        });
                    }
                }
            }
        }
        if let Some(req) = state.take_pending_request() {
            let tx = pump.sender();
            let agent_clone = agent.clone();
            tokio::spawn(async move {
                let mut a = agent_clone.lock().await;
                let _ = a
                    .run_with_events(&req, |event| {
                        let _ = tx.send(event);
                    })
                    .await;
            });
        }

        // ── 4. Frame rate limited redraw ────────────────────────────
        let now = Instant::now();
        let since_last = now.duration_since(last_draw);
        if needs_draw && since_last >= min_frame {
            terminal.draw(|frame| render::render(frame, &mut state))?;
            last_draw = now;
            needs_draw = false;
        } else if since_last >= Duration::from_millis(500) {
            // Periodic refresh even if idle
            terminal.draw(|frame| render::render(frame, &mut state))?;
            last_draw = now;
            needs_draw = false;
        } else if needs_draw {
            // We have something to draw but haven't hit the frame interval.
            // Sleep briefly to avoid busy-looping, but keep it short
            // so we stay responsive to input.
            std::thread::sleep(Duration::from_millis(4));
        }
    }
}

/// Handle mouse events — scroll wheel, hover, and click support.
fn handle_mouse(mouse: MouseEvent, state: &mut AppState) {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_add(3);
            } else {
                state.scroll = state.scroll.saturating_add(3);
            }
        }
        MouseEventKind::ScrollDown => {
            if state.subagent_view.is_some() {
                state.subagent_scroll = state.subagent_scroll.saturating_sub(3);
            } else {
                state.scroll = state.scroll.saturating_sub(3);
            }
        }
        MouseEventKind::Moved | MouseEventKind::Drag(_) => {
            // Hover detection — only in main conversation view
            if state.subagent_view.is_none() {
                let new_hovered = find_hovered_subagent(mouse.column, mouse.row, state);
                if new_hovered != state.hovered_subagent {
                    state.hovered_subagent = new_hovered;
                    // Mark dirty so the hover border is rendered
                    state.cache_dirty = true;
                }
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            // Click on subagent box → enter detail view
            if state.subagent_view.is_none() {
                if let Some(sa_id) = find_hovered_subagent(mouse.column, mouse.row, state) {
                    state.subagent_view = Some(sa_id);
                    state.subagent_scroll = 0;
                    state.hovered_subagent = None;
                    state.mark_dirty();
                }
            }
        }
        _ => {}
    }
}

/// Map a mouse position (col, row) to a subagent ID if the mouse is
/// over a subagent block in the conversation. Returns None otherwise.
fn find_hovered_subagent(col: u16, row: u16, state: &AppState) -> Option<String> {
    let area_y = state.main_area_y;
    let area_h = state.main_area_height;

    // Check if mouse is within the conversation area
    if row < area_y || row >= area_y + area_h || area_h == 0 {
        return None;
    }

    let rel_row = (row - area_y) as usize;
    let visible_height = area_h as usize;
    let max_scroll = state.cache.wrapped_height.saturating_sub(visible_height);
    let scroll_from_top = max_scroll.saturating_sub(state.scroll);
    let abs_row = scroll_from_top + rel_row;

    // Find which cache line this row corresponds to
    let line_idx = state.cache.row_offsets
        .partition_point(|&r| r <= abs_row)
        .saturating_sub(1);

    // Check if the column is within the terminal width (basic sanity check)
    if col < 1 {
        return None;
    }

    // Check if this line index falls within any subagent range
    for &(start, end, ref id) in &state.cache.subagent_line_ranges {
        if line_idx >= start && line_idx < end {
            return Some(id.clone());
        }
    }

    None
}

// ── Pending command processor (for commands from state.rs handle_command) ─

async fn process_command(
    state: &mut AppState,
    agent: &Arc<tokio::sync::Mutex<Agent>>,
    cmd: &str,
) -> Option<String> {
    if cmd == "list_models" {
        let a = agent.lock().await;
        let mut lines = vec!["Registered models:".to_string()];
        for (name, current) in a.list_models() {
            lines.push(if current {
                format!("  * {}", name)
            } else {
                format!("    {}", name)
            });
        }
        return Some(lines.join("\n"));
    }

    if cmd.starts_with("switch_model:") {
        let name = cmd.strip_prefix("switch_model:").unwrap();
        let mut a = agent.lock().await;
        match a.switch_model(name) {
            Ok(()) => {
                state.model = name.to_string();
                update_default_model(name);
                return Some(format!("Switched to model: {}", name));
            }
            Err(e) => {
                return Some(format!("Failed: {}", e));
            }
        }
    }

    if cmd == "clear" {
        let mut a = agent.lock().await;
        a.clear_context();
        state.entries.clear();
        state.mark_dirty();
        return Some("Context cleared. New session started.".to_string());
    }

    if cmd.starts_with("register_model:") {
        let data = cmd.strip_prefix("register_model:").unwrap();
        let parts: Vec<&str> = data.splitn(4, '|').collect();
        if parts.len() != 4 {
            return Some("Invalid register_model format".to_string());
        }
        let provider = parts[0];
        let base_url = parts[1];
        let api_key = parts[2];
        let model_id = parts[3];

        let model_cfg = agent_core::config::ModelConfig {
            name: provider.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model_id: model_id.to_string(),
            max_context_tokens: 32768,
            request_timeout_secs: 120,
            ..Default::default()
        };

        let model_name = format!("{}-{}", provider, model_id);
        let mut a = agent.lock().await;
        match a.register_model(&model_name, model_cfg) {
            Ok(()) => {
                let toml_key = if model_name.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
                    format!("\"{}\"", model_name)
                } else {
                    model_name.clone()
                };
                let entry = format!(
                    "\n[models.{}]\nbase_url = \"{}\"\napi_key = \"{}\"\nmodel_id = \"{}\"\nmax_context_tokens = 32768\n",
                    toml_key, base_url, api_key, model_id
                );
                if let Err(e) = std::fs::write(
                    "config.toml",
                    std::fs::read_to_string("config.toml").unwrap_or_default() + &entry,
                ) {
                    return Some(format!(
                        "Model '{}' registered in memory, config write failed: {}",
                        model_name, e
                    ));
                }
                return Some(format!(
                    "Model registered: {} — saved to config.toml",
                    model_name
                ));
            }
            Err(e) => {
                return Some(format!("Failed: {}", e));
            }
        }
    }

    None
}

/// Rewrite config.toml's `default_model` to persist the current selection.
fn update_default_model(model_name: &str) {
    if let Ok(content) = std::fs::read_to_string("config.toml") {
        // Replace the line that starts with "default_model = "
        let updated: String = content
            .lines()
            .map(|line| {
                if line.trim_start().starts_with("default_model") {
                    format!("default_model = \"{}\"", model_name)
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        let _ = std::fs::write("config.toml", updated);
    }
}
