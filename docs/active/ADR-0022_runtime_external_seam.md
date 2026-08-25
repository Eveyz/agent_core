# ADR-0022: Runtime External Seam

```yaml
---
id: ADR-0022
type: ADR
title: Runtime External Seam
status: Draft
author: agent_core
created: 2026-08-21
updated: 2026-08-21
reviewers: [zniverse]
related: [ADR-0019, PLAN-0014]
supersedes: ~
superseded_by: ~
tags: [runtime, seam, run-manager, agent-loop]
---
```

## Context

`RunManager` advertises `create_run` / `command` / `subscribe`, but callers (Tauri, CLI, gateway) also poke `Brain` fields, bypass the command mailbox for approve/steer/cancel, and parse slash commands inside `Run::run()`. `Run` and `Subagent` each own a turn loop. Complexity has leaked out of the module.

## Decision

- The external seam is `RunManager`. `Brain` is a factory: its fields are crate-private; other crates use methods. `RunHandle` interaction ports are crate-private.
- Lifecycle commands (Start, Pause, Resume, Cancel, FollowUp) use the mailbox. Interaction (approve, answer, steer) is a `RunManager` method. Callers never touch resolvers or cancel tokens.
- Slash strings are not a `Run` invariant. Callers parse `UserIntent`; `Run` only consumes the enum.
- `Run` (Interactive) and `Subagent` (Nested) share one `AgentLoop` turn body. Nested does not gain steer, dual-track transcript, slash, or `ask_user`.
- Workflow stays on the ADR-0019 seam. `AgentEvent` is an in-crate tool dialect, not a public harness contract.

## Consequences

### Positive

- Tauri / CLI / eval learn one interface.
- Compact, stream retry, and tool scheduling are fixed in one place.
- Slash and product UX can change without editing the execution kernel.

### Negative / Trade-offs

- Composition roots still need `Arc<Brain>` for `CustomAgentRunner`.
- Nested keeps `trim_to_fit` instead of LLM compaction, so long subagent tasks may still overflow.

## Alternatives Considered

| Alternative | Pros | Cons | Decision |
|-------------|------|------|----------|
| Fold workflow into `Run` | Fewer types | Couples prompts to DAG orchestration | Rejected (ADR-0019) |
| Keep Brain fields public | Less churn | Callers treat Brain as a service locator | Rejected |
| Third loop adapter for CustomAgent | Symmetric naming | CustomAgent already wraps Subagent | Rejected |

## Implementation Notes

- Intent: `core/src/runtime/intent.rs`
- Request: `RunManager::create_run_from(CreateRunRequest)`
- Loop: `core/src/runtime/agent_loop.rs` (`apply_compact`, `run_model_phase`, `collect_model_stream`)
- Tests at `parse_user_intent`, `RunManager`, and `AgentLoop`; eval `contract_v1` is the integration surface

## References

- ADR-0019 Workflow Runtime Seam
- PLAN-0014 Dual-track transcript

## Change Log

| Date | Author | Change |
|------|--------|--------|
| 2026-08-21 | agent_core | Interactive and Nested now share compact + SSE collect/retry in AgentLoop |
