use crate::types::Message;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
    pub scope: TranscriptScope,
    pub outcome: TranscriptOutcome,
    pub messages: Vec<Message>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TranscriptScope {
    pub session_id: Option<String>,
    pub parent_run_id: Option<String>,
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
    persisted: bool,
    scope: TranscriptScope,
}

impl TranscriptRecorder {
    pub fn new_in(root: &Path, runtime_id: &str) -> Result<Self> {
        let path = Self::path_in(root, runtime_id)?;
        let runtime_id = uuid::Uuid::parse_str(runtime_id)
            .context("transcript runtime id must be a UUID")?
            .to_string();
        Ok(Self {
            path,
            runtime_id,
            messages: Vec::new(),
            partial_assistant: None,
            finalized: false,
            persisted: false,
            scope: TranscriptScope::default(),
        })
    }

    pub fn path_in(root: &Path, runtime_id: &str) -> Result<PathBuf> {
        let runtime_id = uuid::Uuid::parse_str(runtime_id)
            .context("transcript runtime id must be a UUID")?
            .to_string();
        Ok(root.join(format!("{runtime_id}.transcript.json")))
    }

    pub fn in_default_root(runtime_id: &str) -> Option<Self> {
        let path = Self::default_path(runtime_id).ok()?;
        Self::new_in(path.parent()?, runtime_id).ok()
    }

    pub fn default_path(runtime_id: &str) -> Result<PathBuf> {
        let runtime_id = uuid::Uuid::parse_str(runtime_id)
            .context("transcript runtime id must be a UUID")?
            .to_string();
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .context("home directory unavailable")?;
        Self::path_in(
            &PathBuf::from(home).join(".agverse").join("subagents"),
            &runtime_id,
        )
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn set_scope(&mut self, session_id: Option<String>, parent_run_id: Option<String>) {
        self.scope = TranscriptScope {
            session_id,
            parent_run_id,
        };
    }

    pub fn persisted_path(&self) -> Option<&Path> {
        self.persisted.then_some(self.path.as_path())
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
        self.persisted = true;
        Ok(self.path.clone())
    }

    pub fn read(path: &Path) -> Result<TranscriptDocument> {
        let bytes =
            std::fs::read(path).with_context(|| format!("read transcript {}", path.display()))?;
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parse transcript {}", path.display()))
    }

    pub fn read_in(
        root: &Path,
        runtime_id: &str,
        expected_scope: &TranscriptScope,
    ) -> Result<TranscriptDocument> {
        let canonical_root = std::fs::canonicalize(root)
            .with_context(|| format!("canonicalize transcript root {}", root.display()))?;
        let target = Self::path_in(root, runtime_id)?;
        let canonical_target = std::fs::canonicalize(&target)
            .with_context(|| format!("canonicalize transcript {}", target.display()))?;
        if !canonical_target.starts_with(&canonical_root) {
            anyhow::bail!("transcript path escapes its configured root");
        }
        let document = Self::read(&canonical_target)?;
        if &document.scope != expected_scope {
            anyhow::bail!("transcript does not belong to the requesting run/session");
        }
        Ok(document)
    }

    fn persist(&self, outcome: TranscriptOutcome) -> Result<()> {
        let mut messages = self.messages.clone();
        if let Some(partial) = &self.partial_assistant {
            messages.push(Message::assistant(partial));
        }
        let document = TranscriptDocument {
            runtime_id: self.runtime_id.clone(),
            scope: self.scope.clone(),
            outcome,
            messages,
        };
        let parent = self
            .path
            .parent()
            .context("transcript path has no parent")?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("create transcript directory {}", parent.display()))?;
        #[cfg(unix)]
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
            .with_context(|| format!("secure transcript directory {}", parent.display()))?;
        let temporary = parent.join(format!(".{}.{}.tmp", self.runtime_id, uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec_pretty(&document)?;
        let publish = (|| -> Result<()> {
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            options.mode(0o600);
            let mut file = options
                .open(&temporary)
                .with_context(|| format!("create transcript checkpoint {}", temporary.display()))?;
            file.write_all(&bytes)
                .with_context(|| format!("write transcript checkpoint {}", temporary.display()))?;
            file.sync_all()
                .with_context(|| format!("sync transcript checkpoint {}", temporary.display()))?;
            std::fs::rename(&temporary, &self.path)
                .with_context(|| format!("publish transcript {}", self.path.display()))?;
            Ok(())
        })();
        if publish.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        publish?;
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
    use super::{TranscriptOutcome, TranscriptRecorder, TranscriptScope};
    use crate::types::Message;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn dropped_recorder_materializes_recoverable_partial_transcript() {
        let temp = tempfile::tempdir().expect("tempdir");
        let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
        let path = {
            let mut recorder =
                TranscriptRecorder::new_in(temp.path(), runtime_id).expect("valid recorder");
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

    #[cfg(unix)]
    #[test]
    fn scoped_reader_rejects_a_transcript_symlink_outside_its_root() {
        use std::os::unix::fs::symlink;

        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("transcripts");
        std::fs::create_dir(&root).unwrap();
        let runtime_id = "550e8400-e29b-41d4-a716-446655440000";
        let outside = temp.path().join("outside.json");
        std::fs::write(&outside, "{}").unwrap();
        symlink(
            &outside,
            TranscriptRecorder::path_in(&root, runtime_id).unwrap(),
        )
        .unwrap();

        assert!(
            TranscriptRecorder::read_in(&root, runtime_id, &TranscriptScope::default()).is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn persisted_transcripts_are_private_to_the_current_user() {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path().join("transcripts");
        let runtime_id = "550e8400-e29b-41d4-a716-446655440001";
        let mut recorder = TranscriptRecorder::new_in(&root, runtime_id).unwrap();
        let path = recorder
            .finalize(&[Message::user("secret")], TranscriptOutcome::Succeeded)
            .unwrap();
        assert_eq!(
            std::fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
