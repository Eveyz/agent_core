# 2026-06-29: Cache Hit Optimization Analysis

**Date**: 2026-06-29
**Subject**: Analysis of Cache Hit Optimization after introducing Modes and Skills
**Author**: Antigravity AI Agent

## Overview

A comprehensive analysis has been conducted on the `agent_core` context assembly mechanism (`ContextEngine`) to determine if the recently introduced features — **Agent Modes (Build, Plan, Ask)** and **Skill Support** — have broken or degraded the KV cache hit optimizations.

**Conclusion**: The cache hit logic is **perfectly intact** and has not been broken. The 7-segment semantic architecture correctly isolated the static prefix from the dynamic injections.

## Detailed Analysis

### 1. The Core Cache Hit Mechanism
The KV cache hit relies on the fact that the LLM system prompt and the conversation history prefix must remain **completely unchanged** across turns. 
In `core/src/context.rs`, this is achieved by separating segments into `Stability::Stable` (which go into the frozen system prompt) and `Stability::Dynamic` / `Stability::SemiStable` (which are dynamically injected into the *last user message* via `<context_injection>`).

### 2. Impact of Modes (Build / Plan / Ask)
Agent modes change the permissions and the tools available to the agent.
- **Principles Segment (Segment 2)**: Updated based on the current mode (e.g., write permissions vs read-only). It is marked `Stability::Stable`.
- **Tool Catalog Segment (Segment 4)**: The tool registry removes tools (like `bash`, `write_file`) based on the mode. It is marked `Stability::Stable`.
- **Why it's Safe**: In `core/src/runtime/run.rs`, the `mode` is resolved during the instantiation of a `Run` (`Run::new`) and remains **immutable** for the duration of that run. The `ContextEngine` only sees a static `principles` string and a static `tool_catalog` during a single session. 
- **Tool Catalog Refresh Optimization**: Even though `refresh_context_segments` updates the tool catalog every turn, `Context::set_tool_catalog` includes a critical guard (`if seg.content == text { return; }`) which prevents unnecessary invalidations. Thus, the system prompt text does not drift, and KV Cache remains 100% stable within the mode.

### 3. Impact of Skills
The new skill support injects large chunks of context (catalog + active skill descriptions) into the prompt.
- **Loaded Skills (Segment 6)**: In `core/src/context.rs`, this is correctly registered with `Stability::Dynamic` and `RefreshPolicy::PerTurn` (with a budget of 2000 tokens).
- **Why it's Safe**: Because it is `Dynamic`, `assemble_system_prompt()` completely ignores it. Instead, `assemble_context_injection()` picks it up and appends it to the **last user message**. 
- The conversation prefix (System Prompt + all previous messages) remains perfectly identical to the previous turn. The LLM processes the loaded skills as part of the new user message (which is always a cache miss anyway), preserving the massive cache hit for the rest of the context.

## Summary

The architectural decision to separate the `ContextEngine` into `Stable` and `Dynamic` segments has paid off nicely. You successfully added two highly dynamic and structurally complex features (Modes and Skills) without incurring any KV cache degradation for local or remote models. No fixes are required.
