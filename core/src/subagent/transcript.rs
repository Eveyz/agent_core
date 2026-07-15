use crate::types::Message;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptOutcome {
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Aborted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranscriptDocument {
    pub runtime_id: String,
    pub outcome: TranscriptOutcome,
    pub messages: Vec<Message>,
}

/// Owns the durable recovery record for exactly one runtime subagent.
///
/// A checkpoint is cheap and in-memory. Terminal paths call `finalize`; if a
/// future is aborted or unwinds first, `Drop` synchronously materializes the
/// latest checkpoint with an `aborted` outcome.
pub struct TranscriptRecorder {
    runtime_id: String,
    path: PathBuf,
    messages: Vec<Message>,
    partial_assistant: Option<String>,
    finalized: bool,
}

impl TranscriptRecorder {
    pub fn new_in(root: &Path, runtime_id: &str) -> Result<Self> {
        let runtime_id = uuid::Uuid::parse_str(runtime_id)
            .context("transcript runtime id must be a UUID")?
            .to_string();
        Ok(Self {
            path: root.join(format!("{runtime_id}.transcript.json")),
            runtime_id,
            messages: Vec::new(),
            partial_assistant: None,
            finalized: false,
        })
    }

    pub fn in_default_root(runtime_id: &str) -> Option<Self> {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()?;
        Self::new_in(&PathBuf::from(home).join(".agverse").join("subagents"), runtime_id).ok()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn checkpoint(&mut self, messages: &[Message], partial_assistant: Option<&str>) {
        self.messages = messages.to_vec();
        self.partial_assistant = partial_assistant
            .filter(|text| !text.trim().is_empty())
            .map(ToOwned::to_owned);
    }

    pub fn finalize(
        &mut self,
        messages: &[Message],
        outcome: TranscriptOutcome,
    ) -> Result<PathBuf> {
        self.messages = messages.to_vec();
        if matches!(outcome, TranscriptOutcome::Succeeded) {
            self.partial_assistant = None;
        }
        self.persist(outcome)?;
        self.finalized = true;
        Ok(self.path.clone())
    }

    pub fn read(path: &Path) -> Result<TranscriptDocument> {
        let bytes = std::fs::read(path)
            .with_context(|| format!("read transcript {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse transcript {}", path.display()))
    }

    fn persist(&self, outcome: TranscriptOutcome) -> Result<()> {
        let mut messages = self.messages.clone();
        if let Some(partial) = &self.partial_assistant {
            messages.push(Message::assistant(partial));
        }
        let document = TranscriptDocument {
            runtime_id: self.runtime_id.clone(),
            outcome,
            messages,
        };
        let parent = self.path.parent().context("transcript path has no parent")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create transcript directory {}", parent.display()))?;
        let temporary = self.path.with_extension("json.tmp");
        let bytes = serde_json::to_vec_pretty(&document)?;
        std::fs::write(&temporary, bytes)
            .with_context(|| format!("write transcript checkpoint {}", temporary.display()))?;
        std::fs::rename(&temporary, &self.path)
            .with_context(|| format!("publish transcript {}", self.path.display()))?;
        Ok(())
    }
}

impl Drop for TranscriptRecorder {
    fn drop(&mut self) {
        if !self.finalized {
            if let Err(error) = self.persist(TranscriptOutcome::Aborted) {
                tracing::warn!(
                    runtime_id = %self.runtime_id,
                    error = %error,
                    "Failed to persist aborted subagent transcript"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TranscriptOutcome, TranscriptRecorder};
    use crate::types::Message;

    #[test]
    fn dropped_recorder_materializes_recoverable_partial_transcript() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
        let path = {
            let mut recorder = TranscriptRecorder::new_in(temp.path(), runtime_id)
                .expect("valid recorder");
            recorder.checkpoint(
                &[Message::user("inspect runtime")],
                Some("partial provider output"),
            );
            recorder.path().to_path_buf()
        };

        let persisted = TranscriptRecorder::read(&path).expect("persisted transcript");
        assert_eq!(persisted.outcome, TranscriptOutcome::Aborted);
        assert_eq!(persisted.messages.len(), 2);
        assert_eq!(
            persisted.messages[1].content.as_deref(),
            Some("partial provider output")
        );
    }
}
