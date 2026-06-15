pub mod input;
pub mod markdown;
pub mod render;
pub mod state;
pub mod widgets;

use agent_core::Agent;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use state::{AppEvent, AppState, Entry, TurnBlock};
use std::sync::Arc;
use std::time::{Duration, Instant};

const STREAMING_REBUILD_THROTTLE: Duration = Duration::from_millis(50);

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
        state.abort_flag = Some(a.abort_flag.clone());
    }

    // ── MPSC channel ──────────────────────────────────────────────
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

    // Tick task — drives animation via AppEvent::Tick
    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(16));
        loop {
            interval.tick().await;
            if tick_tx.send(AppEvent::Tick).is_err() {
                break;
            }
        }
    });

    let mut last_draw = Instant::now();
    let min_frame = Duration::from_millis(16);
    let mut needs_draw = true;

    loop {
        // ── 1. Poll crossterm → channel ─────────────────────────────
        loop {
            if event::poll(Duration::from_millis(0))? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        let _ = tx.send(AppEvent::Key(key));
                    }
                    Event::Mouse(mouse) => {
                        if let Ok(size) = terminal.size() {
                            let layout = render::compute_layout(size.into(), render::dropdown_height(&state));
                            let _ = tx.send(AppEvent::Mouse(mouse, layout.main));
                        }
                    }
                    Event::Resize(_, _) => {
                        let _ = tx.send(AppEvent::Resize);
                    }
                    _ => {}
                }
            } else {
                break;
            }
        }

        // ── 2. Process ALL pending AppEvents ────────────────────────
        while let Ok(event) = rx.try_recv() {
            needs_draw |= state.apply(event);
            if state.should_quit {
                break;
            }
        }
        if state.should_quit {
            break Ok(());
        }

        // ── 4. Process pending slash commands / requests ────────────
        if let Some(cmd) = state.take_pending_command() {
            needs_draw = true;
            if cmd == "show_model_picker" {
                let a = agent.lock().await;
                let models: Vec<String> = a.list_models().into_iter().map(|(n, _)| n.to_string()).collect();
                state.open_model_picker(models);
            } else if cmd == "show_model_form" {
                state.open_model_form();
            } else if cmd == "abort" {
                if let Some(ref flag) = state.abort_flag {
                    flag.store(true, std::sync::atomic::Ordering::SeqCst);
                }
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
            let req_tx = tx.clone();
            let agent_clone = agent.clone();
            tokio::spawn(async move {
                let mut a = agent_clone.lock().await;
                let _ = a
                    .run_with_events(&req, |event| {
                        let _ = req_tx.send(AppEvent::Agent(event));
                    })
                    .await;
            });
        }

        // ── 5. Cache rebuild (still in loop, but render stays read-only) ──
        if let Ok(term_size) = terminal.size() {
            let width = term_size.width;
            let content_width = width.saturating_sub(2);
            let needs_rebuild = state.cache_dirty || state.cache.width != content_width;
            if needs_rebuild {
                let should_rebuild_now = if state.force_cache_rebuild {
                    state.force_cache_rebuild = false;
                    true
                } else if state.agent_running {
                    match state.cache.last_rebuild {
                        Some(last) => Instant::now().duration_since(last) >= STREAMING_REBUILD_THROTTLE,
                        None => true,
                    }
                } else {
                    true
                };
                if should_rebuild_now {
                    render::rebuild_cache(&mut state, content_width);
                }
            }
            // Clamp scroll after cache rebuild
            let layout = render::compute_layout(term_size.into(), render::dropdown_height(&state));
            let visible_height = layout.main.height as usize;
            let max_scroll = state.cache.wrapped_height.saturating_sub(visible_height);
            state.scroll = state.scroll.min(max_scroll);
        }

        // ── 6. Frame rate limited redraw ────────────────────────────
        let now = Instant::now();
        let since_last = now.duration_since(last_draw);
        if needs_draw && since_last >= min_frame {
            terminal.draw(|frame| render::render(frame, &state))?;
            last_draw = now;
            needs_draw = false;
        } else if since_last >= Duration::from_millis(500) {
            terminal.draw(|frame| render::render(frame, &state))?;
            last_draw = now;
            needs_draw = false;
        } else if needs_draw {
            std::thread::sleep(Duration::from_millis(4));
        }
    }
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

        let model_name = format!("{}/{}", provider, model_id);
        let mut a = agent.lock().await;
        match a.register_model(&model_name, model_cfg) {
            Ok(()) => {
                let provider_toml_key = if provider.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-') {
                    format!("\"{}\"", provider)
                } else {
                    provider.to_string()
                };
                let model_toml_key = if model_id.contains(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-' && c != ':' && c != '.') {
                    format!("\"{}\"", model_id)
                } else {
                    model_id.to_string()
                };
                let entry = format!(
                    "\n[providers.{}]\nname = \"{}\"\nbase_url = \"{}\"\napi_key = \"{}\"\n\n[providers.{}.models.{}]\nmodel_id = \"{}\"\n",
                    provider_toml_key, provider, base_url, api_key, provider_toml_key, model_toml_key, model_id
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
