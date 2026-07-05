pub mod input;
pub mod markdown;
pub mod render;
pub mod state;
pub mod widgets;

use std::sync::Arc;

pub async fn run_tui(_state: Arc<tokio::sync::Mutex<crate::state::CliState>>) -> anyhow::Result<()> {
    println!("TUI mode requires agent_core Agent — not yet ported to CliState.");
    println!("Use CLI mode instead: cargo run -- --no-tui");
    Err(anyhow::anyhow!("TUI not yet ported"))
}
