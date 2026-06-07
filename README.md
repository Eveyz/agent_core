# agent_core — Rust-native AI Agent Framework

A modular, extensible AI agent framework built in Rust. Supports
ReAct reasoning, tool calling, multi-agent teams, MCP servers,
skills, session persistence, and more.

## Quick Start

```bash
# Clone and build
cd agent_core
cargo build --release

# Edit config.toml with your LLM API key
# Then run:
cargo run --release
```

## Architecture

```
agent_core/
├── src/
│   ├── agent.rs           ← ReAct loop, tool execution, context management
│   ├── client/            ← OpenAI-compatible HTTP client + SSE streaming
│   ├── compressor.rs      ← 5-stage context compression pipeline
│   ├── config.rs          ← TOML config loader
│   ├── context.rs         ← 7-segment context engine + KV cache hints
│   ├── hooks/             ← Pre/post hooks (modify, veto)
│   ├── mcp/               ← MCP client (stdio + SSE transport)
│   ├── memory/            ← 4-layer memory (Core/Recall/Archival/Salience)
│   ├── permission/        ← 6-layer permission engine + audit
│   ├── prompt.rs          ← System prompt templates
│   ├── session.rs         ← Session persistence (save/resume/list)
│   ├── skills/            ← Skill manager (auto-trigger, catalog)
│   ├── subagent/          ← Sub-agent spawning + concurrent execution
│   ├── tasks/             ← Task DAG (create/plan/execute)
│   ├── teams/             ← Multi-agent message bus
│   ├── tools/             ← Built-in tools + MCP bridge
│   └── tui/               ← Terminal UI (ratatui)
├── config.toml            ← Default configuration
├── tests/
│   └── integration.rs     ← Integration tests
└── examples/
    └── README.md          ← Usage examples
```

## Key Features

| Feature | Description |
|---------|-------------|
| **ReAct Loop** | Think → Act → Observe cycle with streaming |
| **7-Segment Context** | Semantic context assembly with token budgets |
| **5-Stage Compression** | Snip→Dedup→Chunk→LLM Summary→Gradient |
| **6-Layer Permissions** | Blacklist→Whitelist→Config→Builtin→Yolo |
| **Ebbinghaus Memory** | Forgetting curve + access reinforcement |
| **MCP Client** | stdio + SSE transport, auto tool discovery |
| **Skill Auto-Trigger** | Match user messages → load skill context |
| **Subagent Routing** | Heuristic decision: inline vs spawn vs concurrent |
| **Session Persistence** | Save/resume/list/search conversations |
| **Task DAG** | Dependencies, parallel execution, auto-unblock |

## Configuration

```toml
default_model = "openai"

[models.openai]
base_url = "https://api.openai.com/v1"
api_key = "sk-..."
model_id = "gpt-4o"
max_context_tokens = 128000

[permissions]
mode = "standard"          # paranoid | standard | permissive | yolo

[memory]
db_path = "~/.agent_core/memory.db"

[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]
```

## CLI Commands

```
/help           Show available commands
/status         Agent status (model, tokens, skills, tasks)
/sessions       List saved sessions
/session save   Save current conversation
/session resume <id>  Resume a session
/skills         List available skills
/skill <name>   Load a skill
/todo           Show todo list
```

## Testing

```bash
# Unit tests (190+ tests)
cargo test

# Integration tests
cargo test --test integration

# Check without building
cargo check
```
