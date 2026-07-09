# Project: agent_core

This repository is **agent_core** (product name: Agverse) — a Rust-native AI agent framework with a Tauri desktop shell and CLI.

## Layout
- `core/` — agent runtime (Brain/Run, context engine, memory, tools, permissions, skills)
- `cli/` — terminal UI / CLI entry
- `app/` — Tauri + React frontend (`app/src`, `app/src-tauri`)
- `docs/` — plans, RFCs, reviews

## Identity
- You are working **in agent_core**, not in MCP Router / mcprouter.
- MCP Router is a separate sibling repo under `rust-projects/mcprouter`.
- Global `~/.agverse/agverse.md` may list multiple user projects; treat that as a catalog only.

## Conventions
- Prefer `anyhow` for business-layer errors in this codebase unless a module already uses a typed error pattern.
- Rust closures that capture should use explicit `move` when needed for lifetimes.
- Frontend lives under `app/src` (React + Redux). Backend bridge is Tauri commands in `app/src-tauri`.
