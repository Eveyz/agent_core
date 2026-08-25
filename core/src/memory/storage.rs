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
        let table_exists: bool = conn
            .query_row(
                "SELECT 1 FROM sqlite_master WHERE type='table' AND name='session_messages'",
                [],
                |_| Ok(true),
            )
            .unwrap_or(false);

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
        storage.add_column_if_not_exists(
            "agent_message_tasks",
            "attempt_count",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        storage.add_column_if_not_exists(
            "agent_messages",
            "priority",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        storage.add_column_if_not_exists(
            "agent_swarm_runs",
            "max_hops",
            "INTEGER NOT NULL DEFAULT 12",
        )?;
        storage.add_column_if_not_exists(
            "agent_swarm_runs",
            "hops_used",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        storage.add_column_if_not_exists("agent_swarm_runs", "completion_task_id", "TEXT")?;
        storage.add_column_if_not_exists("agent_swarm_runs", "completion_turn_id", "TEXT")?;
        storage.add_column_if_not_exists(
            "agent_message_tasks",
            "worker_id",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        storage.add_column_if_not_exists(
            "recall_memory",
            "reflection_sequence",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        storage.add_column_if_not_exists(
            "reflection_state",
            "last_reflected_sequence",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        storage.add_column_if_not_exists(
            "reflection_state",
            "claim_token",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        storage.add_column_if_not_exists(
            "reflection_state",
            "last_error_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        storage.add_column_if_not_exists(
            "reflection_facts",
            "agverse_owned",
            "INTEGER NOT NULL DEFAULT 0",
        )?;
        // P2 fact metadata: scope / status / provenance for lifecycle.
        storage.add_column_if_not_exists(
            "reflection_facts",
            "scope",
            "TEXT NOT NULL DEFAULT 'global'",
        )?;
        storage.add_column_if_not_exists(
            "reflection_facts",
            "status",
            "TEXT NOT NULL DEFAULT 'active'",
        )?;
        storage.add_column_if_not_exists(
            "reflection_facts",
            "source_session",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        storage.add_column_if_not_exists(
            "reflection_facts",
            "updated_at",
            "TEXT NOT NULL DEFAULT ''",
        )?;
        storage.initialize_reflection_sequence()?;
        storage.recover_reflection_file_operations()?;
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
                created_at TEXT NOT NULL,
                reflection_sequence INTEGER NOT NULL DEFAULT 0
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

            CREATE TABLE IF NOT EXISTS reflection_state (
                session_id TEXT PRIMARY KEY,
                last_reflected_sequence INTEGER NOT NULL DEFAULT 0,
                status TEXT NOT NULL DEFAULT 'idle',
                last_attempt_at TEXT NOT NULL DEFAULT '',
                last_success_at TEXT NOT NULL DEFAULT '',
                last_error TEXT NOT NULL DEFAULT '',
                last_error_at TEXT NOT NULL DEFAULT '',
                claim_token TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_reflection_state_updated
                ON reflection_state(updated_at);

            CREATE TABLE IF NOT EXISTS reflection_facts (
                fact_key TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                section TEXT NOT NULL,
                archival_id TEXT NOT NULL,
                agverse_owned INTEGER NOT NULL DEFAULT 0,
                scope TEXT NOT NULL DEFAULT 'global',
                status TEXT NOT NULL DEFAULT 'active',
                source_session TEXT NOT NULL DEFAULT '',
                updated_at TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS reflection_fact_sources (
                fact_key TEXT NOT NULL,
                session_id TEXT NOT NULL,
                created_at TEXT NOT NULL,
                PRIMARY KEY (fact_key, session_id)
            );

            CREATE TABLE IF NOT EXISTS reflection_control (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                enabled INTEGER NOT NULL DEFAULT 0,
                updated_at TEXT NOT NULL
            );

            INSERT OR IGNORE INTO reflection_control(singleton, enabled, updated_at)
                VALUES (1, 0, '');

            CREATE TABLE IF NOT EXISTS reflection_file_operations (
                id TEXT PRIMARY KEY,
                path TEXT NOT NULL,
                original_content TEXT,
                updated_content TEXT NOT NULL,
                state TEXT NOT NULL,
                created_at TEXT NOT NULL
            );

            CREATE TABLE IF NOT EXISTS deleted_reflection_sessions (
                session_id TEXT PRIMARY KEY,
                deleted_at TEXT NOT NULL
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

            CREATE TABLE IF NOT EXISTS subagent_lineage (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                parent_session_id TEXT NOT NULL DEFAULT '',
                parent_run_id TEXT NOT NULL DEFAULT '',
                parent_call_id TEXT NOT NULL DEFAULT '',
                child_run_id TEXT NOT NULL,
                created_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_subagent_lineage_parent_run
                ON subagent_lineage(parent_run_id, parent_call_id);

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

            CREATE TABLE IF NOT EXISTS session_model_windows (
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                source_message_count INTEGER NOT NULL,
                source_prefix_sha256 TEXT NOT NULL,
                model_id TEXT NOT NULL DEFAULT '',
                messages_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session_msgs ON session_messages(session_id);

            CREATE TABLE IF NOT EXISTS session_plans (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                title TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'parked',
                source_prompt_id TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_session_plans_session
                ON session_plans(session_id);
            CREATE INDEX IF NOT EXISTS idx_session_plans_status
                ON session_plans(session_id, status);
            CREATE INDEX IF NOT EXISTS idx_session_plans_prompt
                ON session_plans(session_id, source_prompt_id);

            CREATE TABLE IF NOT EXISTS session_plan_items (
                plan_id TEXT NOT NULL REFERENCES session_plans(id) ON DELETE CASCADE,
                item_id TEXT NOT NULL,
                description TEXT NOT NULL DEFAULT '',
                status TEXT NOT NULL DEFAULT 'pending',
                depends_on TEXT NOT NULL DEFAULT '[]',
                created_at TEXT NOT NULL,
                completed_at TEXT,
                PRIMARY KEY (plan_id, item_id)
            );
            CREATE INDEX IF NOT EXISTS idx_session_plan_items_plan
                ON session_plan_items(plan_id);

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

            -- Local, A2A-inspired agent messaging. Agent ids and display names
            -- are deliberately snapshotted instead of foreign-keyed so deleting
            -- an agent definition does not erase the audit trail.
            CREATE TABLE IF NOT EXISTS agent_conversations (
                id TEXT PRIMARY KEY,
                agent_id TEXT NOT NULL,
                project_id TEXT NOT NULL DEFAULT '',
                session_id TEXT NOT NULL UNIQUE REFERENCES sessions(id) ON DELETE CASCADE,
                unread_count INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                UNIQUE(agent_id, project_id)
            );

            CREATE INDEX IF NOT EXISTS idx_agent_conversations_agent
                ON agent_conversations(agent_id, updated_at);

            CREATE TABLE IF NOT EXISTS agent_messages (
                id TEXT PRIMARY KEY,
                schema_version TEXT NOT NULL,
                context_id TEXT NOT NULL,
                from_agent_id TEXT NOT NULL,
                from_revision_id TEXT NOT NULL,
                from_display_name TEXT NOT NULL,
                to_agent_id TEXT NOT NULL,
                to_revision_id TEXT NOT NULL,
                to_display_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                parts TEXT NOT NULL,
                correlation_id TEXT NOT NULL,
                reply_to TEXT,
                source_conversation_id TEXT NOT NULL REFERENCES agent_conversations(id),
                target_conversation_id TEXT NOT NULL REFERENCES agent_conversations(id),
                project_id TEXT NOT NULL DEFAULT '',
                idempotency_key TEXT NOT NULL UNIQUE,
                hop_count INTEGER NOT NULL,
                priority INTEGER NOT NULL DEFAULT 0,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_messages_context
                ON agent_messages(context_id, created_at);
            CREATE INDEX IF NOT EXISTS idx_agent_messages_target
                ON agent_messages(target_conversation_id, created_at);

            CREATE TABLE IF NOT EXISTS agent_message_tasks (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL UNIQUE REFERENCES agent_messages(id),
                recipient_agent_id TEXT NOT NULL,
                recipient_conversation_id TEXT NOT NULL REFERENCES agent_conversations(id),
                status TEXT NOT NULL,
                output_message_id TEXT REFERENCES agent_messages(id),
                error TEXT NOT NULL DEFAULT '',
                attempt_count INTEGER NOT NULL DEFAULT 0,
                worker_id TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_message_tasks_status
                ON agent_message_tasks(status, updated_at);

            CREATE TABLE IF NOT EXISTS agent_message_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                conversation_id TEXT NOT NULL REFERENCES agent_conversations(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                message_id TEXT,
                task_id TEXT,
                payload TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_message_events_conversation
                ON agent_message_events(conversation_id, sequence);

            CREATE TABLE IF NOT EXISTS agent_swarm_runs (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL DEFAULT '',
                root_agent_id TEXT NOT NULL,
                goal TEXT NOT NULL,
                status TEXT NOT NULL,
                max_messages INTEGER NOT NULL,
                messages_used INTEGER NOT NULL DEFAULT 0,
                max_turns INTEGER NOT NULL,
                turns_used INTEGER NOT NULL DEFAULT 0,
                max_hops INTEGER NOT NULL DEFAULT 12,
                hops_used INTEGER NOT NULL DEFAULT 0,
                summary TEXT NOT NULL DEFAULT '',
                error TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL,
                completed_at TEXT,
                completion_task_id TEXT,
                completion_turn_id TEXT
            );

            CREATE INDEX IF NOT EXISTS idx_agent_swarm_runs_project
                ON agent_swarm_runs(project_id, updated_at);

            CREATE TABLE IF NOT EXISTS agent_swarm_active_turns (
                run_id TEXT NOT NULL REFERENCES agent_swarm_runs(id) ON DELETE CASCADE,
                turn_id TEXT NOT NULL,
                agent_id TEXT NOT NULL,
                lane TEXT NOT NULL,
                started_at TEXT NOT NULL,
                PRIMARY KEY (run_id, turn_id)
            );

            CREATE TABLE IF NOT EXISTS agent_swarm_participants (
                run_id TEXT NOT NULL REFERENCES agent_swarm_runs(id) ON DELETE CASCADE,
                agent_id TEXT NOT NULL,
                joined_at TEXT NOT NULL,
                PRIMARY KEY(run_id, agent_id)
            );

            CREATE TABLE IF NOT EXISTS agent_swarm_messages (
                run_id TEXT NOT NULL REFERENCES agent_swarm_runs(id) ON DELETE CASCADE,
                message_id TEXT NOT NULL REFERENCES agent_messages(id),
                PRIMARY KEY(run_id, message_id)
            );

            CREATE TABLE IF NOT EXISTS agent_swarm_events (
                sequence INTEGER PRIMARY KEY AUTOINCREMENT,
                run_id TEXT NOT NULL REFERENCES agent_swarm_runs(id) ON DELETE CASCADE,
                event_type TEXT NOT NULL,
                payload TEXT NOT NULL DEFAULT '{}',
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_agent_swarm_events_run
                ON agent_swarm_events(run_id, sequence);

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

    fn initialize_reflection_sequence(&self) -> Result<()> {
        let db = self.db.lock();
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS reflection_sequence_counter (
                singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                value INTEGER NOT NULL
            );
            INSERT OR IGNORE INTO reflection_sequence_counter(singleton, value) VALUES (1, 0);
            UPDATE recall_memory SET reflection_sequence = rowid WHERE reflection_sequence = 0;
            UPDATE reflection_sequence_counter SET value = MAX(
                value,
                COALESCE((SELECT MAX(reflection_sequence) FROM recall_memory), 0)
            ) WHERE singleton = 1;
            CREATE UNIQUE INDEX IF NOT EXISTS idx_recall_reflection_sequence
                ON recall_memory(reflection_sequence);",
        )?;
        Ok(())
    }

    fn recover_reflection_file_operations(&self) -> Result<()> {
        let operations: Vec<(String, String, Option<String>, String, String)> = {
            let db = self.db.lock();
            let mut stmt = db.prepare(
                "SELECT id, path, original_content, updated_content, state \
                 FROM reflection_file_operations ORDER BY created_at",
            )?;
            stmt.query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for (id, path, original, updated, state) in operations {
            let path = std::path::PathBuf::from(path);
            let target = if state == "committed" {
                Some(updated)
            } else {
                original
            };
            if let Some(content) = target {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                let temp = path
                    .with_extension(format!("reflection-recovery-{}.tmp", uuid::Uuid::new_v4()));
                std::fs::write(&temp, content)?;
                std::fs::rename(temp, &path)?;
            } else if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(e.into());
            }
            self.db
                .lock()
                .execute("DELETE FROM reflection_file_operations WHERE id = ?1", [id])?;
        }
        Ok(())
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
            .with_context(|| format!("failed to add column {} to table {}", column, table))?;
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
