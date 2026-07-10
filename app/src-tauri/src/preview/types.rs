//! Shared preview types and event envelopes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewMode {
    Static,
    Framework,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewStatus {
    Starting,
    Ready,
    Stopping,
    Stopped,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreviewPlacement {
    Hidden,
    Split,
    Popout,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewDescriptor {
    pub id: Uuid,
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub mode: PreviewMode,
    pub url: String,
    pub status: PreviewStatus,
    pub revision: u64,
    pub placement: PreviewPlacement,
    pub entrypoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewStartRequest {
    pub workspace_id: String,
    pub session_id: Option<String>,
    pub mode: PreviewMode,
    /// Relative entry path under workspace (e.g. `index.html`).
    pub entrypoint: Option<String>,
    /// Required for framework mode after user approval.
    pub approved_command: Option<FrameworkCommandRequest>,
    pub placement: Option<PreviewPlacement>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrameworkCommandRequest {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrameworkDetection {
    pub package_manager: Option<String>,
    pub dev_script: Option<String>,
    pub suggested_program: Option<String>,
    pub suggested_args: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewVisibilityRequest {
    pub preview_id: Uuid,
    pub placement: PreviewPlacement,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PreviewLogsRequest {
    pub preview_id: Uuid,
    pub cursor: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LogLine {
    pub cursor: u64,
    pub stream: String,
    pub line: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct LogPage {
    pub lines: Vec<LogLine>,
    pub next_cursor: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PreviewEvent {
    #[serde(rename = "status")]
    Status {
        v: u8,
        preview_id: Uuid,
        status: PreviewStatus,
    },
    #[serde(rename = "reload")]
    Reload {
        v: u8,
        preview_id: Uuid,
        revision: u64,
        paths: Vec<String>,
    },
    #[serde(rename = "log")]
    Log {
        v: u8,
        preview_id: Uuid,
        stream: String,
        line: String,
    },
    #[serde(rename = "error")]
    Error {
        v: u8,
        preview_id: Uuid,
        code: String,
        message: String,
    },
}

impl PreviewEvent {
    pub fn status(preview_id: Uuid, status: PreviewStatus) -> Self {
        Self::Status {
            v: 1,
            preview_id,
            status,
        }
    }

    pub fn reload(preview_id: Uuid, revision: u64, paths: Vec<String>) -> Self {
        Self::Reload {
            v: 1,
            preview_id,
            revision,
            paths,
        }
    }

    pub fn log(preview_id: Uuid, stream: &str, line: String) -> Self {
        Self::Log {
            v: 1,
            preview_id,
            stream: stream.to_string(),
            line,
        }
    }

    pub fn error(preview_id: Uuid, code: &str, message: String) -> Self {
        Self::Error {
            v: 1,
            preview_id,
            code: code.to_string(),
            message,
        }
    }
}

/// Resource quotas for preview sessions.
#[derive(Debug, Clone)]
pub struct PreviewQuotas {
    pub max_per_workspace: usize,
    pub max_global: usize,
    pub max_log_bytes: usize,
}

impl Default for PreviewQuotas {
    fn default() -> Self {
        Self {
            max_per_workspace: 3,
            max_global: 8,
            max_log_bytes: 512 * 1024,
        }
    }
}
