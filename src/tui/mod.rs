pub mod input;
pub mod render;
pub mod state;

use agent_core::Agent;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use state::{AppState, Entry, EventPump, TurnBlock};
use std::sync::Arc;
use std::time::{Duration, Instant};
pub async fn run_tui(agent: Arc<tokio::sync::Mutex<Agent>>) -> Result<()> {
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, agent).await;
    ratatui::restore();
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
        // Drain agent events (non-blocking)
        // Approvals are handled directly inside handle_agent_event via
        // the shared approvals Arc — avoids deadlock with the tokio mutex.
        let had_events = pump.drain(&mut state);
        // Handle keyboard input
        let had_input = if event::poll(Duration::from_millis(8))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    input::handle_key(key, &mut state)?;
                    true
                }
                Event::Resize(_, _) => true,
                _ => false,
            }
        } else {
            false
        };
        needs_draw |= had_events || had_input;
        if state.should_quit {
            break Ok(());
        }

        // ── Process pending slash commands ─────────────────────────
        if let Some(cmd) = state.take_pending_command() {
            needs_draw = true;
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
        // Frame rate limited redraw
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
        }
    }
}

// ── Slash command processor ─────────────────────────────────────────
// Handles commands that need access to the Agent (model list, switch, etc.)

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
                return Some(format!("Switched to model:\n  {}", name));
            }
            Err(e) => {
                return Some(format!("Failed to switch:\n  {}", e));
            }
        }
    }

    if cmd == "clear" {
        let mut a = agent.lock().await;
        a.clear_context();
        state.entries.clear();
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

        // Create model config
        let model_cfg = agent_core::config::ModelConfig {
            name: provider.to_string(),
            base_url: base_url.to_string(),
            api_key: api_key.to_string(),
            model_id: model_id.to_string(),
            max_context_tokens: 32768,
            ..Default::default()
        };

        let model_name = format!("{}-{}", provider, model_id);
        let mut a = agent.lock().await;
        match a.register_model(&model_name, model_cfg) {
            Ok(()) => {
                // Quote key if it contains special TOML characters
                let toml_key = if model_name.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
                    format!("\"{}\"", model_name)
                } else {
                    model_name.clone()
                };
                // Try to persist to config.toml
                let entry = format!(
                    "\n[models.{}]\nbase_url = \"{}\"\napi_key = \"{}\"\nmodel_id = \"{}\"\nmax_context_tokens = 32768\n",
                    toml_key, base_url, api_key, model_id
                );
                if let Err(e) = std::fs::write(
                    "config.toml",
                    std::fs::read_to_string("config.toml").unwrap_or_default() + &entry,
                ) {
                    return Some(format!(
                        "Model '{}' registered (memory only).\n  Config write failed: {}",
                        model_name, e
                    ));
                }
                return Some(format!(
                    "Model registered:\n  {}\n  Use /model {} to switch.",
                    model_name, model_name
                ));
            }
            Err(e) => {
                return Some(format!("Failed to register model: {}", e));
            }
        }
    }

    None
}
