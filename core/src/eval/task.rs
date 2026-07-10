//! Eval task / suite loading.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

use super::grader::GraderSpec;
use super::mock_llm::MockScript;

#[derive(Debug, Clone, Deserialize)]
pub struct SuiteManifest {
    pub name: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskDriver {
    /// Full Brain + mock/live LLM run.
    #[default]
    Run,
    /// Offline: load a JSONL trace and grade collector output only.
    Trace,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalPolicy {
    #[default]
    AutoAllow,
    AutoDeny,
    Yolo,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct TaskActions {
    /// Cancel the run after seeing this event tag (e.g. "run_started", "tool_started").
    #[serde(default)]
    pub cancel_after_event: Option<String>,
    /// Queue a steer message after seeing this event tag.
    #[serde(default)]
    pub steer_after_event: Option<String>,
    #[serde(default)]
    pub steer_message: Option<String>,
    /// Delay ms before cancel/steer (give tools time to start).
    #[serde(default)]
    pub action_delay_ms: u64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TaskManifest {
    pub id: String,
    #[serde(default)]
    pub bucket: Option<String>,
    #[serde(default)]
    pub prompt: String,
    #[serde(default)]
    pub driver: TaskDriver,
    #[serde(default)]
    pub approval: ApprovalPolicy,
    #[serde(default)]
    pub max_iterations: Option<u32>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    #[serde(default)]
    pub grader: GraderSpec,
    #[serde(default)]
    pub actions: TaskActions,
    /// Relative path to mock script.toml (mock mode).
    #[serde(default)]
    pub script: Option<String>,
    /// Relative path to trace jsonl (trace driver).
    #[serde(default)]
    pub trace: Option<String>,
    /// Taxonomy self-check: task is expected to surface harness fail tags.
    /// Excluded from suite harness_fail_rate / CI gate.
    #[serde(default)]
    pub expect_harness_fail: bool,
}

#[derive(Debug, Clone)]
pub struct EvalTask {
    pub dir: PathBuf,
    pub manifest: TaskManifest,
    pub script: Option<MockScript>,
}

#[derive(Debug, Clone)]
pub struct EvalSuite {
    pub root: PathBuf,
    pub manifest: SuiteManifest,
    pub tasks: Vec<EvalTask>,
}

pub fn load_suite(suite_dir: &Path) -> Result<EvalSuite> {
    let suite_toml = suite_dir.join("suite.toml");
    let text = std::fs::read_to_string(&suite_toml)
        .with_context(|| format!("read {}", suite_toml.display()))?;
    let manifest: SuiteManifest = toml::from_str(&text)?;

    let tasks_dir = suite_dir.join("tasks");
    let mut tasks = Vec::new();
    if tasks_dir.is_dir() {
        let mut entries: Vec<_> = std::fs::read_dir(&tasks_dir)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let task = load_task(&entry.path())?;
            tasks.push(task);
        }
    }

    Ok(EvalSuite {
        root: suite_dir.to_path_buf(),
        manifest,
        tasks,
    })
}

pub fn load_task(task_dir: &Path) -> Result<EvalTask> {
    let task_toml = task_dir.join("task.toml");
    let text = std::fs::read_to_string(&task_toml)
        .with_context(|| format!("read {}", task_toml.display()))?;
    let mut manifest: TaskManifest = toml::from_str(&text)?;

    // Optional prompt.md override
    let prompt_md = task_dir.join("prompt.md");
    if prompt_md.exists() {
        manifest.prompt = std::fs::read_to_string(prompt_md)?;
    }

    let script = if let Some(rel) = &manifest.script {
        Some(MockScript::load(&task_dir.join(rel))?)
    } else {
        let default = task_dir.join("script.toml");
        if default.exists() {
            Some(MockScript::load(&default)?)
        } else {
            None
        }
    };

    Ok(EvalTask {
        dir: task_dir.to_path_buf(),
        manifest,
        script,
    })
}

/// Copy `workspace/` under the task into a temp directory; return its path.
pub fn materialize_workspace(task: &EvalTask) -> Result<PathBuf> {
    let tmp = tempfile::tempdir()?.keep();
    let src = task.dir.join("workspace");
    if src.is_dir() {
        copy_dir_recursive(&src, &tmp)?;
    }
    Ok(tmp)
}

fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let to = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir_recursive(&entry.path(), &to)?;
        } else {
            std::fs::copy(entry.path(), &to)?;
        }
    }
    Ok(())
}
