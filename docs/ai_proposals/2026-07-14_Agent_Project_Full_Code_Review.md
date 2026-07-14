# Agent Project Full Code Review

Date: 2026-07-14

## Scope

This review covers:

- Rust agent runtime and Tauri integration
- Run/event lifecycle and event replay
- Context construction, compaction, and model-bound message history
- Conversation and archival memory
- Subagent capability, permission, isolation, and persistence
- React session state, conversation restoration, autosave, rendering performance, and robustness

The CLI TUI implementation is intentionally excluded. Shared core code used by both TUI and the desktop application is included.

This is a repository-wide architecture and correctness review of the current tree, not a review of one commit range.

## Executive assessment

The project has a strong foundation: the runtime has explicit states, typed envelopes, per-run cancellation, process supervision, tool/subagent cleanup guards, structured context segments, hybrid memory retrieval, transactional SQLite writes, RAF-batched frontend events, and lazy chat rendering. The codebase is substantially beyond a toy agent loop.

It is not yet a solid production agent project, mainly because the authoritative state boundaries are unclear:

1. The backend owns the real model transcript during a run, but the frontend later reconstructs and overwrites it from presentation blocks.
2. The event log is described as the replay source of truth, but it is a best-effort subscriber to the same lossy broadcast channel as the UI.
3. Subagent capabilities are represented as strings and reconstructed, rather than derived as an enforced subset of the parent's concrete capability set.
4. Frontend replay and save state is partly global, while the runtime explicitly supports concurrent runs and sessions.

There are four release-blocking findings, followed by several high-priority correctness and robustness issues. Fixing isolated symptoms will not be enough; the first three architectural boundaries above should be made explicit.

## Priority summary

| Priority | Count | Meaning |
| --- | ---: | --- |
| P0 | 4 | Release blocker: data loss, security/isolation breach, or build failure |
| P1 | 10 | High-impact correctness or resilience failure under realistic use |
| P2 | 6 | Important maintainability, performance, or auditability issue |

## P0 findings

### P0-1: Resuming a long session and sending one message can silently delete old history

Locations:

- `app/src/features/chat/chatSlice.ts:686-705`
- `app/src/features/chat/chatSlice.ts:319-347`
- `app/src/features/chat/utils.ts:296-319`
- `core/src/session.rs:386-397`

On resume, all prompts are loaded into `allPrompts`, but only the last two prompts are rebuilt into `entries`, and `visiblePromptsCount` is set to two. When the user sends a new message, `userMessageSent` raises `visiblePromptsCount` to the full `allPrompts.length` without rebuilding the hidden prompts into `entries`.

`getFullMessagesForSession` then calculates zero invisible prompts and serializes only the currently materialized entries. The older hidden prompts are absent. The backend save path deletes every existing `session_messages` row and inserts this incomplete list, so the loss becomes permanent.

The retry reducer has the same shape at `chatSlice.ts:531-547`.

Required fix:

- Do not use a UI visibility count to decide persistence completeness.
- Keep an immutable/canonical transcript separate from the rendered window.
- Until that separation exists, append the new prompt without changing the hidden/visible boundary, or rebuild before any full save.
- Add a regression test: resume at least five prompts, send one new message, save, resume again, and assert byte-for-byte preservation of every old message.

### P0-2: The production frontend build currently fails

Location:

- `app/src/components/chat/turnHelpers.ts:107`

`npm run build` fails with TS2339 because `b.call_id` is accessed on `TurnBlock` after a boolean helper that does not narrow the union strongly enough.

The unit tests pass because Vitest transpilation does not provide the same production type-check gate.

Required fix:

- Make `isSubagentTool` a real type predicate, or perform the discriminant check inline.
- Make `npm run build` a required CI check, not only `vitest run`.

### P0-3: Batch subagents can acquire tools the parent does not have

Locations:

- `core/src/tools/subagent.rs:388-461`
- `core/src/tools/subagent.rs:1036-1058`
- `core/src/tools/mod.rs:306-345`

In the batch tool, when a task supplies a `tools` array, that requested array is assigned to the local variable named `available_tools`. The same array is passed both as the request and as the parent capability ceiling to `spawn_single`.

`spawn_single` therefore validates the request against itself. `ToolRegistry::from_names` then constructs concrete built-in tools such as `write_file`, `edit`, or `shell`, even if the parent registry did not contain them.

The single-subagent path normally passes the real parent list, but it also injects `read_file` after filtering if the intersection is empty. That fallback can likewise grant `read_file` to a parent that did not have it.

Required fix:

- Pass `self.available_tools` as an immutable capability ceiling in every path.
- Compute `requested ∩ parent_capabilities - meta_tools` exactly once.
- Respect an explicit empty list as no tools.
- Reject, rather than silently expand, an empty intersection.
- Add property tests asserting `child_capabilities ⊆ parent_capabilities` for single and batch spawning.

### P0-4: Subagents ignore the parent run's effective working directory

Locations:

- `core/src/runtime/tool_orchestrator.rs:588-597`
- `core/src/tools/subagent.rs:935-977`
- `core/src/tools/subagent.rs:1074-1083`

The orchestrator injects `_working_dir` from the active Run. `spawn_single` ignores it, reads the process-wide current directory, walks upward from that directory, and assigns the result to the child.

This breaks worktree isolation and multi-project correctness. A parent running in an isolated worktree can spawn a child that edits the main checkout. It also reads the local persona from the process directory rather than the run directory.

Required fix:

- Resolve the child working directory only from the parent's already-canonical effective working directory.
- Never widen from a worktree/project root to process CWD.
- Carry an explicit `ExecutionScope { root, cwd, session_id }` into tools and subagents rather than hidden JSON fields.
- Add a test that spawns from a temporary worktree and proves all child file and shell operations stay inside it.

## P1 findings

### P1-1: Session persistence destroys the model transcript's causal ordering

Locations:

- `app/src/features/chat/utils.ts:208-290`
- `app/src/features/chat/chatSlice.ts:185-229`
- `app/src-tauri/src/lib.rs:897-935`
- `core/src/session.rs:413-483`

A real agent prompt can contain multiple model iterations:

```text
user
assistant(tool_call A)
tool(A result)
assistant(tool_call B)
tool(B result)
assistant(final answer)
```

The frontend stores a presentation-oriented block list. `entriesToMessages` combines all assistant text, groups all tool calls into one assistant message, and emits all tool results afterward. On the next run, the backend loads a different sequence from the one the model actually produced.

This can confuse continuation quality and violate strict provider requirements for tool-call/result adjacency. Tool results are also truncated to 5,000 characters before being placed in UI blocks (`eventHandlers.ts:175-182`), so UI performance policy becomes model-history policy.

Required architectural fix:

- The runtime/backend transcript must be the canonical model history.
- Persist each model message and tool result as it is accepted by the runtime, with stable message IDs and ordering.
- Treat React blocks as a projection of canonical messages/events, never as the source used to rebuild model history.
- Store a separate display projection if needed; do not round-trip provider messages through it.

### P1-2: Event resync is neither ordered nor idempotent

Locations:

- `app/src/features/chat/eventHandlers.ts:739-749`
- `app/src/features/chat/chatSlice.ts:81-103`
- `core/src/runtime/event_log.rs:173-191`

If sequence 7 arrives after sequence 5, the frontend applies sequence 7 immediately and sets `lastSeq=7`, then requests replay from 5. Replay sends 6 and 7; sequence 6 regresses `lastSeq`, and sequence 7 is applied a second time. There is no applied `event_id` set and no rejection of stale/duplicate sequence numbers.

Many handlers append blocks, so duplicate application is observable.

Required fix:

- Maintain a per-run reorder buffer.
- Apply only the next contiguous sequence.
- Deduplicate by `(run_id, seq)` and preferably verify `event_id` consistency.
- Sort/validate replay data and fail explicitly on conflicting duplicate sequences.
- Make reducers idempotent for start/end events.

### P1-3: The event log is not a durable source of truth

Locations:

- `core/src/runtime/manager.rs:200-254`
- `core/src/runtime/manager.rs:290-318`
- `core/src/runtime/event_log.rs:101-129`
- `app/src-tauri/src/lib.rs:204-259`

`RunCreated` is broadcast before the logging subscriber is installed, so it is absent from the log and usually missed by new subscribers. The code already compensates for this in the frontend.

More importantly, logging subscribes to the same bounded broadcast channel as UI forwarding. On `Lagged(n)`, it logs a warning and permanently loses the events that replay is supposed to recover. It also performs synchronous open/write work for each event inside an async task. The Tauri forwarder adds a fixed 5 ms sleep per event and also discards lagged events.

Required architectural fix:

- Introduce one per-run event actor/writer that assigns sequence, appends durably, and then publishes.
- Broadcast must be a downstream view of the log, not the log's input.
- Batch/flushed writes are acceptable, but define the durability boundary.
- Remove the fixed per-event IPC sleep; batch envelopes for the frontend with bounded latency.

### P1-4: Atomic sequence allocation does not guarantee emission order

Locations:

- `core/src/runtime/run/mod.rs:408-429`
- `core/src/runtime/run/turn.rs:313-375`

Several concurrent closures call `fetch_add` and then independently call `broadcast.send`. One task can allocate sequence N, be descheduled, and send after another task sends N+1. A unique counter is not an ordering mechanism.

This can create false gap detection even without dropped events. The event actor proposed above should own both stamping and publication.

### P1-5: Recoverable notices, fatal errors, and successful completion are conflated

Locations:

- `core/src/runtime/run/turn.rs:117-129`
- `core/src/runtime/run/turn.rs:513-518`
- `core/src/runtime/run/lifecycle.rs:204-233`
- `core/src/runtime/run/lifecycle.rs:321-326`
- `app/src/features/chat/eventHandlers.ts:479-508`

Model exhaustion emits `RunEvent::Error` and returns `TurnOutcome::Stop`. `run_loop` converts `Stop` into `Ok`, so the runtime transitions to `Completed` and emits `RunCompleted`. Hitting the iteration limit follows the same path.

The frontend guesses whether an `Error` is recoverable by matching English phrases. Any new warning/retry text can incorrectly stop the spinner and trigger saves, while a genuine failed model run is ultimately marked completed.

Required fix:

- Replace generic `Error` with a typed event: severity, recoverable, code, phase, and user message.
- Reserve terminal events for exactly one terminal outcome.
- Model/provider exhaustion should produce `RunFailed`, or a separately named partial terminal state if that is a product decision.
- Do not infer lifecycle state from human-readable text.

### P1-6: Autosave can clear newer dirty state and mishandles concurrent sessions

Locations:

- `app/src/features/chat/chatSlice.ts:407-417`
- `app/src/features/chat/chatSlice.ts:730-733`
- `app/src/features/project/projectSlice.ts:267-303`
- `app/src/store.ts:35-105`
- `app/src/hooks/useAutoSaveSession.ts:18-37`

The save queue correctly serializes backend writes, but `chatSlice` clears `isDirty` for every fulfilled save without comparing the saved revision with the current revision. If new events arrive while an older snapshot is saving, fulfillment clears the new dirty flag.

An RAF event batch may contain multiple sessions, but the middleware selects only the first session for intermediate save. The terminal autosave hook watches only the active session, so a background session can complete without a final canonical save.

Required fix:

- Track a monotonically increasing per-session content revision.
- Include `savedRevision` in the thunk result and clear dirty only when it equals current revision.
- Partition each event batch by run/session and process persistence independently.
- Force and await a final save for every terminal run, whether active or background.

### P1-7: Mid-turn crash snapshots are written but never restored

Locations:

- `core/src/runtime/run/mod.rs:300-304`
- `core/src/runtime/run/turn.rs:1050-1102`
- `core/src/session.rs:949-955`

The runtime carefully writes generation-protected `sessions/{id}.messages.json` snapshots, but no resume/startup path reads them. The only other production reference is deletion cleanup.

As a result, the snapshot does not provide crash recovery. After a crash, prompt status can be repaired, but the canonical mid-turn messages and tool results are lost.

Required fix:

- On startup/resume, compare snapshot generation/time against SQLite state.
- Validate the snapshot schema and tool-call pairing before recovery.
- Reconcile it transactionally into the canonical transcript or expose an explicit recovered/interrupted turn.

### P1-8: Permission patterns are only partially enforced

Locations:

- `core/src/permission/types.rs:203-220`
- `core/src/permission/mod.rs:348-371`
- `core/src/permission/mod.rs:452-570`
- `core/src/permission/whitelist.rs:58-89`
- `core/src/runtime/tool_orchestrator.rs:78-105`

Patterns declare tool, command, paths, hosts, and `max_danger`, but:

- blacklist/config/builtin matching does not consistently evaluate hosts;
- `max_danger` is not used for decisions;
- a constrained dimension is skipped when extraction returns `None`, which can turn a scoped approval into a tool-wide match;
- only one path-like field is extracted, so multi-path operations, nested arguments, source/destination pairs, and arrays are not covered.

This makes policy configuration appear more precise than its real enforcement.

Required fix:

- Centralize `ToolInvocationScope` extraction per tool schema.
- A pattern that constrains a dimension must fail if that dimension is missing.
- Evaluate all declared dimensions and danger bounds in one matcher shared by blacklist, whitelist, config, and builtins.
- Add adversarial tests for absent host/path, arrays, multiple paths, URL parsing, and malformed arguments.

### P1-9: Permission path checks use process CWD, not the Run's effective CWD

Locations:

- `core/src/permission/command_analysis.inc.rs:8-32`
- `core/src/runtime/tool_orchestrator.rs:78-105`
- `core/src/runtime/tool_orchestrator.rs:588-597`

Relative paths are canonicalized against process CWD during permission checking. The Run's `_working_dir` is injected only later, during tool execution. The same relative path can therefore be approved against one root and executed against another.

Required fix:

- Resolve and canonicalize the full invocation using the effective Run cwd before policy evaluation.
- Pass the resolved scope to the tool so checking and execution use the same object.
- For a hard security boundary, complement application checks with OS-level sandboxing or descriptor-relative file operations to reduce symlink/TOCTOU risk.

### P1-10: Stream retry and cancellation can leave runs unresponsive for minutes

Locations:

- `core/src/runtime/run/turn.rs:526-674`
- `core/src/runtime/run/turn.rs:748-753`

The inner retry loop permits ten exponential delays from one to 512 seconds, and the outer recovery loop can repeat the process. Sleeps are not cancellation-aware. `stream.next().await` is also not selected against cancellation or an idle timeout, so a stalled connection that produces no next item can keep the run alive indefinitely.

Required fix:

- Use `tokio::select!` for cancellation, stream progress, and idle timeout.
- Add jitter, a maximum delay, and a total retry deadline/budget.
- Emit structured retry events rather than `Error` strings.

## P2 findings

### P2-1: Dynamic context has no aggregate hard budget

Locations:

- `core/src/context.rs:303-324`
- `core/src/context.rs:459-489`
- `core/src/context.rs:663-705`
- `core/src/runtime/run/compact.rs:42-87`

The loaded-skills segment explicitly uses unlimited tokens, and dynamic injection has no aggregate cap. Compaction removes/summarizes conversation history, but it cannot solve an injection that is oversized by itself. In that case `prepare_summary` may have nothing useful to summarize and the request remains too large.

The injection is also sent as a synthetic user message, mixing runtime-owned instructions, memory, project text, and user-role semantics. This makes provenance and trust priority hard to reason about.

Required fix:

- Add an aggregate context budget ledger with per-source and per-item hard caps.
- Budget catalog metadata separately from selected skill bodies.
- Track provenance and trust level for every injected fragment.
- Use a provider-appropriate system/developer channel or a clearly typed contextual fragment instead of pretending it is user input.

### P2-2: Memory maintenance scheduling does not match its comments

Locations:

- `core/src/runtime/run/turn.rs:196-255`

`turn_index` resets for every Run. Consolidation uses `turn_index % 20 == 0`, so a normal one-turn Run consolidates every time. Lifecycle uses `turn_index > 0 && turn_index % 40 == 0`, so it almost never executes unless one Run happens to finish exactly on that internal iteration.

Required fix:

- Use a persistent per-session/global completed-turn counter or a background scheduler.
- Make maintenance idempotent and observable, with last-run time and result metrics.

### P2-3: Embedding work is performed while holding the memory mutex

Locations:

- `core/src/runtime/run/lifecycle.rs:108-127`
- `core/src/runtime/run/turn.rs:168-188`
- `core/src/runtime/run/context.rs:343-350`

Comments say embeddings are computed outside the lock, but the guard remains alive while `embed_single` executes. This serializes memory operations around relatively expensive inference. The recall path has the same issue.

Required fix:

- Clone an `Arc` to the embedding model while locked, drop the guard explicitly, then embed.
- Keep DB/index mutation under a narrow lock or split storage/index synchronization further.

### P2-4: Recall over-fetches globally and filters by session afterward

Location:

- `core/src/runtime/run/context.rs:356-376`

Auto recall fetches the global top 12, then removes results from other sessions. Relevant current-session results ranked below unrelated global entries are lost. This can make recall appear randomly ineffective as the database grows.

Required fix:

- Filter by session during candidate retrieval/index search.
- If cross-session recall is desired, make it a separate explicit policy with provenance labels and privacy controls.

### P2-5: React entry lookup becomes quadratic over long-lived sessions

Locations:

- `app/src/features/chat/selectors.ts:22-43`
- `app/src/components/chat/LazyEntry.tsx:34-37`
- `app/src/components/chat/EntryRow.tsx:17-25`

Every mounted `EntryRow` searches the active entries array by ID. Because the selector depends on the session entries container, every streaming batch reruns one linear search per mounted entry. `LazyEntry` intentionally never unmounts an entry after it becomes visible, so a long-scrolled session trends toward O(N²) selector work per update even if unchanged rows do not rerender.

Required fix:

- Normalize state as `entryIdsBySession` plus `entriesById`, or maintain an ID index.
- Keep the streaming block in a small independently selected entity.
- Consider real windowing for very long sessions rather than permanent mounting.

### P2-6: Subagent audit/session persistence cannot reliably reconstruct the graph

Locations:

- `core/src/session.rs:320-355`
- `core/src/tools/subagent.rs:703-736`
- `core/src/subagent/mod.rs:777-785`

`save_subagent_with_messages` saves `parent_session_id=None`, so subagent sessions are orphaned despite the schema supporting a parent relationship. File names use agent ID plus whole seconds, so repeated or duplicate IDs within one second can overwrite histories. `into_messages()` returns `context.messages()`, which includes synthetic context injection rather than only the canonical raw child transcript.

Required fix:

- Persist `parent_session_id`, parent run ID, parent call ID, and a unique child run ID.
- Use UUID/event ID filenames or, preferably, the canonical session/event store.
- Persist raw child model messages and context provenance separately.

## Additional observations

### Tauri command robustness

- `send_message` ignores `switch_model` errors at `app/src-tauri/src/lib.rs:105-107`, so UI and runtime can disagree about the selected model.
- A prompt is created before run creation/start at `app/src-tauri/src/lib.rs:151-199`; failure after prompt creation leaves it running until startup repair.
- Prompt finalization depends on receiving the terminal broadcast event at `app/src-tauri/src/lib.rs:237-252`; a lagged/missed terminal event can leave stale state.
- Synchronous session resume is performed while holding the async RunManager mutex at `app/src-tauri/src/lib.rs:103-121`. Move storage I/O outside that lock and into `spawn_blocking`.

### Frontend per-run state

`resyncing`, `_pendingGap`, and `cacheMetrics` are global fields in `ChatState`, although concurrent runs are supported. One run's gap or metrics can overwrite another's. `runIdToSessionId` and `lastSeqByRun` are not fully cleaned on terminal/delete, so long-running desktop sessions accumulate stale mappings.

Make all lifecycle/replay/metrics fields keyed by run ID, with explicit terminal cleanup and a bounded tombstone/dedup cache.

### What is already good

- `Envelope` carries run/session/turn/parent-call identity, sequence, event ID, and timestamp.
- Run states and explicit terminal variants form a good base for a state machine.
- RAII guards for tools/subagents and the process supervisor reduce orphaned UI and process state.
- Per-Run approval resolvers avoid the main actor deadlock for main-agent approvals.
- Session replacement is transactional, which prevents half-written DELETE/INSERT corruption.
- Context hygiene, tool-call pairing preservation during compaction, and cache telemetry are thoughtfully tested.
- Memory has useful building blocks: BM25, vector search, RRF, salience, reflection, and lifecycle concepts.
- React batches high-frequency events per animation frame and defers expensive off-screen rendering.
- The frontend save queue prevents overlapping SQLite replacement writes from committing out of order.

These strengths should be retained while changing the ownership boundaries.

## Recommended target architecture

### 1. Canonical transcript owned by the backend

Use one append-only transcript per session/run containing exact provider-facing messages:

```text
Session
  Prompt/Run
    ModelMessage
    ToolCall
    ToolResult
    ModelMessage
```

Each record should have stable IDs, parent relationships, ordering, status, and timestamps. React receives a display projection and stores only ephemeral UI state such as expanded panels and render windows. It never rewrites canonical messages.

### 2. Durable event writer before fan-out

```text
Runtime producers
      |
      v
Per-run EventWriter (single ordered mailbox)
      |
      +--> durable append/WAL
      |
      +--> UI broadcast / batch IPC
      |
      +--> metrics / reflection consumers
```

The writer owns sequence allocation. Consumers can lag without corrupting the durable stream. Replay folds from the last contiguous applied sequence and is idempotent.

### 3. Typed capability and execution scope

Replace string-list reconstruction with a sealed object:

```text
Parent CapabilitySet + ExecutionScope
                 |
                 v
       validated subset operation
                 |
                 v
Child CapabilitySet + narrowed ExecutionScope
```

Tool permission checks and execution receive the same resolved invocation scope. Subagents cannot widen tools, cwd, sandbox roots, network hosts, approval mode, iteration budget, or concurrency budget.

### 4. Explicit context budget and provenance

Every context fragment should carry:

- source and trust level
- stable/dynamic policy
- per-item cap
- segment cap
- aggregate request budget
- truncation/summarization strategy
- whether it is persisted in transcript or reconstructed each turn

This makes context behavior testable instead of depending on string concatenation conventions.

### 5. Per-session revisioned frontend projection

Normalize entries and blocks by ID, key replay state by run ID, and maintain `contentRevision`/`persistedRevision` per session. Terminal events force persistence for their own session, independent of which session is active.

## Remediation order

### Phase 0: Stop release blockers

1. Fix the TypeScript build.
2. Fix resumed-session data loss and add the end-to-end regression test.
3. Fix batch/single subagent capability subset enforcement.
4. Propagate the parent's effective working directory into every subagent.

### Phase 1: Establish authoritative data flows

1. Make backend runtime transcripts canonical.
2. Add the single ordered durable event writer.
3. Implement idempotent per-run replay/reordering in React.
4. Add session revisions and terminal per-session persistence.

### Phase 2: Harden lifecycle and permissions

1. Introduce typed error/retry/terminal semantics.
2. Make cancellation and retry deadlines bounded.
3. Centralize full-dimensional invocation extraction/matching.
4. Restore and reconcile crash snapshots.

### Phase 3: Context, memory, and performance

1. Add aggregate context budgets and provenance.
2. Move memory maintenance to persistent scheduling.
3. Remove embedding-under-lock and query recall with session filters.
4. Normalize frontend entry storage and add true long-session windowing.

## Required test matrix

The current unit suite is broad, but the missing tests are mostly cross-layer invariants:

1. Resume 5+ prompts -> send -> save -> resume: no message loss.
2. Multi-iteration tool transcript round trip: exact role/order/tool-call equality.
3. Duplicate, stale, out-of-order, and gapped events: deterministic idempotent projection.
4. Event-log subscriber lag: durable replay still contains every sequence.
5. Concurrent runs in different sessions: saves, gaps, metrics, and terminal cleanup remain isolated.
6. Single and batch subagent capability property: child is always a subset.
7. Worktree subagent: no access or writes through the main checkout path.
8. Permission pattern dimensions: missing/multiple path/host/command inputs fail closed.
9. Cancel during retry sleep and a stalled SSE stream: bounded shutdown latency.
10. Crash after tool result but before terminal: snapshot recovery preserves valid transcript.
11. Save revision race: old fulfillment cannot clear new dirty state.
12. Long-session render benchmark: bounded work per streaming frame.

## Verification performed

- `cargo test -p agent_core`: passed outside the restricted sandbox: 496 unit tests and 4 integration tests passed; one doc test ignored.
- The first sandboxed Rust run had two environment-only failures because tests bind a local port and write under the real user data directory; both passed outside the sandbox.
- `cargo check -p app`: passed with warnings.
- `npm test`: passed, 4 files / 28 tests.
- `npm run build`: failed at `app/src/components/chat/turnHelpers.ts:107` with TS2339.

## Final judgment

The runtime concepts are good, and many local components are thoughtfully implemented. The system's main weakness is not lack of features; it is duplicated ownership of truth. The model transcript, event trace, permission scope, and frontend session projection each need one authoritative owner and explicit derived views.

After the four P0 fixes, the highest-leverage work is the canonical backend transcript plus durable ordered event writer. Those two changes remove a large class of session restoration, replay, autosave, and context bugs at once. Without them, additional frontend patches will continue to protect one path while another path silently diverges.

## Implementation follow-up

The remediation plan above was implemented on 2026-07-14. Compatibility with the removed frontend-owned persistence path was intentionally not retained.

### Authoritative state and session restoration

- The backend runtime transcript is now the only canonical conversation history. React entries are a display projection and can no longer overwrite provider messages.
- Runtime snapshots are written when the user message enters the run and again at terminal persistence boundaries. Resume promotes a newer valid snapshot and rejects causally invalid tool-call/result sequences.
- Retry rewinds the canonical transcript at the selected prompt in the backend before starting the replacement run.
- Restoration folds every assistant/tool iteration in canonical order instead of collapsing a prompt to one assistant response.
- The two frontend save hooks, project save thunk, sidebar save calls, and Tauri UI-transcript save command were removed.

### Event lifecycle

- One per-run writer now owns sequence allocation, append-before-publish ordering, and broadcast. `RunCreated` follows the same path.
- Event-log append failures fail and cancel the run. Replay sorts events, deduplicates identical records, and rejects conflicting duplicate sequences.
- React maintains per-run contiguous reorder buffers, rejects stale events, deduplicates by event ID, and bounds replay/tombstone state.
- Recoverable runtime information is represented by typed `Notice { code, severity, recoverable, message }`. `Error` is fatal; model exhaustion and iteration-limit exhaustion now produce `RunFailed`, never `RunCompleted`.
- Model streaming has cancellation-aware retry waits, a bounded retry count/delay, and a 90-second idle deadline.

### Permission and subagent isolation

- Child tools are computed as a strict subset of the parent capability set. Explicit empty sets remain empty; omitted sets inherit only non-meta parent tools.
- The parent effective working directory is carried into every child without walking upward or falling back to process CWD.
- Permission matching now evaluates all invocation paths, normalized hosts, commands, and maximum danger level. Constrained-but-missing dimensions fail closed.
- Approval waiters are scoped by parent run and prompt, removing cross-run prompt-ID collisions.
- Subagent transcripts use collision-resistant IDs and persist full messages plus parent session/run/call lineage in SQLite.

### Context and memory

- Loaded skills and dynamic context have aggregate token budgets in addition to per-item caps.
- Reconstructed context is injected as system context with explicit source/trust headers, not as a synthetic user message.
- Embedding calls no longer hold the memory mutex across `await`.
- Session filters are applied during recall candidate collection rather than after global top-k truncation.
- Memory maintenance cadence is based on a persisted assistant-message count and survives new `Memory` instances and process restarts.

### Frontend robustness and performance

- Long-session resume no longer changes the hidden-history boundary when a new prompt is appended.
- Entry lookup uses a memoized ID index instead of repeated linear searches.
- Chat entries outside the viewport are genuinely unmounted and replaced by measured placeholders.
- Shiki is loaded dynamically; the initial application chunk is below 500 KB in the verified production build. Large language grammars and WASM remain lazy chunks.

### Post-remediation verification

- `cargo test -p agent_core`: 509 unit tests and 4 integration tests passed after all changes; one doc test ignored.
- `cargo check -p app`: passed after the final Rust/Tauri changes.
- Focused capability tests: 4 passed.
- Focused invocation/approval-scope tests: 3 passed.
- `npm test -- --run`: 4 files / 28 tests passed. Two obsolete tests for the removed frontend transcript serializer were deleted with that compatibility path.
- `npm run build -- --logLevel warn`: passed (`tsc` and Vite production build).
