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
