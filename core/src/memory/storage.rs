use anyhow::{Context, Result};
use parking_lot::{Mutex, MutexGuard};
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;

#[derive(Clone)]
pub struct Storage {
    db: Arc<Mutex<Connection>>,
}

impl Storage {
    pub fn new(path: &str) -> Result<Self> {
        let expanded = if path.starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
            path.replacen("~", &home, 1)
        } else {
            path.to_string()
        };

        if let Some(parent) = Path::new(&expanded).parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("failed to create dir: {}", parent.display()))?;
        }

        let conn = Connection::open(&expanded)
            .with_context(|| format!("failed to open database: {expanded}"))?;

        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")?;

        // ── Scheme B Schema Migration ────────────────────────────────
        // Older databases may lack the prompt/model/metadata columns on
        // session_messages. Migrate in place; never drop user session tables
        // during startup.
        let table_exists: bool = conn.query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_messages'",
            [],
            |_| Ok(true),
        ).unwrap_or(false);

        if table_exists {
            let mut stmt = conn.prepare("PRAGMA table_info(session_messages)")?;
            let existing: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);

            for (column, definition) in [
                ("prompt_id", "TEXT NOT NULL DEFAULT ''"),
                ("model", "TEXT DEFAULT ''"),
                ("metadata", "TEXT DEFAULT '{}'"),
            ] {
                if !existing.iter().any(|c| c == column) {
                    conn.execute(
                        &format!("ALTER TABLE session_messages ADD COLUMN {column} {definition}"),
                        [],
                    )
                    .with_context(|| format!("failed to add {column} to session_messages"))?;
                }
            }
        }

        // Session-level pinned goal (survives Stop / resume).
        let sessions_exist: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='sessions'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if sessions_exist {
            let mut stmt = conn.prepare("PRAGMA table_info(sessions)")?;
            let existing: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for (column, definition) in [
                ("pinned_goal", "TEXT NOT NULL DEFAULT ''"),
                ("goal_completed", "INTEGER NOT NULL DEFAULT 0"),
                ("pinned", "INTEGER NOT NULL DEFAULT 0"),
                ("pinned_at", "TEXT NOT NULL DEFAULT ''"),
            ] {
                if !existing.iter().any(|c| c == column) {
                    conn.execute(
                        &format!("ALTER TABLE sessions ADD COLUMN {column} {definition}"),
                        [],
                    )
                    .with_context(|| format!("failed to add {column} to sessions"))?;
                }
            }
        }

        // Sidebar pin for projects.
        let projects_exist: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='projects'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);
        if projects_exist {
            let mut stmt = conn.prepare("PRAGMA table_info(projects)")?;
            let existing: Vec<String> = stmt
                .query_map([], |row| row.get::<_, String>(1))?
                .filter_map(|r| r.ok())
                .collect();
            drop(stmt);
            for (column, definition) in [
                ("pinned", "INTEGER NOT NULL DEFAULT 0"),
                ("pinned_at", "TEXT NOT NULL DEFAULT ''"),
            ] {
                if !existing.iter().any(|c| c == column) {
                    conn.execute(
                        &format!("ALTER TABLE projects ADD COLUMN {column} {definition}"),
                        [],
                    )
                    .with_context(|| format!("failed to add {column} to projects"))?;
                }
            }
        }

        let storage = Self {
            db: Arc::new(Mutex::new(conn)),
        };

        storage.init_tables()?;
        Ok(storage)
    }

    fn init_tables(&self) -> Result<()> {
        let db = self.db.lock();

        db.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS memory_blocks (
                id TEXT PRIMARY KEY,
                label TEXT NOT NULL,
                content TEXT NOT NULL,
                max_chars INTEGER DEFAULT 2000,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS recall_memory (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                importance REAL DEFAULT 0.5,
                memory_strength REAL DEFAULT 1.0,
                access_count INTEGER DEFAULT 0,
                last_accessed_at TEXT,
                category TEXT DEFAULT 'Conversation',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_recall_session ON recall_memory(session_id);
            CREATE INDEX IF NOT EXISTS idx_recall_created ON recall_memory(created_at);

            CREATE TABLE IF NOT EXISTS archival_memory (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                embedding BLOB,
                metadata TEXT,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS conversation_summaries (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                summary TEXT NOT NULL,
                message_range TEXT NOT NULL,
                embedding BLOB,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS projects (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                path TEXT NOT NULL,
                pinned INTEGER NOT NULL DEFAULT 0,
                pinned_at TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS sessions (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL DEFAULT 'Untitled',
                summary TEXT NOT NULL DEFAULT '',
                start_time TEXT NOT NULL,
                end_time TEXT,
                message_count INTEGER DEFAULT 0,
                prompt_count INTEGER DEFAULT 0,
                cwd TEXT DEFAULT '',
                model_used TEXT DEFAULT '',
                tags TEXT DEFAULT '[]',
                archived INTEGER DEFAULT 0,
                parent_session_id TEXT DEFAULT '',
                session_type TEXT DEFAULT 'main',
                project_id TEXT DEFAULT '',
                process_time_ms INTEGER DEFAULT 0,
                thought_time_ms INTEGER DEFAULT 0,
                mode TEXT NOT NULL DEFAULT 'build',
                pinned_goal TEXT NOT NULL DEFAULT '',
                goal_completed INTEGER NOT NULL DEFAULT 0,
                pinned INTEGER NOT NULL DEFAULT 0,
                pinned_at TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_archived ON sessions(archived);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);

            CREATE TABLE IF NOT EXISTS prompts (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                turn_index INTEGER NOT NULL,
                model TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'completed',
                token_usage TEXT NOT NULL DEFAULT '{}',
                started_at TEXT,
                ended_at TEXT,
                created_at TEXT NOT NULL,
                UNIQUE(session_id, turn_index)
            );

            CREATE INDEX IF NOT EXISTS idx_prompts_session ON prompts(session_id);

            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                prompt_id TEXT NOT NULL DEFAULT '',
                msg_index INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT DEFAULT '',
                tool_calls TEXT DEFAULT '[]',
                tool_call_id TEXT DEFAULT '',
                name TEXT DEFAULT '',
                model TEXT DEFAULT '',
                metadata TEXT DEFAULT '{}',
                created_at TEXT NOT NULL,
                UNIQUE(session_id, msg_index)
            );

            CREATE INDEX IF NOT EXISTS idx_session_msgs ON session_messages(session_id);

            CREATE TABLE IF NOT EXISTS cronjobs (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                cadence_type TEXT NOT NULL,
                cadence_value TEXT NOT NULL,
                prompt TEXT NOT NULL,
                project TEXT,
                skills TEXT NOT NULL DEFAULT '[]',
                permission_level TEXT NOT NULL,
                max_concurrency INTEGER,
                model TEXT DEFAULT '',
                enabled INTEGER NOT NULL DEFAULT 1,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS cronjob_runs (
                id TEXT PRIMARY KEY,
                cronjob_id TEXT NOT NULL REFERENCES cronjobs(id) ON DELETE CASCADE,
                session_id TEXT,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                status TEXT NOT NULL
            );

            -- ── PLAN-0009: User-Defined Agents & Multi-Agent Workflow ──

            CREATE TABLE IF NOT EXISTS agents (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                system_prompt TEXT NOT NULL DEFAULT '',
                model TEXT NOT NULL DEFAULT '',
                skills TEXT NOT NULL DEFAULT '[]',
                tools TEXT NOT NULL DEFAULT '[]',
                permission_mode TEXT NOT NULL DEFAULT 'standard',
                permission_rules TEXT NOT NULL DEFAULT '[]',
                max_iterations INTEGER NOT NULL DEFAULT 50,
                max_context_tokens INTEGER NOT NULL DEFAULT 32000,
                memory_enabled INTEGER NOT NULL DEFAULT 1,
                memory_group TEXT NOT NULL DEFAULT '',
                icon TEXT NOT NULL DEFAULT '',
                color TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agents_name ON agents(name);

            CREATE TABLE IF NOT EXISTS agent_memory (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                memory_key TEXT NOT NULL,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                embedding BLOB,
                importance REAL DEFAULT 0.5,
                memory_strength REAL DEFAULT 1.0,
                access_count INTEGER DEFAULT 0,
                last_accessed_at TEXT,
                category TEXT DEFAULT 'Conversation',
                source TEXT DEFAULT 'conversation',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_memory_key ON agent_memory(memory_key);
            CREATE INDEX IF NOT EXISTS idx_agent_memory_created ON agent_memory(created_at);

            CREATE TABLE IF NOT EXISTS agent_history (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL REFERENCES agents(id) ON DELETE CASCADE,
                session_id TEXT NOT NULL,
                workflow_run_id TEXT DEFAULT '',
                trigger TEXT NOT NULL DEFAULT 'manual',
                input TEXT NOT NULL,
                output TEXT NOT NULL DEFAULT '',
                iterations_used INTEGER DEFAULT 0,
                success INTEGER NOT NULL DEFAULT 1,
                model_used TEXT NOT NULL DEFAULT '',
                token_input INTEGER DEFAULT 0,
                token_output INTEGER DEFAULT 0,
                process_time_ms INTEGER DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_history_agent ON agent_history(agent_id);
            CREATE INDEX IF NOT EXISTS idx_agent_history_workflow ON agent_history(workflow_run_id);

            CREATE TABLE IF NOT EXISTS workflows (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                input_schema TEXT NOT NULL DEFAULT '{}',
                trust_mode TEXT NOT NULL DEFAULT 'inherit',
                max_concurrent INTEGER DEFAULT 3,
                on_node_failure TEXT NOT NULL DEFAULT 'abort',
                config TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS workflow_nodes (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
                node_type TEXT NOT NULL,
                label TEXT NOT NULL DEFAULT '',
                agent_id TEXT DEFAULT '',
                config TEXT NOT NULL DEFAULT '{}',
                position_x REAL DEFAULT 0,
                position_y REAL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wf_nodes_workflow ON workflow_nodes(workflow_id);

            CREATE TABLE IF NOT EXISTS workflow_edges (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
                source_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
                target_node_id TEXT NOT NULL REFERENCES workflow_nodes(id) ON DELETE CASCADE,
                source_handle TEXT DEFAULT '',
                target_handle TEXT DEFAULT '',
                label TEXT NOT NULL DEFAULT '',
                condition TEXT NOT NULL DEFAULT '',
                data_mapping TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wf_edges_workflow ON workflow_edges(workflow_id);
            CREATE INDEX IF NOT EXISTS idx_wf_edges_source ON workflow_edges(source_node_id);
            CREATE INDEX IF NOT EXISTS idx_wf_edges_target ON workflow_edges(target_node_id);

            CREATE TABLE IF NOT EXISTS workflow_runs (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL REFERENCES workflows(id) ON DELETE CASCADE,
                session_id TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'pending',
                input TEXT NOT NULL DEFAULT '{}',
                output TEXT NOT NULL DEFAULT '{}',
                error TEXT NOT NULL DEFAULT '',
                total_token_input INTEGER DEFAULT 0,
                total_token_output INTEGER DEFAULT 0,
                started_at TEXT NOT NULL,
                finished_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wf_runs_workflow ON workflow_runs(workflow_id);
            CREATE INDEX IF NOT EXISTS idx_wf_runs_status ON workflow_runs(status);

            CREATE TABLE IF NOT EXISTS workflow_run_node_results (
                id TEXT PRIMARY KEY,
                workflow_run_id TEXT NOT NULL REFERENCES workflow_runs(id) ON DELETE CASCADE,
                node_id TEXT NOT NULL,
                agent_history_id TEXT DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                input TEXT NOT NULL DEFAULT '{}',
                output TEXT NOT NULL DEFAULT '{}',
                error TEXT NOT NULL DEFAULT '',
                token_input INTEGER DEFAULT 0,
                token_output INTEGER DEFAULT 0,
                cost_usd REAL DEFAULT 0.0,
                latency_ms INTEGER DEFAULT 0,
                started_at TEXT,
                finished_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_wf_run_nodes ON workflow_run_node_results(workflow_run_id);
                        ",
        )?;

        // FTS5 full-text search indexes + triggers for automatic sync
        db.execute_batch(
            "
            CREATE VIRTUAL TABLE IF NOT EXISTS recall_memory_fts USING fts5(content, tokenize='unicode61');
            CREATE VIRTUAL TABLE IF NOT EXISTS archival_memory_fts USING fts5(content, tokenize='unicode61');

            CREATE TRIGGER IF NOT EXISTS recall_fts_ai AFTER INSERT ON recall_memory BEGIN
                INSERT INTO recall_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS recall_fts_ad AFTER DELETE ON recall_memory BEGIN
                DELETE FROM recall_memory_fts WHERE rowid = old.rowid;
            END;
            CREATE TRIGGER IF NOT EXISTS recall_fts_au AFTER UPDATE ON recall_memory BEGIN
                DELETE FROM recall_memory_fts WHERE rowid = old.rowid;
                INSERT INTO recall_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;

            CREATE TRIGGER IF NOT EXISTS archival_fts_ai AFTER INSERT ON archival_memory BEGIN
                INSERT INTO archival_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS archival_fts_ad AFTER DELETE ON archival_memory BEGIN
                DELETE FROM archival_memory_fts WHERE rowid = old.rowid;
            END;
            CREATE TRIGGER IF NOT EXISTS archival_fts_au AFTER UPDATE ON archival_memory BEGIN
                DELETE FROM archival_memory_fts WHERE rowid = old.rowid;
                INSERT INTO archival_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;

            INSERT OR IGNORE INTO recall_memory_fts(rowid, content) SELECT rowid, content FROM recall_memory;
            INSERT OR IGNORE INTO archival_memory_fts(rowid, content) SELECT rowid, content FROM archival_memory;

            -- FTS5 for agent_memory (PLAN-0009 per-agent memory)
            CREATE VIRTUAL TABLE IF NOT EXISTS agent_memory_fts USING fts5(content, tokenize='unicode61');

            CREATE TRIGGER IF NOT EXISTS agent_memory_fts_ai AFTER INSERT ON agent_memory BEGIN
                INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;
            CREATE TRIGGER IF NOT EXISTS agent_memory_fts_ad AFTER DELETE ON agent_memory BEGIN
                DELETE FROM agent_memory_fts WHERE rowid = old.rowid;
            END;
            CREATE TRIGGER IF NOT EXISTS agent_memory_fts_au AFTER UPDATE ON agent_memory BEGIN
                DELETE FROM agent_memory_fts WHERE rowid = old.rowid;
                INSERT INTO agent_memory_fts(rowid, content) VALUES (new.rowid, new.content);
            END;

            INSERT OR IGNORE INTO agent_memory_fts(rowid, content) SELECT rowid, content FROM agent_memory;
            ",
        )?;

        Ok(())
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.db.lock()
    }

    /// Idempotently add a column to an existing table.
    ///
    /// Uses `PRAGMA table_info` to check whether the column already exists
    /// before issuing `ALTER TABLE ... ADD COLUMN`, avoiding the
    /// "duplicate column name" error on re-runs.
    pub fn add_column_if_not_exists(
        &self,
        table: &str,
        column: &str,
        definition: &str,
    ) -> Result<()> {
        let db = self.db.lock();
        let mut stmt = db.prepare(&format!("PRAGMA table_info({})", table))?;
        let existing: Vec<String> = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(|r| r.ok())
            .collect();
        if !existing.iter().any(|c| c == column) {
            db.execute(
                &format!("ALTER TABLE {} ADD COLUMN {} {}", table, column, definition),
                [],
            )
            .with_context(|| {
                format!("failed to add column {} to table {}", column, table)
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scheme_b_migration_preserves_existing_session_messages() {
        let dir = tempfile::TempDir::new().unwrap();
        let db_path = dir.path().join("legacy.db");
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE sessions (
                    id TEXT PRIMARY KEY,
                    title TEXT NOT NULL DEFAULT 'Untitled',
                    summary TEXT NOT NULL DEFAULT '',
                    start_time TEXT NOT NULL,
                    end_time TEXT,
                    message_count INTEGER DEFAULT 0,
                    prompt_count INTEGER DEFAULT 0,
                    cwd TEXT DEFAULT '',
                    model_used TEXT DEFAULT '',
                    tags TEXT DEFAULT '[]',
                    archived INTEGER DEFAULT 0,
                    parent_session_id TEXT DEFAULT '',
                    session_type TEXT DEFAULT 'main',
                    project_id TEXT DEFAULT '',
                    process_time_ms INTEGER DEFAULT 0,
                    thought_time_ms INTEGER DEFAULT 0,
                    mode TEXT NOT NULL DEFAULT 'build',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );
                CREATE TABLE session_messages (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    msg_index INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    content TEXT DEFAULT '',
                    tool_calls TEXT DEFAULT '[]',
                    tool_call_id TEXT DEFAULT '',
                    name TEXT DEFAULT '',
                    created_at TEXT NOT NULL,
                    UNIQUE(session_id, msg_index)
                );
                INSERT INTO sessions (id, title, start_time, created_at, updated_at)
                VALUES ('s1', 'legacy', 'now', 'now', 'now');
                INSERT INTO session_messages (session_id, msg_index, role, content, created_at)
                VALUES ('s1', 0, 'user', 'hello', 'now');
                "#,
            )
            .unwrap();
        }

        let _storage = Storage::new(db_path.to_str().unwrap()).unwrap();
        let conn = Connection::open(&db_path).unwrap();
        let content: String = conn
            .query_row(
                "SELECT content FROM session_messages WHERE session_id = 's1' AND msg_index = 0",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let has_metadata: bool = conn
            .query_row(
                "SELECT 1 FROM pragma_table_info('session_messages') WHERE name = 'metadata'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

        assert_eq!(content, "hello");
        assert!(has_metadata);
    }
}
