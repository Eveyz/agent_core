//! agent_core Usage Guide
//!
//! This file is both documentation and a reference for the agent_core API.
//!
//! Run examples with: `cargo run --example <name>`

/// # Quick Start
///
/// ```rust,no_run
/// use agent_core::AgentBuilder;
///
/// #[tokio::main]
/// async fn main() -> anyhow::Result<()> {
///     // Load config from config.toml
///     let mut agent = AgentBuilder::from_config("config.toml")?
///         .with_memory(false)
///         .build()?;
///
///     // Run a prompt
///     let response = agent.run("What is Rust?").await?;
///     println!("{}", response);
///     Ok(())
/// }
/// ```
///
/// # Configuration
///
/// config.toml:
/// ```toml
/// default_model = "openai"
///
/// [models.openai]
/// name = "openai"
/// base_url = "https://api.openai.com/v1"
/// api_key = "sk-..."
/// model_id = "gpt-4o"
///
/// [permissions]
/// mode = "standard"
/// ```
///
/// # Permission Modes
///
/// - `paranoid`: Everything needs approval
/// - `standard`: Built-in rules, ask for unknowns (default)
/// - `permissive`: Auto-allow up to Network level
/// - `yolo`: Allow everything, no questions asked
///
/// # Skills
///
/// Create a skill file at `.agent/skills/<name>/SKILL.md`:
/// ```markdown
/// ---
/// name: my-skill
/// description: My custom skill
/// triggers: [keyword1, keyword2]
/// priority: 10
/// ---
/// # Skill Content
/// This knowledge will be injected into the agent's context.
/// ```
///
/// # MCP Servers
///
/// config.toml:
/// ```toml
/// [[mcp.servers]]
/// name = "filesystem"
/// command = "npx"
/// args = ["-y", "@modelcontextprotocol/server-filesystem", "/allowed/dir"]
/// ```
///
/// # Session Management
///
/// ```text
/// /sessions              List all saved sessions
/// /session save          Save current conversation
/// /session resume <id>   Resume a previous session
/// /session delete <id>   Delete a session
/// ```
fn main() {}
