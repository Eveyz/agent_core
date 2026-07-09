use anyhow::{Context, Result, bail};
use chrono::Utc;
use std::collections::HashMap;

use super::storage::Storage;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MemoryBlock {
    pub id: String,
    pub label: String,
    pub content: String,
    pub max_chars: usize,
    pub updated_at: String,
}

pub struct CoreMemory {
    storage: Storage,
    blocks: HashMap<String, MemoryBlock>,
    default_max_chars: usize,
}

impl CoreMemory {
    pub fn new(storage: Storage, default_max_chars: usize) -> Result<Self> {
        let mut cm = Self {
            storage,
            blocks: HashMap::new(),
            default_max_chars,
        };
        cm.load()?;
        Ok(cm)
    }

    fn load(&mut self) -> Result<()> {
        {
            let db = self.storage.conn();
            let mut stmt = db
                .prepare("SELECT id, label, content, max_chars, updated_at FROM memory_blocks")
                .context("failed to prepare memory_blocks query")?;

            let rows = stmt.query_map([], |row| {
                Ok(MemoryBlock {
                    id: row.get(0)?,
                    label: row.get(1)?,
                    content: row.get(2)?,
                    max_chars: row.get::<_, i64>(3)? as usize,
                    updated_at: row.get(4)?,
                })
            })?;

            self.blocks.clear();
            for row in rows {
                let block = row?;
                self.blocks.insert(block.id.clone(), block);
            }
        }

        if self.blocks.is_empty() {
            self.create("human", "User Info", "")?;
            self.create("persona", "Agent Persona", "")?;
        }

        Ok(())
    }

    pub fn get(&self, id: &str) -> Option<&MemoryBlock> {
        self.blocks.get(id)
    }

    pub fn append(&mut self, id: &str, content: &str) -> Result<()> {
        let block = self
            .blocks
            .get(id)
            .with_context(|| format!("memory block '{id}' not found"))?;

        let new_content = if block.content.is_empty() {
            content.to_string()
        } else {
            format!("{}\n{}", block.content, content)
        };

        if new_content.len() > block.max_chars {
            bail!(
                "content would exceed max chars ({}) for block '{id}'",
                block.max_chars
            );
        }

        let now = Utc::now().to_rfc3339();
        let db = self.storage.conn();
        db.execute(
            "UPDATE memory_blocks SET content = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![new_content, now, id],
        )
        .context("failed to update memory block")?;

        self.blocks.get_mut(id).unwrap().content = new_content;
        self.blocks.get_mut(id).unwrap().updated_at = now;

        Ok(())
    }

    pub fn replace(&mut self, id: &str, old_content: &str, new_content: &str) -> Result<()> {
        let block = self
            .blocks
            .get(id)
            .with_context(|| format!("memory block '{id}' not found"))?;

        let replaced = block.content.replacen(old_content, new_content, 1);

        if replaced.len() > block.max_chars {
            bail!(
                "replacement would exceed max chars ({}) for block '{id}'",
                block.max_chars
            );
        }

        let now = Utc::now().to_rfc3339();
        let db = self.storage.conn();
        db.execute(
            "UPDATE memory_blocks SET content = ?1, updated_at = ?2 WHERE id = ?3",
            rusqlite::params![replaced, now, id],
        )
        .context("failed to update memory block")?;

        self.blocks.get_mut(id).unwrap().content = replaced;
        self.blocks.get_mut(id).unwrap().updated_at = now;

        Ok(())
    }

    pub fn create(&mut self, id: &str, label: &str, content: &str) -> Result<()> {
        if self.blocks.contains_key(id) {
            bail!("memory block '{id}' already exists");
        }

        let now = Utc::now().to_rfc3339();
        let db = self.storage.conn();
        db.execute(
            "INSERT INTO memory_blocks (id, label, content, max_chars, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![id, label, content, self.default_max_chars as i64, now, now],
        )
        .context("failed to create memory block")?;

        self.blocks.insert(
            id.to_string(),
            MemoryBlock {
                id: id.to_string(),
                label: label.to_string(),
                content: content.to_string(),
                max_chars: self.default_max_chars,
                updated_at: now,
            },
        );

        Ok(())
    }

    pub fn list(&self) -> Vec<&MemoryBlock> {
        self.blocks.values().collect()
    }

    pub fn to_context_string(&self) -> String {
        let mut result = String::new();
        for block in self.blocks.values() {
            result.push_str(&format!("[{}]: {}\n", block.id, block.content));
        }
        result
    }

    /// Like `to_context_string` but skips empty blocks (for prompt injection).
    pub fn to_nonempty_context_string(&self) -> String {
        let mut result = String::new();
        for block in self.blocks.values() {
            if !block.content.trim().is_empty() {
                result.push_str(&format!("[{}]: {}\n", block.id, block.content));
            }
        }
        result
    }

    pub fn has(&self, id: &str) -> bool {
        self.blocks.contains_key(id)
    }
}
