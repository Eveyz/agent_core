pub mod input;
pub mod render;
pub mod state;

use agent_core::Agent;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use state::{AppState, EventPump};
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

    // Populate status info from agent
    {
        let a = agent.lock().await;
        state.model = a.current_model().to_string();
        state.tool_mode = format!("{:?}", a.tool_execution_mode());
    }
    let mut pump = EventPump::new();
    let mut last_draw = Instant::now();
    let min_frame = Duration::from_millis(16); // ~60 fps cap while streaming/input is active
    let mut needs_draw = true;
    loop {
        // Drain agent events (non-blocking)
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
        if let Some(req) = state.take_pending_request() {
            if req.starts_with('/') {
                let mut a = agent.lock().await;
                let output = handle_tui_command(&mut a, &req);
                if let Some(out) = output {
                    state.entries.push(state::Entry::System { text: out });
                }
                state.agent_running = false;
                state.agent_state = "idle".into();
                continue;
            }

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
            terminal.draw(|frame| render::render(frame, &state))?;
            last_draw = now;
            needs_draw = false;
        } else if since_last >= Duration::from_millis(500) {
            // Periodic refresh even if idle
            terminal.draw(|frame| render::render(frame, &state))?;
            last_draw = now;
            needs_draw = false;
        }
    }
}

fn handle_tui_command(agent: &mut Agent, cmd: &str) -> Option<String> {
    let cmd = cmd.trim();
    match cmd {
        "/quit" | "/exit" => {
            // will be handled by state.should_quit if we caught it in input.rs
            // but we didn't, so let's just return a message
            Some("Use 'Esc' to quit TUI mode.".into())
        }
        "/help" => {
            Some("Available commands: /models, /clear, /memory, /tokens, /model <name>, /temp <val>, /max-tokens <val>, /tool-mode <mode>, /clear-queues".into())
        }
        "/models" => {
            let mut out = String::from("=== Available Models ===\n");
            for (name, is_current) in agent.list_models() {
                let marker = if is_current { "* " } else { "  " };
                out.push_str(&format!("{}{}\n", marker, name));
            }
            Some(out)
        }
        "/clear" => {
            agent.clear_context();
            Some("Context cleared. New session started.".into())
        }
        "/memory" => {
            if let Some(memory) = agent.memory() {
                let mut out = String::from("=== Core Memory ===\n");
                for block in memory.core().list() {
                    out.push_str(&format!("[{}]: {}\n", block.id, block.content));
                }
                out.push_str(&format!("\nSession: {}", memory.session_id()));
                Some(out)
            } else {
                Some("Memory is disabled.".into())
            }
        }
        "/tokens" => {
            Some(format!("Current tokens: {}", agent.context_token_count()))
        }
        "/clear-queues" => {
            agent.clear_all_queues();
            Some("Steering and follow-up queues cleared.".into())
        }
        c if c.starts_with("/model ") => {
            let name = c.strip_prefix("/model ").unwrap().trim();
            match agent.switch_model(name) {
                Ok(()) => Some(format!("Switched to model: {name}")),
                Err(e) => Some(format!("Error: {e}")),
            }
        }
        c if c.starts_with("/temp ") => {
            let val_str = c.strip_prefix("/temp ").unwrap().trim();
            match val_str.parse::<f64>() {
                Ok(val) => {
                    agent.set_temperature(val);
                    Some(format!("Temperature set to {val}"))
                }
                Err(_) => Some("Invalid temperature value".into()),
            }
        }
        c if c.starts_with("/max-tokens ") => {
            let val_str = c.strip_prefix("/max-tokens ").unwrap().trim();
            match val_str.parse::<u32>() {
                Ok(val) => {
                    agent.set_max_tokens(val);
                    Some(format!("Max tokens set to {val}"))
                }
                Err(_) => Some("Invalid max-tokens value".into()),
            }
        }
        c if c.starts_with("/tool-mode ") => {
            let mode_str = c.strip_prefix("/tool-mode ").unwrap().trim();
            match mode_str.to_lowercase().as_str() {
                "parallel" | "par" => {
                    agent.set_tool_execution_mode(agent_core::ToolExecutionMode::Parallel);
                    Some("Tool execution mode set to: parallel".into())
                }
                "sequential" | "seq" => {
                    agent.set_tool_execution_mode(agent_core::ToolExecutionMode::Sequential);
                    Some("Tool execution mode set to: sequential".into())
                }
                _ => Some("Usage: /tool-mode <parallel|sequential>".into()),
            }
        }
        _ => Some(format!("Unknown or unsupported TUI command: {}", cmd)),
    }
}
