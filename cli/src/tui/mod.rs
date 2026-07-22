pub mod events;
pub mod input;
pub mod markdown;
pub mod render;
pub mod state;
pub mod widgets;

use crate::commands::{self, CommandOutcome};
use crate::state::CliState;
use agent_core::PermissionMode;
use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use state::{AppEvent, AppState};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

const STREAMING_REBUILD_THROTTLE: Duration = Duration::from_millis(50);

pub async fn run_tui(cli: Arc<Mutex<CliState>>) -> Result<()> {
    crossterm::execute!(std::io::stdout(), crossterm::event::EnableMouseCapture)?;
    {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        write!(stdout, "\x1B[?1003h")?;
        stdout.flush()?;
    }
    let mut terminal = ratatui::init();
    let result = run_app(&mut terminal, cli).await;
    ratatui::restore();
    {
        use std::io::Write;
        let mut stdout = std::io::stdout();
        write!(stdout, "\x1B[?1003l")?;
        stdout.flush()?;
    }
    let _ = crossterm::execute!(std::io::stdout(), crossterm::event::DisableMouseCapture);
    result
}

fn short_cwd() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .unwrap_or_else(|| ".".into())
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

async fn run_app(terminal: &mut ratatui::DefaultTerminal, cli: Arc<Mutex<CliState>>) -> Result<()> {
    let mut state = AppState::new();
    {
        let s = cli.lock().await;
        state.model = s.brain.current_model_name().to_string();
        state.tool_mode = format!("{:?}", s.brain.tool_execution_mode);
        state.permission_label = format!("{:?}", s.brain.config.permissions.mode).to_lowercase();
        state.enable_permission = !matches!(s.brain.config.permissions.mode, PermissionMode::Yolo);
        state.enable_hooks = !s.brain.hook_registry.lock().is_empty();
        state.cwd_short = short_cwd();
        state.session_short = s
            .session_id
            .as_deref()
            .map(short_id)
            .unwrap_or_else(|| "(new)".into());
        state.max_context_tokens = s.run_manager.current_max_context_tokens();
        state.tokens = s.context_history.len() * 4;
        state.recompute_context_pct();
    }

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<AppEvent>();

    let tick_tx = tx.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_millis(50));
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
                    Event::Paste(data) => {
                        use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
                        for ch in data.chars() {
                            let code = if ch == '\n' { KeyCode::Enter } else { KeyCode::Char(ch) };
                            let _ = tx.send(AppEvent::Key(KeyEvent::new(code, KeyModifiers::NONE)));
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Ok(size) = terminal.size() {
                            let area: ratatui::layout::Rect = size.into();
                            let input_h = render::input_height(&state, area.width);
                            let layout = render::compute_layout(area, input_h);
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
        while let Ok(ev) = rx.try_recv() {
            needs_draw |= state.apply(ev);
            if state.should_quit {
                break;
            }
        }
        if state.should_quit {
            let mut s = cli.lock().await;
            let _ = commands::save_session(&mut s);
            let mut mgr = s.mcp_mgr.lock().await;
            let _ = mgr.shutdown_all().await;
            break Ok(());
        }

        // ── 3. Pending slash / UI commands ──────────────────────────
        if let Some(cmd) = state.take_pending_command() {
            needs_draw = true;
            handle_pending_command(&cli, &mut state, &cmd).await;
        }

        // ── 4. Pending approval / answer / steer ────────────────────
        if let Some((prompt_id, choice_key)) = state.take_pending_approval() {
            let choice = commands::approval_from_choice_key(&choice_key);
            events::resolve_approval(&cli, &prompt_id, choice).await;
            state.cancel_modal();
            needs_draw = true;
        }
        if let Some((prompt_id, answer)) = state.take_pending_answer() {
            events::resolve_answer(&cli, &prompt_id, answer).await;
            state.cancel_modal();
            needs_draw = true;
        }
        if state.pending_abort {
            state.pending_abort = false;
            if let Some(err) = events::abort_run(&cli).await {
                state.push_notice(err);
            }
            needs_draw = true;
        }
        if let Some(msg) = state.take_pending_steer() {
            if let Some(err) = events::send_steer(&cli, msg).await {
                state.push_notice(err);
            } else {
                state.push_notice("Steer queued.");
            }
            needs_draw = true;
        }
        if let Some(text) = state.take_pending_yank() {
            match arboard::Clipboard::new().and_then(|mut c| c.set_text(text)) {
                Ok(()) => state.push_notice("Copied to clipboard."),
                Err(e) => state.push_notice(format!("Clipboard unavailable: {e}")),
            }
            needs_draw = true;
        }

        // ── 5. Pending user message → create_run ────────────────────
        if let Some(req) = state.take_pending_request() {
            events::spawn_run(cli.clone(), tx.clone(), req);
        }

        // ── 6. Cache rebuild + scroll clamp ──────────────────────────
        if let Ok(term_size) = terminal.size() {
            let area: ratatui::layout::Rect = term_size.into();
            let input_h = render::input_height(&state, area.width);
            let content_width = area.width.saturating_sub(2);
            let needs_rebuild = state.cache_dirty || state.cache.width != content_width;
            if needs_rebuild {
                let should_rebuild_now = if state.force_cache_rebuild {
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
            let layout = render::compute_layout(area, input_h);
            let visible_height = layout.main.height as usize;
            state.viewport_h = visible_height;
            let max_scroll = state.cache.wrapped_height.saturating_sub(visible_height);
            state.scroll = state.scroll.min(max_scroll);
        }

        // ── 7. Frame-rate-limited redraw ──────────────────────────────
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
            tokio::time::sleep(Duration::from_millis(4)).await;
        } else {
            tokio::time::sleep(Duration::from_millis(8)).await;
        }
    }
}

/// Handle a queued `state.pending_command`. Slash commands are prefixed with
/// `slash:`; a few TUI-local actions (`register_model:...`) are handled here
/// directly since they need `CliState` + config-file access.
async fn handle_pending_command(cli: &Arc<Mutex<CliState>>, state: &mut AppState, cmd: &str) {
    if let Some(slash) = cmd.strip_prefix("slash:") {
        let needs_async = matches!(slash, "/abort" | "/mcp" | "/clear-queues")
            || slash.starts_with("/steer ")
            || slash.starts_with("/follow-up ");

        let mut s = cli.lock().await;

        // Two UiRequest variants carry no payload from `commands::dispatch_*`
        // and need CliState access to populate — handle them here while the
        // lock is held, before falling through to the generic path.
        let trimmed = slash.trim();
        if trimmed == "/status" {
            let text = commands::format_status(&s, state.enable_permission, state.enable_hooks);
            drop(s);
            state.open_pager("status", text);
            return;
        }
        if trimmed == "/sessions" {
            let sessions = match s.session_mgr.list(false) {
                Ok(list) => list
                    .into_iter()
                    .map(|m| (m.id.clone(), m.display_line()))
                    .collect(),
                Err(e) => {
                    drop(s);
                    state.push_notice(format!("Failed to list sessions: {e}"));
                    return;
                }
            };
            drop(s);
            state.open_session_list(sessions);
            return;
        }

        let outcome = if needs_async {
            commands::dispatch_async(&mut s, slash, state.enable_permission, state.enable_hooks).await
        } else {
            commands::dispatch_sync(&mut s, slash, state.enable_permission, state.enable_hooks)
        };

        state.model = s.brain.current_model_name().to_string();
        state.tool_mode = format!("{:?}", s.brain.tool_execution_mode);
        state.permission_label = format!("{:?}", s.brain.config.permissions.mode).to_lowercase();
        state.enable_permission = !matches!(s.brain.config.permissions.mode, PermissionMode::Yolo);
        state.session_short = s
            .session_id
            .as_deref()
            .map(short_id)
            .unwrap_or_else(|| "(new)".into());
        state.tokens = s.context_history.len() * 4;
        state.recompute_context_pct();
        drop(s);

        apply_outcome(state, outcome);
    } else if let Some(data) = cmd.strip_prefix("register_model:") {
        apply_register_model(cli, state, data).await;
    } else if let Some(name) = cmd.strip_prefix("select_model:") {
        let mut s = cli.lock().await;
        let outcome = commands::dispatch_sync(
            &mut s,
            &format!("/model {name}"),
            state.enable_permission,
            state.enable_hooks,
        );
        state.model = s.brain.current_model_name().to_string();
        drop(s);
        apply_outcome(state, outcome);
    } else if let Some(idx) = cmd.strip_prefix("rewind_to:") {
        let mut s = cli.lock().await;
        let outcome = commands::dispatch_sync(
            &mut s,
            &format!("/rewind {idx}"),
            state.enable_permission,
            state.enable_hooks,
        );
        drop(s);
        apply_outcome(state, outcome);
    } else if let Some(id) = cmd.strip_prefix("resume_session:") {
        let mut s = cli.lock().await;
        let outcome = commands::dispatch_sync(
            &mut s,
            &format!("/session resume {id}"),
            state.enable_permission,
            state.enable_hooks,
        );
        state.session_short = s.session_id.as_deref().map(short_id).unwrap_or_else(|| "(new)".into());
        state.tokens = s.context_history.len() * 4;
        drop(s);
        apply_outcome(state, outcome);
    }
}

fn apply_outcome(state: &mut AppState, outcome: CommandOutcome) {
    match outcome {
        CommandOutcome::Quit => state.modal = state::ModalState::QuitConfirm,
        other => state.apply_command_outcome(other),
    }
    state.mark_dirty();
}

async fn apply_register_model(cli: &Arc<Mutex<CliState>>, state: &mut AppState, data: &str) {
    let parts: Vec<&str> = data.splitn(4, '|').collect();
    if parts.len() != 4 {
        state.push_notice("Invalid register_model format.");
        return;
    }
    let (provider, base_url, api_key, model_id) = (parts[0], parts[1], parts[2], parts[3]);
    if provider.is_empty() || base_url.is_empty() || model_id.is_empty() {
        state.push_notice("Provider, base URL, and model ID are required.");
        return;
    }
    let model_cfg = agent_core::config::ModelConfig {
        name: provider.to_string(),
        base_url: base_url.to_string(),
        api_key: api_key.to_string(),
        model_id: model_id.to_string(),
        max_context_tokens: 32768,
        request_timeout_secs: 120,
        ..Default::default()
    };
    let model_name = format!("{provider}/{model_id}");
    let mut s = cli.lock().await;
    let mut cfg = s.brain.config.clone();
    cfg.add_model(model_name.clone(), model_cfg);
    if let Err(e) = s.run_manager.update_config(cfg.clone()) {
        state.push_notice(format!("Registered in memory but update failed: {e}"));
        return;
    }
    s.brain = (**s.run_manager.brain()).clone();
    let path = agent_core::paths::get_agverse_dir().join("config.toml");
    if let Err(e) = cfg.save(&path.to_string_lossy()) {
        state.push_notice(format!("Model '{model_name}' registered; config save failed: {e}"));
    } else {
        state.push_notice(format!("Model registered: {model_name} — saved to config.toml"));
    }
    state.model = s.brain.current_model_name().to_string();
}
