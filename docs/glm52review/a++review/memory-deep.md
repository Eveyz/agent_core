I'll start by exploring the memory system directory structure and then analyze each file systematically.
Let me read all the memory system files in parallel.
Now let me read the remaining files:
Let me read the remaining parts of key files:
Now let me read the remaining parts and the agent_registry:
Now let me search for specific patterns across all files systematically:
Now let me search for lock contention points, async/blocking, and SQL patterns:
Now let me check how memory tools interact with async and check the reflection daemon's blocking operations:
Now let me check for the `add_column_if_not_exists` usage and the SQL injection pattern in storage.rs:
Now I have a comprehensive understanding of the entire codebase. Let me compile the structured report.

---

# Deep-Dive Analysis: Memory System & Related Modules

## Executive Summary

The memory system is architecturally sound (Ebbinghaus decay, RRF fusion, BM25+HNSW hybrid pipeline) but has **critical correctness bugs**, **pervasive lock contention** from a single global `Mutex<Connection>`, **blocking synchronous I/O in async contexts**, **unbounded in-memory growth**, and **dangerous `unwrap()` calls after DB writes** that can panic in production. Below is the file-by-file breakdown.

---

## 1. `core/src/memory/mod.rs` — MemoryManager

### (1) unwrap()/expect() in production paths
| Line | Code | Severity |
|------|------|----------|
| **400** | `let bm25 = self.bm25.as_ref().unwrap();` | 🔴 HIGH — Panics if called directly (not via `search_conversation`). Only guarded by `is_some()` in the caller. |
| **401** | `let hnsw = self.hnsw.as_ref().unwrap();` | 🔴 HIGH — Same issue. |
| **503** | `let bm25 = self.bm25.as_ref().unwrap();` | 🔴 HIGH — In `search_conversation_hybrid_precomputed`. |
| **504** | `let hnsw = self.hnsw.as_ref().unwrap();` | 🔴 HIGH — Same. |

**A++ Recommendation:** Replace all four with `ok_or_else(|| anyhow!("BM25/HNSW not configured"))`?—?or make the methods private and only callable from `search_conversation` which already checks `is_some()`.

### (2) Lock contention points
- **Global `Arc<Mutex<MemoryManager>>`** (brain.rs:41, tools/*): Every read, write, search, and consolidation serializes through ONE mutex. Search operations hold the lock for 10?50ms (embedding) + DB I/O.
- **`search_conversation_bm25_with_salience`** (line 345?386): Acquires `storage_conn()` (the global DB mutex) inside the MemoryManager mutex. Carefully scoped to drop before `bump_strength_batch` (which re-acquires it)?—?**fragile pattern, easy to break in maintenance**.
- **`store_conversation`** (line 163): Acquires storage lock (via `recall.store`), then BM25 writer lock, then HNSW write lock?—?three locks in sequence.

**A++ Recommendation:** 
1. Split `MemoryManager` into `Arc<RwLock<MemoryManager>>`?—?reads (search) use `.read()`, writes (store) use `.write()`.
2. Use a connection pool (e.g., `r2d2` + `r2d2-sqlite`) instead of a single `Mutex<Connection>`. This eliminates the fundamental bottleneck.
3. Pre-compute embeddings outside the lock (the `search_conversation_precomputed` pattern is correct?—?make it the default).

### (3) Blocking operations in async contexts
- **Line 407**: `model.embed_single(query).unwrap_or_default()` in `search_conversation_hybrid`?—?synchronous 10?50ms embedding call while holding the MemoryManager mutex. Blocks all other memory operations.
- **Line 175**: Same in `store_conversation` for HNSW sync.

**A++ Recommendation:** Deprecate `search_conversation_hybrid` in favor of `search_conversation_precomputed`. Embed the query *before* acquiring the lock.

### (4) SQL injection risks
- **Lines 254, 329, 439, 532, 657**: Dynamic `IN (?, ?1, ?2, ...)` clause built via `format!()`. The placeholders are positional parameters (safe), and values are bound via `param_refs`. **No injection**, but the pattern is fragile?—?if `bm25_ids`/`candidates` is empty, `IN ()` produces invalid SQL (guarded by early returns, but brittle).
- **storage.rs:405, 412**: `format!("PRAGMA table_info({})", table)` and `format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition)`?—?**table/column/definition are interpolated directly into SQL**. The function is `pub`, so if any caller passes user input, it's injectable.

**A++ Recommendation:** Validate `table`/`column` against a whitelist regex (`^[a-zA-Z_][a-zA-Z0-9_]*$`) in `add_column_if_not_exists`. Use `rusqlite::Connection::prepare` with parameterized DDL where possible (SQLite doesn't support parameterized DDL, so validation is the only option).

### (5) Memory leak risks
- **HNSW fallback** (hnsw.rs:75?78): Unbounded `Vec<(String, Vec<f32>)>`. Grows on every `store_conversation`. Only cleared on restart. **Long-running processes will leak memory.**
- **BM25 index**: In-memory, grows with every insert. No eviction.
- **`recall_memory` table**: No automatic pruning. `prune_cold_memories` exists but is never called automatically.
- **`session_event_log`**: No cleanup. Grows per session indefinitely.
- **`conversation_summaries`**: No cleanup mechanism.

**A++ Recommendation:**
1. Add a size cap to HNSW fallback (e.g., 10,000 entries). When exceeded, trigger a background rebuild or evict oldest.
2. Add a background timer that calls `prune_cold_memories` every N hours.
3. Add `DELETE FROM session_event_log WHERE session_id = ? AND id < (SELECT MAX(id) - 1000 FROM session_event_log WHERE session_id = ?)` to cap event log per session.

### (6) Error handling gaps
| Line | Code | Issue |
|------|------|-------|
| **169** | `let _ = bm25.insert(&id, content);` | Silently drops BM25 insert failure. Index becomes inconsistent. |
| **175** | `model.embed_single(content).unwrap_or_default()` | Embedding failure returns empty vec?—?HNSW gets garbage. |
| **200** | `let _ = self.recall.bump_strength_batch(&ids);` | Silently ignores reinforcement failure. |
| **240** | `bm25.search(query, 150).unwrap_or_default()` | Search failure returns empty results (silent data loss). |
| **389, 490, 583** | `let _ = self.recall.bump_strength_batch(&id_refs);` | Same pattern, 3 occurrences. |

**A++ Recommendation:** Log warnings on all silently-ignored errors: `tracing::warn!("BM25 insert failed: {e}");`. For embedding failures, skip HNSW sync rather than inserting a zero vector.

### (7) Missing test coverage
- ❌ No tests for `search_conversation_bm25`, `search_conversation_bm25_with_salience`, `search_conversation_hybrid`, `search_conversation_precomputed`, `search_conversation_filtered`.
- ❌ No tests for BM25/HNSW sync in `store_conversation`.
- ❌ No tests for `consolidate()`.
- Only 2 basic smoke tests exist.

**A++ Recommendation:** Add tests with a mock BM25+HNSW index: test hybrid search ranking, test fallback when BM25 returns empty, test that `store_conversation` syncs to BM25.

### (8) Performance bottlenecks
- **`bump_strength_batch`** (recall.rs:482): N+1 queries (1 SELECT + N UPDATEs per record). Should be a single `UPDATE ... SET memory_strength = ... WHERE id IN (...)` with computed values.
- **`search_conversation_hybrid`**: Embedding call (10?50ms) inside the lock. 
- **Dynamic SQL rebuilding**: The `format!("... IN ({})", placeholders)` pattern is repeated 6+ times. `prepare_cached` won't cache because the SQL string changes with the number of IDs.

---

## 2. `core/src/memory/storage.rs` — Storage

### (1) unwrap()/expect()
None in production paths.

### (2) Lock contention
- **`Arc<Mutex<Connection>>`** (line 9): **THE** fundamental bottleneck. Every single DB operation across ALL modules (recall, archival, block, session, agent_registry, consolidation) serializes through this one mutex. This is the #1 performance issue in the entire system.

**A++ Recommendation:** Replace with `r2d2::Pool<SqliteConnectionManager>` (connection pool). Allows concurrent reads (WAL mode already supports this) and eliminates lock contention entirely. Alternatively, use `RwLock<Connection>`?—?SQLite in WAL mode supports concurrent readers.

### (4) SQL injection risks
- **Line 405**: `format!("PRAGMA table_info({})", table)`?—?table name interpolated.
- **Line 412**: `format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition)`?—?all three interpolated.

**A++ Recommendation:** Add input validation:
```rust
fn validate_identifier(s: &str) -> Result<()> {
    if !s.chars().all(|c| c.is_alphanumeric() || c == '_') || s.is_empty() {
        bail!("invalid SQL identifier: {}", s);
    }
    Ok(())
}
```

### (6) Error handling gaps
- **Line 383**: `let _ = db.execute_batch(migration);`?—?silently ignores migration failures. If a migration fails (e.g., disk full), the DB is in an inconsistent state and no error is reported.

**A++ Recommendation:** Log migration results: `if let Err(e) = db.execute_batch(migration) { tracing::debug!("migration skipped: {e}"); }`.

### (8) Performance bottlenecks
- **Line 31**: `PRAGMA journal_mode=WAL` is set but **`PRAGMA synchronous=NORMAL`** is missing. Default `synchronous=FULL` does an fsync on every commit?—?2?10x slower than needed for WAL mode.
- Missing: `PRAGMA cache_size`, `PRAGMA temp_store=MEMORY`, `PRAGMA mmap_size`.

**A++ Recommendation:** Add: `PRAGMA synchronous=NORMAL; PRAGMA cache_size=-64000; PRAGMA temp_store=MEMORY; PRAGMA mmap_size=268435456;`

---

## 3. `core/src/memory/recall.rs` — RecallMemory

### (1) unwrap()/expect()
None in production paths. ✅

### (6) Error handling gaps
| Line | Code | Issue |
|------|------|-------|
| **467** | `.unwrap_or(1.0)` | `bump_strength` silently defaults to 1.0 if record not found. |
| **496** | `.unwrap_or(1.0)` | Same in batch. |
| **500** | `let _ = db.execute(...)` | UPDATE failure silently ignored in batch bump. |
| **561, 564, 571** | `unwrap_or(...)` in `stats()` | Acceptable defaults. |

### (7) Missing test coverage
- ❌ No tests for `search_by_vector`, `search_by_keyword`, `search_scored`, `bump_strength`, `bump_strength_batch`, `prune_cold_memories`, `promote_to_archival`, `search_by_date`, `stats`, `collect_candidates`, `build_fts_query`.

**A++ Recommendation:** Add integration tests with a temp DB: store 100 records, verify search ranking, test bump_strength increments, test prune deletes cold records.

### (8) Performance bottlenecks
- **`bump_strength_batch`** (line 482): N+1 query pattern. Should use: `UPDATE recall_memory SET memory_strength = memory_strength * ? + ?, access_count = access_count + 1, last_accessed_at = ? WHERE id IN (...)`.
- **`search_by_vector`** (line 283): Loads all candidate embeddings into memory, computes cosine similarity in Rust. For 150 candidates × 384 dims, this is fine. But `prepare(&sql)` (not `prepare_cached`) means the statement is recompiled every call.
- **`collect_candidates`** (line 168): Two separate queries (FTS5 + recent) merged in Rust. Could be a single `UNION` query.

---

## 4. `core/src/memory/archival.rs`

### (6) Error handling gaps
- **Lines 124?132, 136?144**: Multiple `if let Ok(...)` patterns that silently skip FTS5 query failures and row-read errors. If FTS5 is corrupted, search silently returns fewer results.

### (7) Missing test coverage
- ❌ **Zero tests.** No tests for `insert`, `insert_with_embedding`, `search`, `search_by_vector`, `search_by_keyword`, `delete`.

**A++ Recommendation:** Add basic CRUD + search tests mirroring the mod.rs test pattern.

### (8) Performance bottlenecks
- **Line 165**: `db.prepare(&sql)`?—?not `prepare_cached`. Recompiles every call.
- **Line 185**: `prepare` (not cached) for the candidate loading query.

---

## 5. `core/src/memory/block.rs` — CoreMemory

### (1) unwrap()/expect() in production paths
| Line | Code | Severity |
|------|------|----------|
| **96** | `self.blocks.get_mut(id).unwrap().content = new_content;` | 🔴 CRITICAL — Panics if in-memory map doesn't have the key AFTER the DB write succeeded. Leaves DB and memory inconsistent. |
| **97** | `self.blocks.get_mut(id).unwrap().updated_at = now;` | 🔴 CRITICAL — Same. |
| **125** | `self.blocks.get_mut(id).unwrap().content = replaced;` | 🔴 CRITICAL — Same pattern in `replace`. |
| **126** | `self.blocks.get_mut(id).unwrap().updated_at = now;` | 🔴 CRITICAL — Same. |

**A++ Recommendation:** Use `if let Some(block) = self.blocks.get_mut(id)` and log an error if missing (shouldn't happen, but don't panic after a successful DB write). Better: update the in-memory state *before* the DB write, or use a transaction.

### (2) Lock contention
- `CoreMemory` holds a `Storage` clone (global mutex). `append`/`replace`/`create` acquire the DB lock. Since `CoreMemory` is accessed via `memory.core_mut()` which requires the `MemoryManager` mutex, this is **nested locking**: MemoryManager mutex → Storage mutex.

### (7) Missing test coverage
- ❌ **Zero tests.** No tests for `new`, `load`, `append`, `replace`, `create`, `list`, `to_context_string`, `has`.

**A++ Recommendation:** Add tests: create block, append content, replace content, verify max_chars enforcement, test load creates default blocks.

---

## 6. `core/src/memory/bm25.rs` — BM25Index

### (2) Lock contention
- **`Arc<Mutex<IndexWriter>>`** (line 27): All writes (insert/delete) serialize. Search doesn't need the writer lock (creates its own reader).

### (8) Performance bottlenecks
- **Line 70**: `writer.commit()` after every single `insert` call?—?extremely expensive for batch inserts. Each commit fsyncs.
- **Lines 86?90**: `search` creates a new `IndexReader` and `Searcher` on every call. Tantivy readers are designed to be reused.
- **Line 53**: `from_records` correctly commits once?—?good.

**A++ Recommendation:**
1. Cache the reader: `reader: Arc<IndexReader>` initialized in `new()`/`from_records()`. Call `reader.reload()` after writes.
2. For `insert`, consider deferred commits (commit every N inserts or on a timer).
3. Add `insert_batch(&[(String, String)])` for bulk operations.

### (7) Missing test coverage
- ❌ **Zero tests.**

---

## 7. `core/src/memory/hnsw.rs` — HNSWIndex

### (1) unwrap()/expect() in production paths
| Line | Code | Severity |
|------|------|----------|
| **76** | `self.fallback.write().expect("HNSW fallback lock poisoned")` | 🟡 MEDIUM — Panics on lock poisoning (thread panic). |
| **83** | `self.map.read().expect("HNSW lock poisoned")` | 🟡 MEDIUM — Same. |
| **96** | `self.fallback.read().expect("HNSW fallback lock poisoned")` | 🟡 MEDIUM — Same. |

**A++ Recommendation:** Use `unwrap_or_else` with a logged fallback, or switch to `parking_lot::RwLock` which doesn't poison.

### (5) Memory leak risks
- **Line 75?78**: `fallback: Vec<(String, Vec<f32>)>` grows **unbounded**. Every `store_conversation` call adds one entry. No eviction. For a long-running agent making 1000 conversations/day, this is ~1.5MB/day (384 dims × 4 bytes × 1000).

**A++ Recommendation:** Cap at a configurable maximum (e.g., 5000). When exceeded, either trigger a rebuild or evict oldest entries. Add a `fallback_len()` method for monitoring.

### (7) Missing test coverage
- ✅ Has 4 tests (empty search, nearest, fallback, normalize). Good coverage.

### (8) Performance bottlenecks
- **Line 84**: `query_point = NormalizedEmbedding(query.to_vec())`?—?allocates a new Vec on every search.
- **Line 96?104**: Brute-force search over fallback is O(n) per query.
- **Line 89**: `!map.iter().next().is_none()`?—?creates an iterator just to check if the map is empty. Should use a stored `is_empty` flag or `map.len()`.

---

## 8. `core/src/memory/embedding.rs` — EmbeddingModel

### (1) unwrap()/expect()
| Line | Code | Severity |
|------|------|----------|
| **34** | `let model = guard.as_mut().unwrap();` | 🟡 MEDIUM — After `is_none()` check, safe. But if the `TextEmbedding::try_new` call fails (line 32), `*guard` is NOT set, and `guard` remains `None`. The `?` on line 32 returns early, so `unwrap()` is never reached. **Actually safe**, but fragile if the logic changes. |

### (2) Lock contention
- **`Mutex<Option<TextEmbedding>>`** (line 7): First call blocks ALL threads while loading the model (potentially 1?5 seconds for ONNX model download/load). Subsequent calls serialize through the mutex for every `embed()` call?—?**even though the model is read-only after loading**.

**A++ Recommendation:** Use `OnceLock<TextEmbedding>` (std) or `arc-swap` for lock-free reads after initialization. The model is immutable after load.

### (3) Blocking operations in async contexts
- **`embed_single`** (line 39): Synchronous ONNX inference, 10?50ms per call. Called from:
  - `recall.store()`?—?called from `store_conversation`?—?called from async tool context.
  - `recall.search()`?—?called from async tool context.
  - `reflection.rs:run_reflection`?—?async function.

**A++ Recommendation:** Wrap embedding calls in `tokio::task::spawn_blocking` when called from async contexts, or provide an async variant `embed_single_async`.

### (7) Missing test coverage
- ❌ **Zero tests.** No tests for `embed`, `embed_single`, `cosine_similarity`, `embedding_to_bytes`, `bytes_to_embedding`.

**A++ Recommendation:** Add tests for `cosine_similarity` (identical vectors → 1.0, orthogonal → 0.0), `bytes_to_embedding`/`embedding_to_bytes` round-trip, zero-vector handling.

---

## 9. `core/src/memory/salience.rs`

### Analysis
Well-designed and well-tested. ✅

### (7) Test coverage
- ✅ 15+ tests covering recall_score, retrieval_score, bump_strength, auto_rate_importance, classify, category_half_life. Excellent.

### (8) Performance
- **`auto_rate_importance`** (line 228): Scans content for ~30 keyword patterns with `to_lowercase().contains()`. For long messages, this is O(n×k) where n=message length, k=keyword count. Minor, but could use Aho-Corasick for large-scale.

---

## 10. `core/src/memory/rrf.rs`

### Analysis
Clean, well-tested. ✅

### (7) Test coverage
- ✅ 4 tests (single list, consensus boost, normalized bounds, empty). Good.

---

## 11. `core/src/memory/consolidation.rs` — MemoryConsolidator

### (5) Memory leak risks
- `dedup_recall_memory` and `dedup_archival_memory` load up to 5000 records into memory (each with a 384-dim f32 embedding = 1.5KB). 5000 × 1.5KB = 7.5MB. Acceptable but should be documented.

### (8) Performance bottlenecks
- **O(n²) dedup** (lines 69?82, 123?136): 5000² = **25 million** cosine similarity computations. Each is 384 multiply-adds. Total: ~10 billion FLOPs. At 10 GFLOPS, this takes ~1 second. Acceptable for periodic maintenance, but will degrade if LIMIT is increased.
- **One-by-one deletes** (lines 88?93, 141?147): N individual `DELETE` queries instead of `DELETE FROM ... WHERE id IN (...)`.

**A++ Recommendation:** 
1. Use a single batch `DELETE FROM recall_memory WHERE id IN (?, ?, ...)`.
2. Consider LSH (locality-sensitive hashing) for approximate dedup?—?reduces O(n²) to O(n log n).
3. Run in `spawn_blocking` to avoid blocking the async runtime.

### (7) Missing test coverage
- ❌ **Zero tests.**

---

## 12. `core/src/memory/reflection.rs` — ReflectionDaemon

### (2) Lock contention
- **Line 204**: `let mem = memory.lock();`?—?acquires the **global MemoryManager mutex** in an async task. If a tool is holding the lock (e.g., during `search_conversation`), the reflection daemon blocks. Since this is a `parking_lot::Mutex` (not async-aware), it **blocks the entire Tokio worker thread**.

### (3) Blocking operations in async contexts
| Line | Code | Issue |
|------|------|-------|
| **187** | `std::fs::read_to_string(&agverse_path)` | Blocking file I/O in async context. |
| **191** | `std::fs::write(&agverse_path, &updated)` | Same. |
| **204** | `memory.lock()` | Blocking mutex acquisition in async context. |
| **213** | `mem.archival().insert(...)` | Synchronous DB write in async context. |

**A++ Recommendation:** Wrap all blocking operations in `tokio::task::spawn_blocking`:
```rust
let memory = memory.clone();
tokio::task::spawn_blocking(move || {
    let mem = memory.lock();
    mem.archival().insert(&fact.text, Some(&metadata))
}).await?;
```

### (6) Error handling gaps
- **Line 113**: `let _ = sender.try_send(...)`?—?silently drops reflections when channel is full (cap=200). Intentional (non-blocking), but should log at `debug` level for observability.

### (7) Missing test coverage
- ❌ No tests for `ensure_spawned`, `try_send`, `run_reflection`. Has good tests for `should_skip`, `parse_facts`, `append_facts_to_sections`, `build_extraction_prompt`.

---

## 13. `core/src/memory/diff_preference.rs` — DiffPreferenceEngine

### (3) Blocking in async
- **Line 70**: `client.chat_completion(&messages, &[]).await?`?—?properly async. ✅

### (6) Error handling gaps
- **Line 18**: `tokio::spawn(async move { ... })`?—?task handle is dropped. If the task panics, it's silently lost. No `JoinHandle` supervision.

**A++ Recommendation:** Store the `JoinHandle` and add error logging, or use `tokio::spawn` with a panic catcher.

### (7) Missing test coverage
- ❌ **Zero tests.**

---

## 14. `core/src/session.rs` — SessionManager

### (1) unwrap()/expect() in production paths
| Line | Code | Severity |
|------|------|----------|
| **193** | `let id = session_id.unwrap();` | 🟡 MEDIUM — Safe because `exists` is only true when `session_id` is `Some`, but logic is implicit. |
| **339** | `v.as_array().unwrap()` | 🟡 MEDIUM — After `v.is_array()` check. Safe but fragile. |

### (2) Lock contention
- All methods acquire `self.storage.conn()`?—?the global DB mutex.
- **`save_full`** (line 170): Holds the lock for the entire save operation (check exists + delete + insert all messages). For a 100-message session, this is 102 queries under one lock hold.
- **`resume`** (line 310): Carefully scopes the DB lock (lines 319?374) before calling `get_event_log`?—?good pattern, but fragile.

### (3) Blocking in async
- All methods are synchronous. If called from async contexts (likely via Tauri commands), they block the Tokio runtime.

### (5) Memory leak risks
- **`session_event_log`**: No cleanup. Each session can accumulate unlimited event log entries.
- **`session_messages`**: Deleted on session update (line 200?203), so bounded by message count. ✅

### (6) Error handling gaps
- **Line 376**: `self.get_event_log(session_id).unwrap_or_default()`?—?silently returns empty event log on failure.
- **Line 454**: `serde_json::from_str(&payload_str).unwrap_or(serde_json::json!({}))`?—?silently replaces corrupted payload with empty object.
- **Line 527**: `serde_json::from_str(&tags_str).unwrap_or_default()`?—?silently drops tags on parse failure.

### (7) Test coverage
- ✅ Good coverage: 15+ tests for save, list, resume, rename, summary, tags, archive, delete, purge, count, auto_title, subagent save.

### (8) Performance bottlenecks
- **`save_full`** (line 200?203): Deletes ALL messages and re-inserts on every update. For a 100-message session updated frequently, this is O(n) per save. Should use diff-based updates or `INSERT OR REPLACE`.
- **`list`** (line 242): Fixed `LIMIT 100`. No pagination.
- **`truncate_payload`** (line 467): Recursively traverses JSON. For deeply nested payloads, this is slow.

---

## 15. `core/src/agent_registry/definition.rs`

### (6) Error handling gaps
| Line | Code | Issue |
|------|------|-------|
| **94?95** | `serde_json::from_str(&skills_json).unwrap_or_default()` | Silently returns empty vec on corrupted JSON. Data loss. |
| **96?97** | `serde_json::from_str(&tools_json).unwrap_or_default()` | Same. |
| **98?99** | `serde_json::from_str(&rules_json).unwrap_or_default()` | Same. |
| **219** | `.filter_map(|r| r.ok())` in `list()` | Silently skips failed rows. |

**A++ Recommendation:** Log a warning when JSON parse fails: `tracing::warn!("failed to parse skills JSON for agent: {e}");`

### (7) Missing test coverage
- ❌ **Zero tests** for CRUD operations or builder functions.

---

## 16. `core/src/agent_registry/memory.rs` — AgentMemoryStore

### (1) BUG (not unwrap, but critical correctness issue)
| Line | Code | Severity |
|------|------|----------|
| **320** | `let new_strength = self.scorer.bump_strength(1.0);` | 🔴 **CRITICAL BUG** — Always bumps from 1.0, not the current `memory_strength`. Unlike `recall.rs:bump_strength` which reads the current value first, this always resets to `1.0 * 1.05 + 0.15 = 1.2`. Memory reinforcement is completely broken for agent memory. |

**A++ Recommendation:** Read the current strength first:
```rust
let current: f32 = db.query_row(
    "SELECT COALESCE(memory_strength, 1.0) FROM agent_memory WHERE id = ?1",
    rusqlite::params![id], |row| row.get(0)
).unwrap_or(1.0);
let new_strength = self.scorer.bump_strength(current);
```

### (6) Error handling gaps
- **Line 312**: `unwrap_or(0)` in `count()`?—?acceptable.
- Uses `filter_map(|r| r.ok())` in `load_records_by_rowids`?—?silently skips failed rows.

### (7) Missing test coverage
- ❌ **Zero tests.**

### (8) Performance bottlenecks
- **`search_by_keyword`** (line 154): `candidates.contains(&row)` is O(n) per insertion?—?O(n²) total. Use a `HashSet` instead.
- **`load_records_by_rowids`**: Uses `prepare` (not `prepare_cached`).

---

## 17. `core/src/agent_registry/history.rs`

### (6) Error handling gaps
- **Line 120?121**: `.filter_map(|r| r.ok())`?—?silently skips failed row parses.
- **Line 148**: `unwrap_or(0)` in `count()`?—?acceptable.

### (7) Missing test coverage
- ❌ **Zero tests.**

---

## 18. `core/src/agent_registry/skill_drafts.rs`

### (3) Blocking in async
- All file I/O (`std::fs::*`) is synchronous. If called from async context, blocks.

### (7) Test coverage
- ✅ Good coverage: 7 tests for generate, list, approve, reject, parse round-trip.

---

## Cross-Cutting Summary

### Top 5 Critical Issues (Priority Order)

1. **🔴 `block.rs:96-97, 125-126`** — `unwrap()` after DB write can panic, leaving DB/memory inconsistent.
2. **🔴 `agent_registry/memory.rs:320`** — `bump_strength` always uses 1.0 as base, breaking memory reinforcement for custom agents.
3. **🔴 `storage.rs:9`** — Single `Mutex<Connection>` serializes ALL DB operations across ALL modules. Use a connection pool.
4. **🟡 `hnsw.rs:75`** — Unbounded `fallback` Vec grows forever in long-running processes.
5. **🟡 `reflection.rs:204`** — Blocking `memory.lock()` in async context blocks Tokio worker thread.

### Top 5 Performance Quick Wins

1. Add `PRAGMA synchronous=NORMAL; PRAGMA cache_size=-64000;` to `storage.rs:31`.
2. Cache the tantivy `IndexReader` in `bm25.rs` instead of creating per-search.
3. Batch `bump_strength_batch` into a single `UPDATE ... WHERE id IN (...)`.
4. Use `prepare_cached` instead of `prepare` in archival.rs and agent_registry/memory.rs.
5. Batch deletes in `consolidation.rs` instead of one-by-one `DELETE`.

### Top 5 Test Coverage Gaps

1. `block.rs`?—?**zero tests**, has critical `unwrap()`s.
2. `consolidation.rs`?—?**zero tests**, O(n²) algorithm with deletion logic.
3. `agent_registry/memory.rs`?—?**zero tests**, has a critical bug.
4. `bm25.rs`?—?**zero tests**, core search infrastructure.
5. `embedding.rs`?—?**zero tests**, used everywhere.
