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
                cwd TEXT DEFAULT '',
                model_used TEXT DEFAULT '',
                tags TEXT DEFAULT '[]',
                archived INTEGER DEFAULT 0,
                parent_session_id TEXT DEFAULT '',
                session_type TEXT DEFAULT 'main',
                project_id TEXT DEFAULT '',
                created_at TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_sessions_updated ON sessions(updated_at);
            CREATE INDEX IF NOT EXISTS idx_sessions_archived ON sessions(archived);
            CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_session_id);

            CREATE TABLE IF NOT EXISTS session_messages (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                msg_index INTEGER NOT NULL,
                role TEXT NOT NULL,
                content TEXT DEFAULT '',
                tool_calls TEXT DEFAULT '[]',
                tool_call_id TEXT DEFAULT '',
                name TEXT DEFAULT '',
                created_at TEXT NOT NULL,
                UNIQUE(session_id, msg_index)
            );

            CREATE INDEX IF NOT EXISTS idx_session_msgs ON session_messages(session_id);

            CREATE TABLE IF NOT EXISTS session_event_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                turn_index INTEGER NOT NULL,
                event_type TEXT NOT NULL,
                payload TEXT DEFAULT '{}',
                started_at TEXT,
                ended_at TEXT,
                created_at TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_session_events ON session_event_log(session_id);
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
            ",
        )?;

        // Migrate existing databases: add columns if missing (idempotent)
        let migrations = &[
            "ALTER TABLE recall_memory ADD COLUMN memory_strength REAL DEFAULT 1.0",
            "ALTER TABLE recall_memory ADD COLUMN access_count INTEGER DEFAULT 0",
            "ALTER TABLE recall_memory ADD COLUMN last_accessed_at TEXT",
            "ALTER TABLE recall_memory ADD COLUMN category TEXT DEFAULT 'Conversation'",
            "ALTER TABLE sessions ADD COLUMN parent_session_id TEXT DEFAULT ''",
            "ALTER TABLE sessions ADD COLUMN session_type TEXT DEFAULT 'main'",
            "ALTER TABLE sessions ADD COLUMN project_id TEXT DEFAULT ''",
            "ALTER TABLE sessions ADD COLUMN process_time_ms INTEGER DEFAULT 0",
            "ALTER TABLE sessions ADD COLUMN thought_time_ms INTEGER DEFAULT 0",
        ];
        for migration in migrations {
            let _ = db.execute_batch(migration);
        }

        Ok(())
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.db.lock()
    }
}
