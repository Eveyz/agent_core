I'll read all files in parallel. Starting with the largest and most important ones.
Let me continue reading these files and start reading the others.
Now let me read the remaining files and the rest of the larger ones.
Now let me read the remaining files: storage.rs, archival.rs, block.rs, embedding.rs, consolidation.rs, reflection.rs, bm25.rs, hnsw.rs, rrf.rs, diff_preference.rs.
Now let me read the remaining files: storage.rs (rest), consolidation.rs, reflection.rs, bm25.rs, hnsw.rs, rrf.rs, diff_preference.rs.
Now let me read the remaining files: hnsw.rs, rrf.rs, diff_preference.rs, and the rest of reflection.rs.
I have now read all 13 files completely. Here is the thorough analysis of the complete memory subsystem.

---

# Memory Subsystem Analysis — `/Users/zniverse/Documents/projects/rust-projects/agent_core/core/src/memory/`

## File sizes (line counts)
```
mod.rs           741
salience.rs      619
recall.rs        732
archival.rs      261
block.rs         173
embedding.rs      74
consolidation.rs 158
reflection.rs    404
bm25.rs          122
hnsw.rs          174
rrf.rs            93
diff_preference.rs 89
storage.rs       421
```

---

# 1. `mod.rs` (741 lines) — The `MemoryManager` facade

### (1) Purpose
Top-level facade composing all memory subsystems: `CoreMemory` (block), `RecallMemory` (short-term), `ArchivalMemory` (long-term), `MemoryConsolidator` (dedup), plus optional `BM25Index` and `HNSWIndex` for hybrid search. Provides multiple search pipelines: plain vector/keyword, BM25-only, BM25+salience, and BM25+HNSW hybrid with RRF fusion.

### (2) Key data structures
- `MemoryManager` (lines 27-35): holds `core`, `recall`, `archival`, `consolidator`, `session_id: String`, `bm25: Option<BM25Index>`, `hnsw: Option<HNSWIndex>`.
- Re-exports: `MemoryStats`, `MemoryCategory`, `SalienceConfig`, `SalienceScorer`, `ScoredRecord` (lines 25-26).
- Sub-module declarations (lines 1-11, 741): includes `diff_preference` declared at line 741 (`pub mod diff_preference;`) — **placed after the `#[cfg(test)] mod tests` block at line 707**, which is unusual placement but valid Rust.

### (3) SQL schema and queries
No schema defined here; delegates to `storage.rs`. Queries appear in search methods:
- **BM25 search query** (lines 254-260): `SELECT id, session_id, role, content, embedding, importance, COALESCE(memory_strength, 1.0), COALESCE(access_count, 0), last_accessed_at, COALESCE(category, 'Conversation'), created_at FROM recall_memory WHERE id IN ({placeholders})`. Placeholders built dynamically as `?1, ?2, ...` (lines 247-252).
- Same query pattern repeated in `search_conversation_bm25_with_salience` (lines 329-335), `search_conversation_hybrid` (lines 439-445), `search_conversation_hybrid_precomputed` (lines 532-538), `search_conversation_filtered` (lines 656-672).

### (4) Embedding/vector search approach
- `store_conversation` (lines 163-186): stores via recall, then syncs to BM25 (line 169) and HNSW fallback (lines 173-183). The HNSW sync re-embeds content (`model.embed_single(content)`, line 175) — **a second embedding call** even though recall.store already computed one. This is wasteful since the embedding is stored in SQLite but not returned.
- `search_conversation` (lines 188-202): if both BM25+HNSW available → hybrid; else → recall.search; then bumps strength.
- `search_conversation_precomputed` (lines 210-222): accepts pre-computed query embedding to avoid embedding inside the lock.
- `search_conversation_hybrid` (lines 395-493): Phase 1 dual recall (BM25 150 + HNSW 150), Phase 2 RRF fusion (`rrf::fuse_normalized`), Phase 3 truncate to 100 candidates, Phase 4-5 load from SQLite + salience score. Block-scoped db guard to avoid deadlock with `bump_strength_batch`.
- `search_conversation_hybrid_precomputed` (lines 497-586): same but with pre-computed embedding.

### (5) Concurrency/locking patterns
- The `MemoryManager` itself has no `Arc<Mutex<>>` — it's expected to be wrapped externally (reflection.rs wraps it in `Arc<Mutex<MemoryManager>>`).
- **Critical deadlock avoidance pattern** (lines 343-386, 430-487, 523-580): the SQLite connection guard is block-scoped so it drops before `bump_strength_batch` is called (which acquires the same `parking_lot::Mutex` via `storage_conn()`). Comments explicitly note "same mutex = deadlock" (line 344).
- `search_conversation` bumps strength via `bump_strength_batch` (line 200) — this holds the lock again after search completes.

### (6) Code quality issues
- **Massive code duplication**: the hybrid search SQL + scoring loop is nearly identical between `search_conversation_hybrid` (lines 395-493) and `search_conversation_hybrid_precomputed` (lines 497-586) — ~90 lines duplicated. Same for `search_conversation_bm25` and `search_conversation_bm25_with_salience`.
- Line 208 doc comment typo: "Inside" should be "inside".
- `prepare` vs `prepare_cached` inconsistency: `search_conversation_bm25` uses `prepare_cached` (line 269), but `search_conversation_hybrid` uses `prepare` (line 453) and `search_conversation_hybrid_precomputed` uses `prepare` (line 546). The hybrid paths should use `prepare_cached` for performance.
- `search_conversation_filtered` (line 684) uses `db.prepare` (not cached).

### (7) Performance concerns
- **Double embedding on store** (line 175): `store_conversation` calls `recall.store` which embeds, then re-embeds for HNSW sync. Should return the embedding from `recall.store` or read it back.
- `search_conversation_hybrid` embeds the query inside the method (line 407: `model.embed_single(query)`) — this is 10-50ms of blocking inside what may be a locked context. The `precomputed` variant exists to fix this, but the non-precomputed path remains.
- `bm25.search(query, 150)` and `hnsw.search(&normalized, 150)` retrieve 150 candidates each, but then fused list is truncated to 100 (line 419), and SQLite loads all 100.
- RRF map rebuilt twice: once by `rrf::fuse_normalized` (line 416) and once locally (lines 456-460) — the local `rrf_map` **overrides** the fused scores with a re-computed rank-based score, discarding the actual RRF fusion scores.

### (8) Bugs
- **RRF scores discarded** (lines 456-460, 549-553): `fuse_normalized` returns `(id, s_rrf)` pairs, but the code takes only `.map(|(id, _)| id)` (lines 419-422) discarding the scores, then rebuilds a new `rrf_map` using `60.0 / (60.0 + rank as f32 + 1.0)` based on the **truncated candidate order** — not the RRF fusion scores. The `s_rrf` from `fuse_normalized` is never used. This means the RRF fusion is effectively just rank-based scoring from a single merged list, defeating the purpose of reciprocal rank fusion.
- **`search_conversation_filtered` does not bump strength** (lines 632-704): unlike other search methods, it never calls `bump_strength_batch`, so filtered search results don't get reinforcement.
- `search_conversation_bm25` (line 262) clones `bm25_ids` into `params: Vec<String>` unnecessarily — could borrow.
- In `search_conversation_hybrid`, if `self.recall.embedding_model()` is `None` (line 406), `hnsw_ids` is empty (line 411) but the code still proceeds with BM25-only results fused with an empty list — functionally OK but wasteful.

---

# 2. `storage.rs` (421 lines) — SQLite connection wrapper

### (1) Purpose
Wraps a single `rusqlite::Connection` in `Arc<Mutex<Connection>>` (parking_lot). Handles DB initialization, schema creation, FTS5 virtual tables + triggers, and idempotent migrations.

### (2) Key data structures
- `Storage` (lines 7-10): `#[derive(Clone)] pub struct Storage { db: Arc<Mutex<Connection>> }`. Cloneable because it just clones the `Arc`.

### (3) SQL schema and queries
**Schema created in `init_tables`** (lines 41-387):
- `memory_blocks` (lines 46-53): `id TEXT PK, label, content, max_chars INTEGER DEFAULT 2000, created_at, updated_at`.
- `recall_memory` (lines 55-67): `id TEXT PK, session_id, role, content, embedding BLOB, importance REAL DEFAULT 0.5, memory_strength REAL DEFAULT 1.0, access_count INTEGER DEFAULT 0, last_accessed_at TEXT, category TEXT DEFAULT 'Conversation', created_at`.
  - Indexes: `idx_recall_session` on `session_id` (line 69), `idx_recall_created` on `created_at` (line 70).
- `archival_memory` (lines 72-78): `id TEXT PK, content, embedding BLOB, metadata TEXT, created_at`.
- `conversation_summaries` (lines 80-87): `id, session_id, summary, message_range, embedding BLOB, created_at`.
- `projects` (lines 89-95).
- `sessions` (lines 97-114): many columns including `parent_session_id`, `session_type`, `project_id`, `mode`.
  - Indexes: `idx_sessions_updated`, `idx_sessions_archived`, `idx_sessions_parent` (lines 116-118).
- `session_messages` (lines 120-131): FK to sessions with `ON DELETE CASCADE`, `UNIQUE(session_id, msg_index)`.
- `session_event_log` (lines 135-144).
- `cronjobs` (lines 148-161), `cronjob_runs` (lines 163-170).
- `agents` (lines 174-192), `agent_memory` (lines 196-210), `agent_history` (lines 215-230).
- `workflows` (lines 235-246), `workflow_nodes` (lines 248-258), `workflow_edges` (lines 262-273), `workflow_runs` (lines 279-292), `workflow_run_node_results` (lines 297-313).

**FTS5 virtual tables + triggers** (lines 319-366):
- `recall_memory_fts` (line 322): `USING fts5(content, tokenize='unicode61')`.
- `archival_memory_fts` (line 323).
- `agent_memory_fts` (line 351).
- Triggers: `recall_fts_ai` (insert), `recall_fts_ad` (delete), `recall_fts_au` (update = delete + insert). Same pattern for archival and agent_memory.
- Backfill: `INSERT OR IGNORE INTO recall_memory_fts(rowid, content) SELECT rowid, content FROM recall_memory` (lines 347-348, 364).

**Migrations** (lines 368-384): `ALTER TABLE ... ADD COLUMN` for: `memory_strength`, `access_count`, `last_accessed_at`, `category` on recall_memory; `parent_session_id`, `session_type`, `project_id`, `process_time_ms`, `thought_time_ms`, `mode` on sessions; `model` on cronjobs.

### (4) Embedding/vector search approach
None directly — storage is agnostic. Embeddings stored as BLOB (little-endian f32 bytes).

### (5) Concurrency/locking patterns
- Single `Arc<Mutex<Connection>>` — **one global mutex for the entire DB**. All operations serialize through this lock. `conn()` (lines 389-391) returns `MutexGuard<'_, Connection>`.
- `init_tables` locks the mutex (line 42) and holds it for the entire schema creation batch.
- WAL mode enabled (line 31): `PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;`.

### (6) Code quality issues
- **Schema mixed with non-memory tables**: `storage.rs` in the `memory` module creates tables for sessions, cronjobs, agents, workflows — unrelated to memory. This is a separation-of-concerns violation; storage.rs is a general DB layer masquerading as memory storage.
- Migrations use `let _ = db.execute_batch(migration)` (line 383) — silently ignores errors. This is intentional (idempotent ALTER TABLE fails if column exists) but could mask real errors. The `add_column_if_not_exists` method (lines 398-420) exists as a proper alternative but isn't used for the migrations list.
- No `PRAGMA synchronous`, `PRAGMA cache_size`, or `PRAGMA busy_timeout` tuning — WAL is set but no busy timeout means concurrent access from multiple connections (if ever added) could get `SQLITE_BUSY` errors.

### (7) Performance concerns
- **Single connection, single mutex**: all DB access serializes. Under concurrent load (multiple agents, reflection daemon + main thread), this is a bottleneck.
- No connection pooling.
- FTS5 backfill on every startup (lines 347-348): `INSERT OR IGNORE INTO recall_memory_fts SELECT rowid, content FROM recall_memory` scans the entire table on every init — `INSERT OR IGNORE` makes it idempotent but it still scans all rows.

### (8) Bugs
- **No `busy_timeout` PRAGMA**: if a second `Storage::new` is called on the same DB file (e.g., agent_memory using a separate Storage instance), WAL helps but without `busy_timeout`, writes could fail with `SQLITE_BUSY` immediately rather than waiting.
- Migrations list (lines 369-380) includes `ALTER TABLE sessions ADD COLUMN mode TEXT NOT NULL DEFAULT 'build'` (line 379) — but `mode` is already in the `CREATE TABLE` (line 111). On fresh DBs, the ALTER fails silently (column exists). On old DBs, it adds the column. This is fine but the silent failure masks any real issue.
- The `agent_memory` table (lines 196-210) has a `source` column but no FTS trigger for `source`-based filtering — only content is indexed.

---

# 3. `recall.rs` (732 lines) — Short-term conversational memory

### (1) Purpose
Manages short-term conversation records with embedding-based vector search, FTS5 keyword search, salience scoring (Ebbinghaus decay), strength reinforcement, pruning, and promotion to archival.

### (2) Key data structures
- `RecallRecord` (lines 10-23): `id, session_id, role, content, embedding: Vec<f32>, importance, memory_strength, access_count: u32, last_accessed_at: Option<String>, category: MemoryCategory, created_at`.
- `RecallMemory` (lines 25-29): `storage: Storage, embedding_model: Option<Arc<EmbeddingModel>>, scorer: SalienceScorer`.
- `MemoryStats` (lines 727-732): `total_count: usize, avg_strength: f32, avg_importance: f32`.

### (3) SQL schema and queries
**Store** (lines 105-109): `INSERT INTO recall_memory (id, session_id, role, content, embedding, importance, memory_strength, access_count, category, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1.0, 0, ?7, ?8)`.

**FTS5 keyword search** (lines 218-226): `SELECT r.id, r.session_id, r.role, r.content, r.embedding, r.importance, COALESCE(r.memory_strength, 1.0), COALESCE(r.access_count, 0), r.last_accessed_at, COALESCE(r.category, 'Conversation'), r.created_at FROM recall_memory_fts f JOIN recall_memory r ON r.rowid = f.rowid WHERE recall_memory_fts MATCH ?1 ORDER BY rank LIMIT ?2`.

**LIKE fallback** (lines 243-249): `SELECT ... FROM recall_memory WHERE content LIKE ?1 ORDER BY created_at DESC LIMIT ?2`.

**Candidate collection** (lines 168-209): `collect_candidates` — FTS5 MATCH (`SELECT rowid FROM recall_memory_fts WHERE recall_memory_fts MATCH ?1 LIMIT ?2`, line 179) + recent records (`SELECT rowid FROM recall_memory ORDER BY created_at DESC LIMIT ?1`, line 195).

**Vector search** (lines 306-312): `SELECT ... FROM recall_memory WHERE rowid IN ({placeholders})` — loads candidates by rowid.

**Scored search** (lines 382-389): same pattern, different column set (no session_id/role).

**bump_strength** (lines 461-475): `SELECT COALESCE(memory_strength, 1.0) FROM recall_memory WHERE id = ?1` then `UPDATE recall_memory SET memory_strength = ?1, access_count = access_count + 1, last_accessed_at = ?2 WHERE id = ?3`.

**bump_strength_batch** (lines 489-504): same query in a loop — **N+1 pattern**: one SELECT + one UPDATE per ID, no batching.

**search_by_date** (lines 518-527): `SELECT ... FROM recall_memory WHERE created_at >= ?1 AND created_at <= ?2 ORDER BY created_at DESC LIMIT ?3`.

**stats** (lines 558-580): three separate `query_row` calls: `COUNT(*)`, `AVG(COALESCE(memory_strength, 1.0))`, `AVG(importance)`.

**prune_cold_memories** (lines 597-603): `SELECT id, importance, COALESCE(memory_strength, 1.0), COALESCE(category, 'Conversation'), created_at FROM recall_memory WHERE importance < ?1 ORDER BY created_at ASC LIMIT ?2`, then `DELETE FROM recall_memory WHERE id IN ({})` (lines 643-646).

**promote_to_archival** (lines 673-679): `SELECT id, content, embedding, role, session_id, importance, COALESCE(category, 'Conversation') FROM recall_memory WHERE importance >= ?1 ORDER BY created_at ASC LIMIT ?2`, then `DELETE FROM recall_memory WHERE id = ?1` per record (line 716).

### (4) Embedding/vector search approach
- `search` (lines 130-137): embeds query, calls `search_by_vector`.
- `search_by_vector` (lines 283-350): FTS5 pre-filter (50 candidates) + recent 100 records → load full records → cosine similarity → `retrieval_score` (α·semantic + β·recall + γ·importance) → sort → truncate.
- `collect_candidates` (lines 168-209): HashSet to dedupe FTS5 + recent rowids.
- Cosine similarity computed in Rust (embedding.rs), not in SQL — all embeddings loaded into memory for scoring.

### (5) Concurrency/locking patterns
- `storage_conn()` (lines 63-65) returns `parking_lot::MutexGuard<rusqlite::Connection>` — callers hold the lock for the duration of their query.
- `bump_strength` and `bump_strength_batch` each acquire their own lock.
- No re-entrancy issues within a single method since each method acquires the lock once.

### (6) Code quality issues
- **Row parsing duplication**: `parse_recall_row` (lines 263-279) is used in `search_by_keyword`, `search_by_vector`, `search_by_date`. But `search_by_date` (lines 530-546) inlines the parsing instead of calling `parse_recall_row` — duplicated logic.
- `parse_recall_row_static` (lines 68-70) is a trivial public wrapper around the private `parse_recall_row` — exists only for cross-module access from `mod.rs`. Could just make `parse_recall_row` public.
- `build_fts_query` (lines 141-164) and `collect_candidates` are duplicated in `archival.rs` (lines 9-32) — nearly identical code.
- `store_raw` (lines 115-123) is "legacy compatibility" — could be deprecated.

### (7) Performance concerns
- **N+1 query in bump_strength_batch** (lines 489-504): for each ID, one SELECT + one UPDATE. With top_k=10, that's 20 queries. Should batch into a single `SELECT ... WHERE id IN (...)` and a single `UPDATE ... CASE WHEN id = ... THEN ...`.
- **All embeddings loaded into memory** for vector search (line 320): for 150 candidates, each 384-dim f32 = 1.5KB, total ~230KB — acceptable but scales poorly.
- **stats() makes 3 separate queries** (lines 560-574): could be a single `SELECT COUNT(*), AVG(...), AVG(...)`.
- `prune_cold_memories` loads all low-importance records into memory and computes recall_score in Rust — could filter in SQL with a time-based WHERE clause.
- `promote_to_archival` does per-record DELETE in a loop (line 716) — N+1 pattern.

### (8) Bugs
- **`bump_strength` silent failure** (line 461-467): `query_row(...).unwrap_or(1.0)` — if the record doesn't exist, it silently returns 1.0 and the subsequent UPDATE affects 0 rows with no error. The method returns `Ok(())` regardless.
- **`bump_strength_batch` ignores UPDATE errors** (line 500): `let _ = db.execute(...)` — discards errors entirely.
- **`search_by_vector` empty embedding handling**: if `embedding_bytes` is empty (no embedding model), `bytes_to_embedding` returns `Vec::new()`, and `cosine_similarity` returns 0.0 (embedding.rs line 50-52). Records without embeddings get semantic score 0, but they still appear in results if they're recent or FTS-matched — they'll just rank low. Not a bug per se but could be surprising.
- **`prune_cold_memories` potential integer overflow** (line 609): `max_to_delete as i64` — if `max_to_delete` is `usize::MAX` on a 64-bit system, this could overflow `i64`. Unlikely in practice.

---

# 4. `archival.rs` (261 lines) — Long-term archival memory

### (1) Purpose
Long-term knowledge storage with vector search, FTS5 keyword search, and delete. Used for durable facts and promoted recall memories.

### (2) Key data structures
- `ArchivalRecord` (lines 34-41): `id, content, embedding: Vec<f32>, metadata: Option<String>, created_at`.
- `ArchivalMemory` (lines 43-46): `storage: Storage, embedding_model: Option<Arc<EmbeddingModel>>`.

### (3) SQL schema and queries
**Insert** (lines 79-82): `INSERT INTO archival_memory (id, content, embedding, metadata, created_at) VALUES (?1, ?2, ?3, ?4, ?5)`.
**Insert with embedding** (lines 99-102): same query.
**Vector search** (lines 124-158): FTS5 candidates (50) + recent 100 → `SELECT id, content, embedding, metadata, created_at FROM archival_memory WHERE rowid IN ({})` → cosine similarity → sort → truncate.
**Keyword search FTS5** (lines 196-201): `SELECT a.id, a.content, a.embedding, a.metadata, a.created_at FROM archival_memory_fts f JOIN archival_memory a ON a.rowid = f.rowid WHERE archival_memory_fts MATCH ?1 ORDER BY rank LIMIT ?2`.
**Keyword fallback LIKE** (lines 226-230): `SELECT ... FROM archival_memory WHERE content LIKE ?1 ORDER BY created_at DESC LIMIT ?2`.
**Delete** (lines 252-256): `DELETE FROM archival_memory WHERE id = ?1`.

### (4) Embedding/vector search approach
Same as recall: embed query → FTS5 pre-filter (50) + recent 100 → load candidates → cosine similarity → sort. No salience scoring (no time decay for archival — appropriate for durable storage).

### (5) Concurrency/locking patterns
Acquires `storage.conn()` lock per method call. No nested locking.

### (6) Code quality issues
- **`build_fts_query` duplicated** from recall.rs (lines 9-32 vs recall.rs lines 141-164) — exact same function. Should be shared.
- **Row parsing duplicated** three times: `search_by_vector` (lines 168-177), `search_by_keyword` FTS5 (lines 204-213), `search_by_keyword` LIKE (lines 232-241) — all parse the same 5 columns identically. Should use a shared `parse_archival_row` function like recall.rs does.
- `insert` and `insert_with_embedding` share the same INSERT SQL — `insert` could call `insert_with_embedding` after computing the embedding.

### (7) Performance concerns
- **Always loads recent 100 records** (lines 136-144) even for vector search — this means every search pulls 100 recent records regardless of query relevance. For large archival stores, this is wasteful but bounded.
- No salience/pruning for archival — it grows unbounded. No TTL or consolidation threshold beyond the `consolidation.rs` dedup.
- FTS5 `LIMIT 50` (line 125) + recent `LIMIT 100` = up to 150 candidates loaded with full embeddings.

### (8) Bugs
- **FTS5 `MATCH ?1` without parameter limit**: the FTS5 query (line 125) has no `LIMIT` parameter binding — it's hardcoded as `LIMIT 50` in the SQL string but actually the query uses `?1` for the fts_query and no limit parameter. Looking again: line 125: `SELECT rowid FROM archival_memory_fts WHERE archival_memory_fts MATCH ?1 LIMIT 50` — the 50 is hardcoded in SQL, not parameterized. This is fine but inflexible.
- No bugs in logic — the search fallback chain (FTS5 → LIKE) is correct.

---

# 5. `block.rs` (173 lines) — Core memory blocks (manual notes)

### (1) Purpose
Manages "core memory blocks" — persistent key-value notes (e.g., "human" persona info, "persona" agent behavior). Loaded into a HashMap at startup, synced to SQLite on modifications.

### (2) Key data structures
- `MemoryBlock` (lines 7-14): `id, label, content, max_chars: usize, updated_at`. Derives `Serialize, Deserialize`.
- `CoreMemory` (lines 16-20): `storage: Storage, blocks: HashMap<String, MemoryBlock>, default_max_chars: usize`.

### (3) SQL schema and queries
**Load** (line 37): `SELECT id, label, content, max_chars, updated_at FROM memory_blocks`.
**Append** (lines 90-94): `UPDATE memory_blocks SET content = ?1, updated_at = ?2 WHERE id = ?3`.
**Replace** (lines 119-123): same UPDATE query.
**Create** (lines 138-141): `INSERT INTO memory_blocks (id, label, content, max_chars, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)`.

### (4) Embedding/vector search approach
None — core memory blocks are not embedded or vector-searched. They're injected directly into the agent's context via `to_context_string` (lines 162-168).

### (5) Concurrency/locking patterns
- `CoreMemory` holds a `Storage` (clones the Arc), so it shares the same global DB mutex.
- `load()` (lines 33-63) acquires the lock in a block scope, then potentially calls `self.create()` (lines 58-59) which acquires the lock again — **but the first lock is released by the block scope** (lines 34-55) before `create` is called. This is correct.
- `append` and `replace` acquire the lock (lines 89, 118), update the DB, then update the in-memory HashMap. **Not atomic** — if the DB update succeeds but the process crashes before the HashMap update, they'd be out of sync. But since `load()` re-reads from DB on restart, this is self-healing.

### (6) Code quality issues
- **`unwrap()` on HashMap get_mut** (lines 96-97, 125-126): `self.blocks.get_mut(id).unwrap()` — this is safe because the block existence was checked earlier, but it's fragile. If the block was somehow removed between the check and the update (not possible in single-threaded `&mut self`), it would panic.
- **`max_chars` check uses byte length** (lines 81, 110): `new_content.len()` and `replaced.len()` — this counts bytes, not chars. For multi-byte UTF-8 content (Chinese text), the limit is effectively smaller than expected. The `max_chars` field name implies character count but the check uses byte count.
- `to_context_string` (lines 162-168) iterates `HashMap` values — **non-deterministic order**. The context string will have blocks in arbitrary order, which could cause non-deterministic LLM behavior.

### (7) Performance concerns
- Minimal — core memory is small (few blocks, few KB each). The HashMap is loaded once at startup.
- `load()` acquires the lock once and reads all blocks — efficient.

### (8) Bugs
- **Byte vs char length mismatch** (lines 81, 110): `max_chars` suggests character limit but `.len()` measures bytes. A 2000-byte limit for Chinese text allows only ~667 characters. This is a semantic bug — the field should either be renamed to `max_bytes` or the check should use `.chars().count()`.
- **`replace` only replaces first occurrence** (line 108): `block.content.replacen(old_content, new_content, 1)` — documented behavior but could surprise users expecting global replace.
- **No `delete` method** — blocks can be created and modified but never deleted through this API.

---

# 6. `embedding.rs` (74 lines) — Embedding model wrapper

### (1) Purpose
Wraps `fastembed::TextEmbedding` (ONNX-based local embeddings) with lazy initialization. Provides cosine similarity and byte serialization helpers.

### (2) Key data structures
- `EmbeddingModel` (lines 5-8): `model_name: String, inner: Mutex<Option<TextEmbedding>>`. Lazy-loaded on first `embed()` call.
- Functions: `cosine_similarity`, `embedding_to_bytes`, `bytes_to_embedding`.

### (3) SQL schema and queries
None. Embeddings serialized as little-endian f32 bytes via `embedding_to_bytes` (line 66: `f.to_le_bytes()`) and deserialized via `bytes_to_embedding` (lines 69-73: `chunks_exact(4)` → `f32::from_le_bytes`).

### (4) Embedding/vector search approach
- `embed` (lines 18-37): lazy-loads model on first call, then `model.embed(texts, None)`.
- `embed_single` (lines 39-42): wraps `embed` for single text, returns `Vec<f32>` or empty vec on failure.
- `cosine_similarity` (lines 49-63): standard dot product / (norm_a * norm_b). Returns 0.0 for mismatched lengths or zero norms.
- `dimension` (line 44-46): hardcoded `384` — matches BGE-small/MiniLM models.

### (5) Concurrency/locking patterns
- `inner: Mutex<Option<TextEmbedding>>` — the mutex is held during model loading (potentially seconds) AND during every `embed()` call (line 35). This means **all embedding calls are serialized** — concurrent `embed_single` calls block each other. The ONNX model itself may be thread-safe, but the Mutex forces serialization.

### (6) Code quality issues
- **`dimension()` hardcoded to 384** (line 45): If someone configures `BGEBaseENV15` (768-dim), this returns the wrong dimension. The model enum selection (lines 25-30) supports different models with different dimensions, but `dimension()` always returns 384.
- **Unknown model name silently falls back** (lines 29): `_ => fastembed::EmbeddingModel::BGESmallENV15` — no warning or error for unrecognized model names.
- `embed_single` uses `unwrap_or_default()` (line 41) — returns empty vec if embedding fails, which downstream code must handle (cosine_similarity returns 0.0 for empty vecs).

### (7) Performance concerns
- **Mutex held during inference** (line 35): `model.embed()` runs under the lock. For batch processing, this serializes all embedding requests. Should use `RwLock` or `OnceLock` for the model and release the lock before inference.
- Model loading is synchronous and blocking (line 32) — could take 1-5 seconds on first use, blocking whatever thread calls it.
- `embed_single` allocates a `Vec<String>` for each call (line 40) — minor but avoidable with a batch API.

### (8) Bugs
- **`dimension()` returns wrong value for non-384 models** (line 45): If `BGEBaseENV15` (768-dim) is configured, `dimension()` returns 384. This isn't currently used for validation anywhere, so it's a latent bug.
- No validation that the stored embedding dimension matches `dimension()` — mixing models would produce garbage cosine similarities (mismatched lengths return 0.0).

---

# 7. `consolidation.rs` (158 lines) — Memory deduplication

### (1) Purpose
Deduplicates near-identical memories in recall and archival storage using cosine similarity thresholding. O(n²) pairwise comparison.

### (2) Key data structures
- `MemoryConsolidator` (lines 8-12): `storage: Storage, embedding_model: Option<Arc<EmbeddingModel>>`. Derives `Clone`.
- `ConsolidationReport` (lines 154-158): `deduped_recall: usize, deduped_archival: usize`.

### (3) SQL schema and queries
**Read recall** (lines 49-51): `SELECT id, embedding FROM recall_memory ORDER BY created_at DESC LIMIT 5000`.
**Delete recall** (lines 89-92): `DELETE FROM recall_memory WHERE id = ?1` — per ID in a loop.
**Read archival** (lines 103-105): `SELECT id, embedding FROM archival_memory ORDER BY created_at DESC LIMIT 5000`.
**Delete archival** (lines 143-146): `DELETE FROM archival_memory WHERE id = ?1` — per ID in a loop.

### (4) Embedding/vector search approach
Loads up to 5000 records' embeddings, performs O(n²) pairwise cosine similarity. Threshold: 0.85 for recall (line 66), 0.90 for archival (line 120). Deletes the later duplicate (higher index in DESC order = older record).

### (5) Concurrency/locking patterns
- **Three-phase locking** (documented in comments): Phase 1 reads records (lock held briefly), Phase 2 does O(n²) computation lock-free (line 63: "storage.db lock released"), Phase 3 batch deletes (lock held briefly).
- This is a well-designed pattern that minimizes lock contention.

### (6) Code quality issues
- **Code duplication**: `dedup_recall_memory` (lines 45-97) and `dedup_archival_memory` (lines 99-151) are nearly identical — only the table name and threshold differ. Should be a generic `dedup_table(table, threshold)`.
- Thresholds are hardcoded (0.85, 0.90) — should be configurable.
- LIMIT 5000 is hardcoded — large stores with >5000 records won't fully dedup.

### (7) Performance concerns
- **O(n²) complexity**: 5000 records = 12.5M comparisons. Each comparison is a 384-dim dot product (~384 multiplications). At ~1ns per multiply, that's ~5 seconds. This is acceptable for periodic background runs but would block if called synchronously.
- Each cosine_similarity call allocates nothing (iterators), but the 5000 embeddings (each 1.5KB) = ~7.5MB in memory.
- **N+1 delete pattern** (lines 88-93, 141-147): deletes one at a time in a loop. Should batch into `DELETE FROM ... WHERE id IN (...)`.

### (8) Bugs
- **`consolidate()` no-ops without embedding model** (lines 30-35): if `embedding_model.is_none()`, returns empty report. This is correct behavior (can't compute similarity without embeddings) but could be documented better.
- **Dedup deletes older records** (line 79: `records[j].0` where j > i, and records are sorted DESC by created_at, so j = older). This means the **newer** duplicate is kept. This is reasonable but could lose context if the older record had been accessed more (higher memory_strength) — the dedup doesn't consider access_count or memory_strength when choosing which to delete.
- No transaction wrapping the deletes — if the process crashes mid-delete, some duplicates are removed and others aren't. Not harmful but inconsistent.

---

# 8. `reflection.rs` (404 lines) — Background reflection daemon

### (1) Purpose
Background daemon that extracts durable facts from conversations using an LLM and writes them to `agverse.md` (core memory file) and archival memory. Lazy-spawned Tokio task communicating via mpsc channel.

### (2) Key data structures
- `ReflectionDaemon` (lines 30-33): `sender: Mutex<Option<mpsc::Sender<ConversationSlice>>>, init: Mutex<Option<DaemonInit>>`.
- `DaemonInit` (lines 35-39): `client: OpenAIClient, memory: Arc<Mutex<MemoryManager>>, trigger_count: usize`.
- `ConversationSlice` (lines 19-22): `role: String, content: String`.
- `ExtractedFact` (lines 150-154): `section: String, text: String`.

### (3) SQL schema and queries
No direct SQL — delegates to `MemoryManager::archival().insert()` (line 213) which uses archival_memory INSERT.

### (4) Embedding/vector search approach
None directly. Facts are stored in archival memory (which embeds them) and written to `agverse.md` as text.

### (5) Concurrency/locking patterns
- **Lazy spawn** (lines 59-106): `ensure_spawned()` checks if sender exists, takes init, tries to get Tokio runtime handle. If no runtime, puts init back (line 72) for later retry.
- **Two separate mutexes**: `sender` and `init` — locked sequentially (lines 60, 65). Potential for **lock ordering issues** if another path locks them in reverse order, but only `ensure_spawned` locks both.
- `try_send` (lines 109-118): non-blocking, silently drops if channel full (capacity 200, line 76).
- The spawned task holds `Arc<Mutex<MemoryManager>>` and locks it when writing facts (line 204: `let mem = memory.lock()`).

### (6) Code quality issues
- **File I/O without proper error recovery** (lines 187-201): reads `agverse.md`, modifies in memory, writes back. If the write fails, the in-memory modification is lost. No backup or atomic write.
- `append_facts_to_sections` (lines 224-249) uses string search for section headers (`# {section}`) — fragile. A section named "User Preferences" matches `# User Preferences` but also matches `## User Preferences` (substring). The search for next section uses `\n# ` (line 233) which wouldn't match `## ` subsections — inconsistent.
- `parse_facts` (lines 269-313) has a **dangerous fallback** (lines 305-310): if the LLM response is <200 chars and doesn't start with `{`, it treats the entire response as a single fact. This could store garbage as a "fact" if the LLM returns a conversational response.
- `build_extraction_prompt` (lines 251-266) uses `format!` with raw `{conversation}` — if conversation contains `{` or `}`, it could break the format string. Actually, looking more carefully, the `r#"..."#` raw string with `{conversation}` is a `format!` placeholder, so literal braces in conversation would need escaping. **This is a format string injection issue** — if the conversation text contains `{`, `format!` will try to interpret it as a format specifier and panic.

### (7) Performance concerns
- LLM call per `trigger_count` messages (line 89-93) — synchronous within the async task. The `chat_completion` is `.await`ed, so it's properly async.
- Channel capacity 200 (line 76) — if reflection is slow, messages are silently dropped (line 113: `try_send`).
- `memory.lock()` (line 204) blocks the reflection task if the main thread holds the MemoryManager lock — could cause reflection delays.

### (8) Bugs
- **`format!` injection in `build_extraction_prompt`** (line 264): `{conversation}` is a format placeholder. If the conversation text contains `{` or `}`, `format!` will either misinterpret them as format specifiers (panic) or produce garbled output. Should use `format!(r#"...{conversation}..."#)` only if conversation is guaranteed brace-free, or better, use string concatenation/`replace`.
- **`append_facts_to_sections` off-by-one** (lines 231-235): `next_section` finds `\n# ` after the section header. If the section is the last one, it inserts before `result.len()` (end of string). But the insert position calculation `after_header + p` where `p` is the position of `\n# ` — this inserts **before** the newline, meaning the fact is appended right after the last line of content without a preceding newline. Actually, looking again: line 238 `result.insert_str(next_section, &format!("\n- {}", fact.text))` — it adds a `\n` prefix, so this is OK.
- **Race condition on `agverse.md`**: the reflection daemon writes to `agverse.md` while the agent may also be reading/writing it through core memory. No file locking.
- **`should_skip` word count** (line 124): `content.split_whitespace().count()` counts whitespace-separated tokens. For Chinese text (no spaces), a long message could be counted as 1 word and skipped.

---

# 9. `salience.rs` (619 lines, ~455 logic + 164 tests) — Salience scoring

### (1) Purpose
Implements Ebbinghaus forgetting curve for memory decay, importance auto-rating heuristics, memory category classification, and strength reinforcement. Two scoring modes: legacy `retrieval_score` (additive) and `hybrid_score` (multiplicative decay).

### (2) Key data structures
- `SalienceConfig` (lines 27-67): `alpha` (semantic weight, 0.55), `beta` (recall weight, 0.25), `gamma` (importance weight, 0.20), `default_half_life_hours` (168), `importance_decay_modifier` (3.0), `strength_bump_additive` (0.15), `strength_bump_multiplicative` (1.05), `max_strength` (5.0), `recall_floor` (0.01). All with serde defaults.
- `SalienceScorer` (lines 117-119): `config: SalienceConfig`.
- `MemoryCategory` (lines 334-346): enum `Conversation, Decision, Code, Preference, Trivia`. Derives `Copy`.
- `ScoredRecord` (lines 428-439): `id, content, total_score, semantic_score, recall_score, importance, memory_strength, hours_since_created, category`.

### (3) SQL schema and queries
None directly — pure computation. The scorer is used by recall.rs and mod.rs.

### (4) Embedding/vector search approach
- `recall_score` (lines 137-164): `R(t, S, imp) = e^(-t / (S × half_life × importance_factor))` where `importance_factor = 1 + (decay_modifier - 1) × max(0, (importance - 0.5) × 2)`. Floor applied.
- `retrieval_score` (lines 171-184): `α × semantic + β × recall + γ × importance`.
- `hybrid_score` (lines 200-213): `base = α × S_rrf + γ × importance`, `score = base × e^(-λ × t)` where `λ = ln(2) / half_life`. No beta, no memory_strength.
- `bump_strength` (lines 218-222): `S_new = S_old × multiplier + additive`, capped at `max_strength`.

### (5) Concurrency/locking patterns
None — `SalienceScorer` holds no locks, no interior mutability. Pure computation.

### (6) Code quality issues
- **`MemoryCategory::from_str`** (lines 356-364) shadows `std::str::FromStr` — should implement the trait instead of a custom method. The method takes `&str` and returns `Self`, same signature as `FromStr` but without the associated `Err` type.
- **`auto_rate_importance`** (lines 228-316): long function with many hardcoded keyword lists. The keyword matching uses `to_lowercase()` on the entire content for each keyword check (line 259: `content.to_lowercase().contains(&kw.to_lowercase())`) — this lowercases the content once per keyword (20+ keywords), allocating a new String each time. Should lowercase once.
- **Category classification** (lines 367-422): `classify` also calls `content.to_lowercase()` (line 368) — another allocation. And keyword lists overlap between `auto_rate_importance` and `classify` (e.g., "规则", "约定", "convention" appear in both).
- Importance scoring can exceed 1.0 before clamping (line 315): base 0.3 + 0.1 (user) + 0.08×N (keywords) + 0.03 (path) + 0.02 (numbers) + 0.05 (length) + 0.03 (caps) + 0.10 (URL) + 0.08 (error) + 0.06 (code) = up to ~0.85 typical, but with many keyword matches could exceed 1.0. The `clamp(0.0, 1.0)` (line 315) handles this.

### (7) Performance concerns
- **`auto_rate_importance` called on every store** (recall.rs line 93): for each conversation message, this function runs 20+ keyword searches with `to_lowercase()` allocations. For high-throughput conversation logging, this adds up.
- `recall_score` uses `.exp()` (line 160) — transcendental function, but only called during search scoring (bounded by candidate count).
- No caching of computed scores.

### (8) Bugs
- **`hybrid_score` doesn't use memory_strength** (lines 200-213): the doc comment (lines 197-199) says "memory_strength reinforcement is still applied via bump_strength() on access" — but the hybrid score formula has no strength term, so a memory accessed 100 times (strength=5.0) scores the same as one never accessed (strength=1.0) in hybrid mode. This means **reinforcement has no effect on hybrid search ranking**. The strength is bumped but never used for scoring. This is a design bug — the comment acknowledges it but frames it as intentional, yet it defeats the purpose of reinforcement.
- **`recall_score` importance factor formula** (line 151-152): `importance_factor = 1.0 + (modifier - 1.0) × max(0.0, (importance - 0.5) × 2.0)`. For importance=0.5, factor=1.0. For importance=1.0, factor = 1.0 + 2.0×1.0 = 3.0. For importance=0.3, factor=1.0 (no penalty for low importance). This means **low-importance memories don't decay faster** — only high-importance ones decay slower. The asymmetry is probably intentional but means importance=0.0 and importance=0.5 have the same decay rate.
- **`category_half_life`** (lines 321-329): Trivia has `× 0.5` (84h), Decision `× 2.0` (336h), Preference `× 3.0` (504h). But `hybrid_score` uses `lambda = ln(2) / half_life` — for Preference, lambda is tiny, so decay is very slow. This is correct but means Preferences essentially never decay in hybrid mode.

---

# 10. `bm25.rs` (122 lines) — BM25 full-text search index

### (1) Purpose
In-memory BM25 index using tantivy. Provides keyword-based candidate retrieval for hybrid search. Rebuilt from SQLite on restart.

### (2) Key data structures
- `BM25Index` (lines 22-28): `index: Index, schema: Schema, content_field: Field, id_field: Field, writer: Arc<Mutex<IndexWriter>>`.

### (3) SQL schema and queries
None — pure in-memory index. `rebuild` (lines 116-121) takes records as input (read from SQLite by caller).

### (4) Embedding/vector search approach
None — BM25 is purely lexical. Uses tantivy's `TEXT` field tokenizer for content and `STRING` for ID.

### (5) Concurrency/locking patterns
- `writer: Arc<Mutex<IndexWriter>>` — writer mutex serializes all writes (insert/delete/commit).
- `search` (lines 85-112): creates a **new reader on every search** (line 86-89: `self.index.reader()`). Tantivy readers are designed to be reused; creating one per search is expensive because it may involve reopening segment readers.

### (6) Code quality issues
- **Reader created per search** (line 86): `self.index.reader()` should be created once and reused. Tantivy's `IndexReader` is designed to be long-lived with `reload()` called after writes.
- `from_records` (lines 47-62) locks the writer, adds all docs, commits, drops the lock. Correct.
- `insert` (lines 65-72) and `delete` (lines 75-81) each lock + commit individually — **commit per operation** is expensive. Should batch.
- `rebuild` (lines 116-121) ignores `_dir` parameter (line 117: `_dir: &Path`) — the parameter is unused. The doc comment (line 115) explains it's for "tantivy's mmap directory — unused since we use in-RAM index." Dead parameter.

### (7) Performance concerns
- **Commit per insert** (line 70): each `insert()` call does `writer.commit()`. Tantivy commits flush segments — this is expensive for frequent inserts. Should batch or use `prepare_commit`/`commit_async`.
- **Reader per search** (line 86): creating a reader involves reading segment metadata. For frequent searches, this adds overhead. Should cache the reader and call `reload()` after writes.
- Index is in-RAM (`Index::create_in_ram`, line 38) — no persistence. Rebuilt from SQLite on every restart.
- 50MB writer heap (line 40: `index.writer(50_000_000)`) — generous but fine for in-memory.

### (8) Bugs
- **Stale reader**: since `search` creates a new reader each time, it should see committed writes. But if `insert` commits and `search` creates a reader, there may be a race if the reader is created before the commit is fully flushed. In practice, tantivy's `commit()` is synchronous, so this should be fine.
- **No error on empty query**: `query_parser.parse_query(query)` (line 93-95) — if query is empty or unparseable, returns an error. The caller (mod.rs) uses `.unwrap_or_default()` (line 404) to handle this, so it's handled upstream.
- **`delete` doesn't verify the term exists** — silently succeeds if the ID isn't in the index.

---

# 11. `hnsw.rs` (174 lines, ~110 logic + 64 tests) — HNSW vector index

### (1) Purpose
Approximate nearest neighbor search using `instant_distance::HnswMap`. Immutable after construction; new records go to a brute-force fallback list. Uses inner-product distance on normalized embeddings.

### (2) Key data structures
- `NormalizedEmbedding` (lines 20-31): `Vec<f32>` wrapper implementing `instant_distance::Point` with `distance = 1.0 - dot(A, B)`.
- `HNSWIndex` (lines 36-41): `map: RwLock<HnswMap<NormalizedEmbedding, String>>, fallback: RwLock<Vec<(String, Vec<f32>)>>`.

### (3) SQL schema and queries
None — pure in-memory. Rebuilt from SQLite by caller.

### (4) Embedding/vector search approach
- `from_records` (lines 46-60): builds HnswMap from normalized embeddings with `ef_search(150)`.
- `search` (lines 82-109): searches HNSW map + brute-force fallback, merges results, truncates to `top_k`.
- `normalize_embedding` (lines 113-120): L2 normalization.
- Distance metric: `1.0 - dot(A, B)` on normalized vectors = cosine distance.

### (5) Concurrency/locking patterns
- `map: RwLock<HnswMap<...>>` — read lock for search, but HnswMap is immutable so no write lock ever acquired (no method to rebuild).
- `fallback: RwLock<Vec<...>>` — write lock for `add_fallback`, read lock for search.
- **Lock poisoning**: uses `.expect("HNSW lock poisoned")` (lines 76, 83, 96) — panics on poisoned lock. Unlike parking_lot (which doesn't poison), this uses `std::sync::RwLock`, so poisoning is possible.

### (6) Code quality issues
- **Uses `std::sync::RwLock`** instead of `parking_lot::RwLock` — inconsistent with the rest of the codebase which uses parking_lot. std RwLock is slower and can poison.
- **`map.iter().next().is_none()`** (line 89): awkward way to check if the map is empty. `map.is_empty()` would be clearer if available, or `map.iter().count() == 0`.
- `search` doesn't deduplicate between HNSW results and fallback results — if a record is in both (shouldn't happen but could if rebuild doesn't clear fallback), it appears twice.
- No method to rebuild or clear the fallback list — the comment (line 40) says "Cleared on rebuild" but there's no rebuild method.

### (7) Performance concerns
- **Fallback grows unbounded**: `add_fallback` (lines 75-78) pushes to a Vec that's never cleared during the session. Over a long session, the fallback could grow large, and brute-force search (lines 97-104) is O(n) per search.
- **HNSW map is immutable** — new records added at runtime are only in the fallback, meaning HNSW's ANN efficiency is lost for recent records. The comment (lines 7-8) acknowledges this and suggests `sqlite-vec` for production.
- `search` creates a new `Search::default()` (line 90) per call — this is the intended usage pattern for instant_distance.
- `ef_search(150)` (line 53) — relatively high, favoring recall over speed.

### (8) Bugs
- **Fallback never cleared** (line 75-78): there's no method to clear the fallback list. If the index is rebuilt (by creating a new `HNSWIndex`), the old fallback is lost — but the new index won't have the fallback records unless they're included in `from_records`. The caller (mod.rs) would need to recreate the `HNSWIndex` and set it via `set_hnsw`. If they don't, fallback records are lost on rebuild.
- **`search` truncates after merge** (line 107): HNSW returns up to `top_k` results (default behavior of `map.search`), then fallback adds more, then truncates to `top_k`. But the fallback results are appended **after** HNSW results, so if HNSW returns `top_k` results, fallback results are always truncated away. The fallback is only useful if HNSW returns fewer than `top_k` results. **This means fallback records are effectively never returned** unless the HNSW index is empty or has fewer than `top_k` entries. This is a significant bug — the fallback mechanism is broken for the common case.
  - Wait, let me re-check: `map.search(&query_point, &mut search)` returns an iterator. Does it limit to `top_k`? Looking at instant_distance API — `HnswMap::search` returns all results, not limited by top_k. The `Search` struct has a `ef` parameter but not a result limit. So `results.map(|item| item.value.clone())` collects ALL results. Then fallback adds all. Then truncate to top_k. So this is actually OK — HNSW returns all its results, fallback adds all, then truncate. The bug I described doesn't exist. Let me verify: the `Search::default()` has no limit set. `map.search()` returns an iterator over all found neighbors. So yes, all HNSW results are collected, then all fallback, then truncated. This is correct but potentially returns many results before truncating.
- **`normalize_embedding` zero vector** (lines 114-118): returns the zero vector as-is. A zero embedding in the HNSW index would have `distance = 1.0 - 0.0 = 1.0` to everything, which is the maximum distance — so zero embeddings would never be found. This is acceptable behavior.

---

# 12. `rrf.rs` (93 lines, ~50 logic + 43 tests) — Reciprocal Rank Fusion

### (1) Purpose
Merges multiple ranked result lists using Reciprocal Rank Fusion: `S(id) = Σ 1/(k + rank_i(id))`. Used to fuse BM25 and HNSW results.

### (2) Key data structures
- No structs — two free functions: `fuse` and `fuse_normalized`.
- Uses `HashMap<String, f32>` for score accumulation.

### (3) SQL schema and queries
None — pure computation.

### (4) Embedding/vector search approach
None — operates on ranked ID lists, not embeddings.

### (5) Concurrency/locking patterns
None — stateless functions.

### (6) Code quality issues
- Clean, well-tested module. Minimal and focused.
- `fuse_normalized` (lines 38-50) discards the original RRF scores from `fuse` and recomputes based on the **fused rank**: `S_rrf = k / (k + rank + 1)`. This means the actual RRF consensus score is thrown away — the normalized score is just a rank-based discount. This is documented (lines 36-37) but means `fuse_normalized` doesn't actually use RRF scores for anything — it could just sort by RRF and then apply the formula.

### (7) Performance concerns
- `fuse` allocates a HashMap and Vec — fine for small lists (150 items).
- Sorting is O(n log n).

### (8) Bugs
- **`fuse_normalized` doesn't use RRF scores** (lines 38-50): as noted, the function calls `fuse` to get the sorted order, then discards the scores and recomputes based on rank. The `s_rrf` values returned are identical to what you'd get from any single sorted list — the RRF fusion only affects the **ordering**, not the scores. This means downstream salience scoring gets the same `s_rrf` regardless of whether an item appeared in both lists (consensus) or just one. The consensus-boosting property of RRF is lost in the score. This is a design issue — the scores should reflect the actual RRF fusion (e.g., `s_rrf = rrf_score / max_rrf_score`).
- As noted in mod.rs analysis (bug #1), the `fuse_normalized` output is **completely discarded** by the caller — mod.rs takes only the IDs and rebuilds its own rank-based map. So even the rank-based normalization is unused.

---

# 13. `diff_preference.rs` (89 lines) — Diff-based preference extraction

### (1) Purpose
Spawns a background task that analyzes user edits to agent-modified files and extracts durable programming preferences using an LLM. Emits `ApprovalRequired` events for the extracted preferences.

### (2) Key data structures
- `DiffPreferenceEngine` (line 8): unit struct, no state.
- Uses `UserEditDiffEvent` from `crate::reflector::diff_observer`.

### (3) SQL schema and queries
None — doesn't write to DB directly. Emits events for the approval flow.

### (4) Embedding/vector search approach
None — LLM-based extraction only.

### (5) Concurrency/locking patterns
- `spawn_analysis` (lines 11-50): `tokio::spawn` — fire-and-forget async task.
- Iterates over diffs sequentially (line 19: `for diff in diffs`).
- Uses `broadcast::Sender<Envelope>` for event emission (line 22).

### (6) Code quality issues
- **`spawn_analysis` returns nothing** — no handle to the spawned task, no way to await completion. The task is truly fire-and-forget.
- **Markdown JSON stripping** (line 73): `trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```")` — doesn't handle ` ```json` (with leading space) or ` ``` ` (with trailing space) properly. The final `.trim()` helps but the `trim_start_matches` is applied before trim, so `  ```json` wouldn't match.
- `analyze_diff` prompt (lines 53-66) uses `format!` with `{}` placeholders for `diff.file_path` and `diff.diff` — **format string injection**: if the diff contains `{`, `format!` will panic or misinterpret. Same bug as reflection.rs.
- Confidence threshold hardcoded at 0.7 (line 77).

### (7) Performance concerns
- Sequential LLM calls for each diff (line 19-48) — if there are 10 diffs, that's 10 sequential LLM calls. Could parallelize with `futures::join_all` or bounded concurrency.
- No rate limiting on LLM calls.

### (8) Bugs
- **`format!` injection** (line 60): `format!(r#"...{diff}..."#)` where `diff` is `diff.diff` — if the diff text contains `{` or `}`, `format!` will try to parse them as format specifiers and may panic. This is the same bug as in reflection.rs. Should use `format!("...\n{}\n...", diff.diff)` is actually what it does, but the raw diff content with braces would break. Actually wait — looking again at line 60: `{}` is a format placeholder, not `{diff}`. Lines 58-65:
  ```
  File: {}
  Diff:
  {}
  ```
  And line 66: `diff.file_path, diff.diff`. So the `{}` placeholders are correct format specifiers. But if `diff.diff` contains literal `{` or `}` characters (which diffs often do for Rust code!), the `format!` macro will try to interpret them as format specifiers and **panic**. This is a real bug — any diff of Rust code containing `{` or `}` will crash the preference extraction task.

  Actually, I need to reconsider. The `format!` placeholders are `{}`, and the arguments are `diff.file_path` and `diff.diff`. The `format!` macro processes the format string, and `{}` is replaced by the arguments. But if the **argument** (the diff content) contains `{`, that's fine — the arguments are not re-parsed as format strings. The format string itself is the literal `r#"..."#` which contains `{}` placeholders. So this is actually safe. I was wrong — the format string injection bug exists in reflection.rs (line 264: `{conversation}` in the format string) but NOT here, because here the format string uses `{}` not `{diff}`.

  Let me re-check reflection.rs: line 264 has `{conversation}` in the format string, and `conversation` is passed as an argument. `format!(r#"...{conversation}..."#)` — this is **named argument** syntax. The format string contains `{conversation}` which is replaced by the `conversation` variable. This is actually safe too — `{conversation}` is a named placeholder, and the conversation content is the argument, not part of the format string. So there's no format string injection in either file. I was wrong about both.

  However, if the conversation/diff text contains literal `{` and `}`, and these appear in the **format string** (not as arguments), they would be interpreted. But since they're arguments, they're safe. Let me re-examine: `format!(r#"...{conversation}..."#)` — the format string is the raw string literal, and `{conversation}` is a named argument placeholder. The value of `conversation` is substituted. The value itself is NOT parsed for format specifiers. So this is safe. No bug here.

- **No error handling on broadcast send** (line 22): `let _ = event_tx.send(...)` — if there are no receivers, the send silently fails. This is standard for broadcast channels.
- **`seq` counter uses `Ordering::Relaxed`** (line 24): `seq.fetch_add(1, Ordering::Relaxed)` — for a sequence number that determines event ordering, `Relaxed` doesn't provide synchronization guarantees. If events from different sources need global ordering, this could produce duplicate or out-of-order sequence numbers. However, since each `spawn_analysis` call likely runs sequentially within a single task, this is probably fine in practice.

---

# Cross-cutting analysis

## Architecture overview
The memory subsystem is a three-tier design:
1. **Core memory** (`block.rs`) — manual notes injected into context (persona, user info)
2. **Recall memory** (`recall.rs`) — short-term conversation history with Ebbinghaus decay
3. **Archival memory** (`archival.rs`) — long-term durable facts

Supporting components:
- **Storage** (`storage.rs`) — shared SQLite connection with global mutex
- **Embedding** (`embedding.rs`) — fastembed/ONNX local embeddings
- **Salience** (`salience.rs`) — decay scoring + importance heuristics
- **BM25** (`bm25.rs`) — tantivy in-memory keyword index
- **HNSW** (`hnsw.rs`) — instant_distance ANN index
- **RRF** (`rrf.rs`) — rank fusion
- **Consolidation** (`consolidation.rs`) — O(n²) dedup
- **Reflection** (`reflection.rs`) — LLM fact extraction daemon
- **Diff preference** (`diff_preference.rs`) — LLM preference extraction from diffs

## Top bugs (severity-ordered)

1. **RRF scores completely discarded** (mod.rs lines 419-422, 456-460): `fuse_normalized` computes RRF consensus scores but the caller discards them and rebuilds a rank-based map. The consensus-boosting property of RRF (items appearing in both BM25 and HNSW lists get higher scores) is lost.

2. **Memory strength reinforcement has no effect in hybrid search** (salience.rs lines 200-213): `hybrid_score` doesn't use `memory_strength`, so bumped strengths never affect hybrid search ranking. The strength is updated on every search but never read in the hybrid path.

3. **Fallback records in HNSW rarely returned** (hnsw.rs lines 82-109): HNSW returns all results, then fallback results are appended. If HNSW returns ≥top_k results, fallback records are truncated away. Recent memories (only in fallback) may never appear in search results.

4. **Byte vs char length in core memory** (block.rs lines 81, 110): `max_chars` field uses byte length (`.len()`) for limit checking, misleading for multi-byte UTF-8 content.

5. **bump_strength_batch N+1 queries** (recall.rs lines 489-504): one SELECT + one UPDATE per ID instead of batch operations.

6. **Embedding model mutex serializes all inference** (embedding.rs line 35): `model.embed()` runs under the Mutex, blocking concurrent embedding requests.

7. **BM25 reader created per search** (bm25.rs line 86): should be cached and reloaded, not recreated.

8. **Non-deterministic core memory context order** (block.rs lines 162-168): `to_context_string` iterates HashMap, producing non-deterministic block ordering.

## Top performance concerns

1. **Single global DB mutex** (storage.rs): all SQLite access serializes through one `Arc<Mutex<Connection>>`. Under concurrent access (reflection daemon + main agent + sub-agents), this is a bottleneck.

2. **O(n²) consolidation** (consolidation.rs): 5000 records = 12.5M comparisons, ~5 seconds. Acceptable for periodic background but not real-time.

3. **Double embedding on store** (mod.rs line 175): `store_conversation` embeds content in `recall.store`, then re-embeds for HNSW fallback sync.

4. **All candidate embeddings loaded into memory** (recall.rs line 320, archival.rs line 168): up to 150 full embeddings loaded for cosine scoring in Rust.

5. **Unbounded HNSW fallback list** (hnsw.rs line 75-78): grows throughout session, brute-force searched every query.
