//! Durable agent metadata store (sidecar next to SessionManager).
//!
//! Gateway **agent id** == Agverse **session id**. Extra fields Cursor exposes
//! (repos, workspace kind, run index) live here as JSON under
//! `~/.agverse/gateway/agents/<id>.json`.

use crate::models::{AgentDetail, AgentSummary, RepoInput, RunView, WorkspaceView};
use anyhow::{Context, Result};
use chrono::Utc;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentRecord {
    pub id: String,
    pub name: String,
    /// `ACTIVE` | `ARCHIVED`
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_run_id: Option<String>,
    pub workspace: WorkspaceRecord,
    #[serde(default)]
    pub repos: Vec<RepoInput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    /// Run ids newest-first.
    #[serde(default)]
    pub run_ids: Vec<String>,
    /// Per-run metadata for list/get after the process restarts.
    #[serde(default)]
    pub runs: HashMap<String, RunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRecord {
    /// `host` | `git`
    #[serde(rename = "type")]
    pub workspace_type: String,
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunRecord {
    pub id: String,
    pub agent_id: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_id: Option<String>,
}

impl AgentRecord {
    pub fn to_summary(&self) -> AgentSummary {
        AgentSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            latest_run_id: self.latest_run_id.clone(),
        }
    }

    pub fn to_detail(&self) -> AgentDetail {
        AgentDetail {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            latest_run_id: self.latest_run_id.clone(),
            workspace: WorkspaceView {
                workspace_type: self.workspace.workspace_type.clone(),
                path: self.workspace.path.clone(),
            },
            repos: self.repos.clone(),
            mode: self.mode.clone(),
        }
    }
}

impl RunRecord {
    pub fn to_view(&self) -> RunView {
        RunView {
            id: self.id.clone(),
            agent_id: self.agent_id.clone(),
            status: self.status.clone(),
            created_at: self.created_at.clone(),
            updated_at: self.updated_at.clone(),
            duration_ms: self.duration_ms,
            result: self.result.clone(),
            prompt_id: self.prompt_id.clone(),
        }
    }
}

pub struct AgentStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl AgentStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root).with_context(|| format!("mkdir {}", root.display()))?;
        Ok(Self {
            root,
            lock: Mutex::new(()),
        })
    }

    fn path_for(&self, id: &str) -> PathBuf {
        self.root.join(format!("{id}.json"))
    }

    pub fn get(&self, id: &str) -> Result<Option<AgentRecord>> {
        let _g = self.lock.lock();
        self.get_unlocked(id)
    }

    fn get_unlocked(&self, id: &str) -> Result<Option<AgentRecord>> {
        let path = self.path_for(id);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)?;
        Ok(Some(serde_json::from_str(&raw)?))
    }

    pub fn save(&self, record: &AgentRecord) -> Result<()> {
        let _g = self.lock.lock();
        self.save_unlocked(record)
    }

    fn save_unlocked(&self, record: &AgentRecord) -> Result<()> {
        let path = self.path_for(&record.id);
        let tmp = path.with_extension("json.tmp");
        let raw = serde_json::to_string_pretty(record)?;
        fs::write(&tmp, raw)?;
        fs::rename(&tmp, &path)?;
        Ok(())
    }

    pub fn delete(&self, id: &str) -> Result<bool> {
        let _g = self.lock.lock();
        let path = self.path_for(id);
        if path.exists() {
            fs::remove_file(&path)?;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub fn list(&self, include_archived: bool) -> Result<Vec<AgentRecord>> {
        let _g = self.lock.lock();
        let mut items = Vec::new();
        if !self.root.exists() {
            return Ok(items);
        }
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let raw = fs::read_to_string(&path)?;
            let rec: AgentRecord = serde_json::from_str(&raw)?;
            if include_archived || rec.status != "ARCHIVED" {
                items.push(rec);
            }
        }
        items.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        Ok(items)
    }

    pub fn update<F>(&self, id: &str, f: F) -> Result<AgentRecord>
    where
        F: FnOnce(&mut AgentRecord),
    {
        let _g = self.lock.lock();
        let mut rec = self
            .get_unlocked(id)?
            .with_context(|| format!("agent {id} not found"))?;
        f(&mut rec);
        rec.updated_at = Utc::now().to_rfc3339();
        self.save_unlocked(&rec)?;
        Ok(rec)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

pub fn gateway_dir() -> PathBuf {
    agent_core::paths::get_agverse_dir().join("gateway")
}

pub fn agents_dir() -> PathBuf {
    gateway_dir().join("agents")
}

pub fn workspaces_dir() -> PathBuf {
    gateway_dir().join("workspaces")
}
