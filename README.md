# Agverse

**Agverse** is a Rust-native AI coding agent: one shared runtime, four surfaces.

| Surface | Binary / app | What it is |
| -------- | ------------ | ---------- |
| **Desktop** | Agverse (Tauri + React) | Full GUI with streaming chat, skills, workflows |
| **CLI** | `ageverse` | Interactive REPL |
| **TUI** | `ageverse --tui` | Full-screen terminal UI |
| **Gateway** | `agverse-gateway` | Remote HTTP control plane (Axum) |

Repo name: **`agent_core`**. Shared library crate: `agent_core`. CLI package: `agent-cli` → binary **`ageverse`**.

> Early / `0.1.0` — APIs and UI will change. Design internals: [`docs/DESIGN.md`](docs/DESIGN.md).

---

## Why Agverse

Most agent CLIs glue a chat UI to an HTTP client. Agverse treats the **model window as an engineered artifact**:

- **KV-cache–aware prompts** — frozen system prefix, trailing per-turn injection, cache hit/miss telemetry
- **Dual-track history** — lean model window vs full UI/persistence transcript
- **Layered memory** — Core / Recall / Archival + `~/.agverse/agverse.md`
- **Permission policy** — sandbox → modes → rules; five-tier human approvals
- **Durable runs** — crash-friendly sessions, SQLite memory, append-only run logs
- **MCP & skills** — remote tools + on-disk skill packs with live refresh
- **Workflows** — agent-driven durable workflows and `@workflow` mentions
- **Eval harness** — contract / golden suites + Harbor adapter

---

## Quick start

### Prerequisites

- **Rust** (recent stable; edition 2024)
- **Node.js 20+** + npm (desktop app only)
- An **API key** for at least one OpenAI-compatible provider

### Configure

```bash
mkdir -p ~/.agverse
cp config.toml ~/.agverse/config.toml
# set DEEPSEEK_KEY / OPENAI_API_KEY, or paste keys into the file
```

Minimal config:

```toml
default_model = "deepseek"

[models.deepseek]
base_url = "https://api.deepseek.com/v1"
api_key = "${DEEPSEEK_KEY}"
model_id = "deepseek-chat"
max_context_tokens = 65536

[permissions]
auto_allow_up_to = "readonly"
```

On first run without a config file, the CLI can scaffold `~/.agverse/config.toml` from `OPENAI_API_KEY` (optional `OPENAI_BASE_URL` / `OPENAI_MODEL`).

Validate:

```bash
cargo run -p agent-cli -- config show
cargo run -p agent-cli -- config validate
```

### CLI (REPL)

```bash
cargo run -p agent-cli
# release:
cargo build -p agent-cli --release
./target/release/ageverse
```

Useful flags:

```bash
cargo run -p agent-cli -- --model deepseek
cargo run -p agent-cli -- --permission developer
cargo run -p agent-cli -- -p "Summarize this repo"   # oneshot
```

Inside the REPL: `/help`. Exit: `/quit` or Ctrl+D.

### TUI

```bash
cargo run -p agent-cli -- --tui
```

### Desktop GUI

```bash
cd app
npm install
npm run tauri -- dev      # http://localhost:1420
```

Frontend-only (no native shell): `npm run dev` · tests: `npm test`.

### Gateway

```bash
export AGVERSE_API_KEY=dev-secret
cargo run -p agverse-gateway
# 127.0.0.1:8787
```

See [`gateway/README.md`](gateway/README.md).

---

## Repository layout

```
agent_core/
├── core/              # agent_core library (runtime, tools, memory, workflows)
├── cli/               # ageverse (REPL + TUI + oneshot + eval)
├── gateway/           # agverse-gateway HTTP API
├── app/               # Desktop UI (React/Vite + Tauri)
├── evals/             # Contract / golden harness suites
├── ageverse_harbor/   # Harbor benchmark adapter
├── docs/              # ADRs, plans, design deep dive
├── config.toml        # Sample config
└── Cargo.toml         # Workspace
```

| Package | Path | Role |
| ------- | ---- | ---- |
| `agent_core` | `core/` | Shared runtime |
| `agent-cli` | `cli/` | `ageverse` binary |
| `agverse-gateway` | `gateway/` | Remote HTTP API |
| `app` (Tauri) | `app/src-tauri/` | Desktop backend |

---

## Development

```bash
# Library + CLI
cargo check -p agent_core
cargo check -p agent-cli
cargo test -p agent_core
cargo test -p agent-cli --bin ageverse

# Desktop
cd app && npm install && npm run tauri -- dev
```

Tips:

- Pass CLI flags after `--`: `cargo run -p agent-cli -- --tui`
- CLI builds `agent_core` **without** embeddings (slim). Desktop enables ONNX embeddings by default.
- Logs: `RUST_LOG=info` / `debug`

### Release builds

```bash
cargo build -p agent-cli --release          # → target/release/ageverse
cd app && npm install && npm run tauri -- build
# installers under app/src-tauri/target/release/bundle/
```

Harbor / Linux x86_64 from macOS: `./ageverse_harbor/build-linux.sh` — see [`ageverse_harbor/README.md`](ageverse_harbor/README.md).

### Shell completion

```bash
ageverse completion zsh > ~/.zsh_completions/_ageverse
ageverse completion bash
ageverse completion fish
```

### Eval

```bash
./evals/run_contract.sh
cargo run -p agent-cli -- eval run --suite contract_v1 --mode mock --gate
cargo run -p agent-cli -- eval run --suite golden_v1 --mode live --model deepseek
```

Details: [`evals/README.md`](evals/README.md).

---

## Data on disk

| Path | Purpose |
| ---- | ------- |
| `~/.agverse/config.toml` | Models, permissions, MCP, … |
| `~/.agverse/memory.db` | Memory + session storage |
| `~/.agverse/agverse.md` | Human-editable global notes |
| `~/.agverse/sessions/<id>/` | Crash snapshots, tool spills, artifacts |
| `~/.agverse/runs/<run_id>.jsonl` | Append-only run event logs |
| `~/.agverse/skills/` | Skill packs |

---

## Documentation

| Doc | Topic |
| --- | ----- |
| [`docs/DESIGN.md`](docs/DESIGN.md) | Architecture & design deep dive |
| [`docs/index.md`](docs/index.md) | ADR / PLAN / RFC index |
| [`docs/README_PROCESS.md`](docs/README_PROCESS.md) | Doc lifecycle |
| [`evals/README.md`](evals/README.md) | Eval harness |
| [`ageverse_harbor/README.md`](ageverse_harbor/README.md) | Harbor adapter |
| [`gateway/README.md`](gateway/README.md) | Remote gateway |
| [`examples/README.md`](examples/README.md) | Library examples |

---

## Troubleshooting

| Symptom | Fix |
| ------- | --- |
| Auth / no models | Check `~/.agverse/config.toml` and `${ENV}` vars (`config validate`) |
| `ageverse: command not found` | Use `cargo run -p agent-cli` or put `target/release/ageverse` on `PATH` |
| Harbor `Exec format error` on macOS | Build with `./ageverse_harbor/build-linux.sh` |
| GUI won’t start | `cd app && npm install`, Rust toolchain, then `npm run tauri -- dev` |
| TUI leaves terminal messy | Exit via `/quit` (avoid `kill -9` mid-draw) |

---

## Contributing

Issues and PRs are welcome — please include repro steps for bugs.

Suggested workflow:

1. `cargo test -p agent_core` (and relevant `app` Vitest) before opening a PR
2. Prefer small, focused changes; link related ADRs/plans under `docs/` when changing architecture
3. Do not commit secrets, `~/.agverse` data, or `node_modules`

---

## License

License **TBD** — a `LICENSE` file will ship with the public release. Until then, treat the code as source-available for evaluation only (all rights reserved by the authors).

```bash
ageverse -V    # version + git commit on release builds
```
