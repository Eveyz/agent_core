use anyhow::{Context, Result};
use async_trait::async_trait;
use parking_lot::Mutex;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{Tool, ToolUpdateFn};
use crate::runtime::ProcessSupervisor;
use crate::skills::ScriptEntry;
use crate::types::EventSender;

pub fn is_skill_script_tool_name(name: &str) -> bool {
    name.starts_with("skill.")
        || (name.starts_with("skill_")
            && !matches!(
                name,
                "skill_list"
                    | "skill_search"
                    | "skill_load"
                    | "skill_deactivate"
                    | "skill_reload"
                    | "skill_list_resources"
                    | "skill_read_resource"
            ))
}

/// A tool that runs a script from an activated skill.
///
/// Registered as `skill.<skill_name>.<script_name>` in the per-Run
/// `ToolRegistry`. Uses [`ProcessSupervisor`] for process-group isolation
/// when attached.
pub struct SkillScriptTool {
    /// Full tool name: `skill.<skill_name>.<script_name>`.
    full_name: String,

    /// LLM-facing description.
    description: String,

    /// Absolute path to the skill's root directory (working dir for execution).
    skill_dir: PathBuf,

    /// Absolute path to the script file.
    script_path: PathBuf,

    /// Per-script timeout in seconds (default: 60).
    timeout_secs: u64,

    /// JSON Schema for parameters, as declared in SKILL.md `scripts[*].schema`.
    parameter_schema: Value,

    /// Optional supervisor — set during Run initialization.
    supervisor: Option<Arc<Mutex<ProcessSupervisor>>>,
}

impl SkillScriptTool {
    /// Create a new script tool without a supervisor (legacy/default path).
    pub fn new(skill_name: &str, script: &ScriptEntry, skill_dir: PathBuf) -> Self {
        let script_path = skill_dir.join(&script.file);
        let full_name = format!("skill.{}.{}", skill_name, script.name);
        Self {
            full_name,
            description: script.description.clone(),
            skill_dir,
            script_path,
            timeout_secs: script.timeout_secs,
            parameter_schema: script.schema.clone(),
            supervisor: None,
        }
    }

    /// Attach a ProcessSupervisor for process-group kill semantics.
    pub fn with_supervisor(mut self, sup: Arc<Mutex<ProcessSupervisor>>) -> Self {
        self.supervisor = Some(sup);
        self
    }

    /// Convert JSON Value args to CLI flags for the script.
    ///
    /// String → `--key "value"`, bool(true) → `--flag`, bool(false) → omitted,
    /// numbers → `--key 42`, nested objects → `--key '{"nested": true}'` (JSON string).
    fn args_to_cli(prefix: &str, args: &Value) -> Vec<String> {
        let mut flags: Vec<String> = Vec::new();
        if let Some(obj) = args.as_object() {
            for (key, val) in obj {
                // Runtime routing metadata is not part of the skill's public
                // schema and must never leak into strict script parsers.
                if key.starts_with('_') {
                    continue;
                }
                let flag_name = if prefix.is_empty() {
                    format!("--{key}")
                } else {
                    format!("{prefix}.{key}")
                };
                match val {
                    Value::Bool(true) => flags.push(flag_name),
                    Value::Bool(false) => {} // omit false booleans
                    Value::Null => {}        // omit nulls
                    Value::String(s) => {
                        flags.push(flag_name);
                        flags.push(s.clone());
                    }
                    Value::Number(n) => {
                        flags.push(flag_name);
                        flags.push(n.to_string());
                    }
                    Value::Array(arr) => {
                        for item in arr {
                            flags.push(flag_name.clone());
                            flags.push(item_to_arg_value(item));
                        }
                    }
                    Value::Object(_) => {
                        flags.push(flag_name);
                        flags.push(serde_json::to_string(val).unwrap_or_default());
                    }
                }
            }
        }
        flags
    }

    fn command_parts(script_path: &Path, cli_args: Vec<String>) -> Result<(String, Vec<String>)> {
        let script = script_path
            .to_str()
            .context("script path is not valid UTF-8")?
            .to_string();
        let extension = script_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        let (program, mut args) = match extension {
            "sh" => ("sh".to_string(), vec![script]),
            "bash" => ("bash".to_string(), vec![script]),
            "py" => (
                if cfg!(windows) { "python" } else { "python3" }.to_string(),
                vec![script],
            ),
            "js" => ("node".to_string(), vec![script]),
            "rb" => ("ruby".to_string(), vec![script]),
            "ts" => ("npx".to_string(), vec!["tsx".to_string(), script]),
            "go" => ("go".to_string(), vec!["run".to_string(), script]),
            "rs" => ("rust-script".to_string(), vec![script]),
            _ => (script, Vec::new()),
        };
        args.extend(cli_args);
        Ok((program, args))
    }

    /// Run the script directly without reparsing model-provided arguments in a shell.
    async fn run_sync(
        &self,
        program: &str,
        args: &[String],
        working_dir: &str,
        timeout_secs: u64,
    ) -> Result<String> {
        use tokio::io::AsyncReadExt;

        tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
            let mut child = tokio::process::Command::new(program)
                .args(args)
                .current_dir(working_dir)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("failed to spawn script process")?;

            let (stdout, stderr) = {
                let mut stdout_buf = Vec::new();
                let mut stderr_buf = Vec::new();
                let (_stdout, _stderr) = tokio::try_join!(
                    child.stdout.as_mut().unwrap().read_to_end(&mut stdout_buf),
                    child.stderr.as_mut().unwrap().read_to_end(&mut stderr_buf),
                )?;
                (
                    String::from_utf8_lossy(&stdout_buf).to_string(),
                    String::from_utf8_lossy(&stderr_buf).to_string(),
                )
            };

            let exit_code = child.wait().await?.code().unwrap_or(-1);
            let mut result = stdout;
            if !stderr.is_empty() {
                result.push_str("\n--- stderr ---\n");
                result.push_str(&stderr);
            }
            if exit_code != 0 {
                result.push_str(&format!("\n[exit code: {exit_code}]"));
            }
            Ok(result)
        })
        .await
        .context("script execution timed out")?
    }
}

/// Convert a JSON value to a CLI argument string (non-nested).
fn item_to_arg_value(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => serde_json::to_string(val).unwrap_or_default(),
    }
}

#[async_trait]
impl Tool for SkillScriptTool {
    fn name(&self) -> &str {
        &self.full_name
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn parameters_schema(&self) -> Value {
        self.parameter_schema.clone()
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_with_stream(args, None, None).await
    }

    async fn execute_with_stream(
        &self,
        args: Value,
        on_update: Option<ToolUpdateFn>,
        _event_sender: Option<EventSender>,
    ) -> Result<String> {
        let cli_args = Self::args_to_cli("", &args);
        let script_path = crate::skills::manifest::resolve_existing_path_within(
            &self.skill_dir,
            &self.script_path,
            "script file",
        )?;
        let (program, process_args) = Self::command_parts(&script_path, cli_args)?;
        let working_dir = self
            .skill_dir
            .to_str()
            .context("skill dir is not valid UTF-8")?;

        if let Some(ref sup) = self.supervisor {
            // ── Supervised path ──────────────────────────────────
            run_supervised(
                sup,
                &program,
                &process_args,
                working_dir,
                self.timeout_secs.min(600),
                on_update,
            )
            .await
        } else {
            // ── Legacy path ──────────────────────────────────────
            self.run_sync(
                &program,
                &process_args,
                working_dir,
                self.timeout_secs.min(600),
            )
            .await
        }
    }
}

/// Supervised script execution: spawn via ProcessSupervisor (process group),
/// stream stdout line-by-line, kill on cancel.
///
/// Follows the same pattern as ShellTool::run_shell_supervised.
async fn run_supervised(
    sup: &Arc<Mutex<ProcessSupervisor>>,
    program: &str,
    args: &[String],
    working_dir: &str,
    timeout_secs: u64,
    on_update: Option<ToolUpdateFn>,
) -> Result<String> {
    let child_id = {
        let mut supervisor = sup.lock();
        supervisor.spawn_process(program, args, Some(working_dir))?
    };

    let stdout_handle = {
        let mut supervisor = sup.lock();
        supervisor
            .get_child(&child_id)
            .and_then(|c| c.take_stdout())
    };

    let stderr_handle = {
        let mut supervisor = sup.lock();
        supervisor
            .get_child(&child_id)
            .and_then(|c| c.take_stderr())
    };

    // Read stdout — streaming line-by-line if on_update is provided.
    let stdout_fut = async {
        let mut result = String::new();
        if let Some(mut stdout) = stdout_handle {
            if let Some(ref update) = on_update {
                use tokio::io::AsyncBufReadExt;
                let reader = tokio::io::BufReader::new(stdout);
                let mut lines = reader.lines();
                loop {
                    match lines.next_line().await {
                        Ok(Some(line)) => {
                            update(&line);
                            result.push_str(&line);
                            result.push('\n');
                        }
                        Ok(None) => break,
                        Err(e) => {
                            tracing::warn!(error = %e, "stdout read error in script");
                            break;
                        }
                    }
                }
            } else {
                use tokio::io::AsyncReadExt;
                let mut buf = Vec::new();
                if let Err(e) = stdout.read_to_end(&mut buf).await {
                    tracing::warn!(error = %e, "stdout read error in script");
                }
                result = String::from_utf8_lossy(&buf).to_string();
            }
        }
        result
    };

    // Read stderr fully.
    let stderr_fut = async {
        let mut result = String::new();
        if let Some(mut stderr) = stderr_handle {
            use tokio::io::AsyncReadExt;
            let mut buf = Vec::new();
            if let Err(e) = stderr.read_to_end(&mut buf).await {
                tracing::warn!(error = %e, "stderr read error in script");
            }
            result = String::from_utf8_lossy(&buf).to_string();
        }
        result
    };

    // Poll exit code.
    let sup_clone = sup.clone();
    let child_id_clone = child_id.clone();
    let wait_fut = async move {
        loop {
            let maybe_code = {
                let mut supervisor = sup_clone.lock();
                supervisor
                    .get_child(&child_id_clone)
                    .and_then(|c| c.try_exit_code())
            };
            if let Some(code) = maybe_code {
                return code;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    };

    let outcome = tokio::time::timeout(std::time::Duration::from_secs(timeout_secs), async {
        tokio::join!(stdout_fut, stderr_fut, wait_fut)
    })
    .await;

    let (stdout_str, stderr_str, exit_code) = match outcome {
        Ok(values) => values,
        Err(_) => {
            let mut supervisor = sup.lock();
            let _ = supervisor.kill(&child_id);
            anyhow::bail!("script execution timed out after {timeout_secs}s");
        }
    };

    // Clean up child.
    {
        let mut supervisor = sup.lock();
        if let Err(e) = supervisor.kill(&child_id) {
            tracing::warn!(error = %e, "failed to kill script child during cleanup");
        }
    }

    let mut result = stdout_str;
    if !stderr_str.is_empty() {
        result.push_str("\n--- stderr ---\n");
        result.push_str(&stderr_str);
    }
    if exit_code != 0 {
        result.push_str(&format!("\n[exit code: {exit_code}]"));
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_args_to_cli_simple() {
        let args = json!({"env": "staging", "verbose": true, "count": 3});
        let flags = SkillScriptTool::args_to_cli("", &args);
        // Order is HashMap-dependent, so check presence rather than exact order.
        assert!(flags.iter().any(|f| f == "--env"));
        assert!(flags.iter().any(|f| f == "staging"));
        assert!(flags.iter().any(|f| f == "--verbose"));
        assert!(flags.iter().any(|f| f == "--count"));
        assert!(flags.iter().any(|f| f == "3"));
        // No flag for true boolean — it appears as just --verbose (no "true" following).
    }

    #[test]
    fn test_args_to_cli_false_omitted() {
        let args = json!({"dry_run": false, "name": "test"});
        let flags = SkillScriptTool::args_to_cli("", &args);
        assert!(!flags.iter().any(|f| f == "--dry_run"));
        assert!(flags.iter().any(|f| f == "--name"));
        assert!(flags.iter().any(|f| f == "test"));
    }

    #[test]
    fn test_args_to_cli_nested() {
        let args = json!({"config": {"host": "localhost", "port": 8080}});
        let flags = SkillScriptTool::args_to_cli("", &args);
        assert!(flags.iter().any(|f| f == "--config"));
        assert!(flags.iter().any(|f| f.contains("localhost")));
        assert!(flags.iter().any(|f| f.contains("8080")));
    }

    #[test]
    fn test_args_to_cli_array() {
        let args = json!({"tags": ["rust", "async", "tools"]});
        let flags = SkillScriptTool::args_to_cli("", &args);
        assert_eq!(flags.iter().filter(|f| *f == "--tags").count(), 3);
        assert!(flags.contains(&"rust".to_string()));
        assert!(flags.contains(&"async".to_string()));
        assert!(flags.contains(&"tools".to_string()));
    }

    #[test]
    fn runtime_metadata_is_not_forwarded_to_scripts() {
        let args = json!({
            "query": "rust",
            "_session_id": "session-1",
            "_working_dir": "/tmp/worktree"
        });
        assert_eq!(
            SkillScriptTool::args_to_cli("", &args),
            vec!["--query".to_string(), "rust".to_string()]
        );
    }

    #[test]
    fn test_tool_name_format() {
        let entry = ScriptEntry {
            name: "run_export".into(),
            description: "Export data".into(),
            file: "scripts/export.sh".into(),
            timeout_secs: 60,
            schema: json!({}),
        };
        let tool = SkillScriptTool::new("pptx", &entry, PathBuf::from("/tmp/test-skill"));
        assert_eq!(tool.name(), "skill.pptx.run_export");
        assert!(is_skill_script_tool_name(tool.name()));
        assert!(is_skill_script_tool_name("skill_pptx_run_export"));
        assert!(!is_skill_script_tool_name("skill_read_resource"));
    }

    #[test]
    fn test_with_supervisor() {
        let supervisor = Arc::new(Mutex::new(ProcessSupervisor::new()));
        let entry = ScriptEntry {
            name: "test".into(),
            description: "test".into(),
            file: "scripts/test.sh".into(),
            timeout_secs: 10,
            schema: json!({}),
        };
        let tool = SkillScriptTool::new("test-skill", &entry, PathBuf::from("/tmp"))
            .with_supervisor(supervisor);
        assert!(tool.supervisor.is_some());
        assert_eq!(tool.timeout_secs, 10);
    }

    #[test]
    fn command_parts_keep_shell_metacharacters_as_one_argument() {
        let (program, args) = SkillScriptTool::command_parts(
            Path::new("/tmp/my skill.py"),
            vec!["--value".into(), "hello; touch /tmp/never".into()],
        )
        .unwrap();
        assert_eq!(program, if cfg!(windows) { "python" } else { "python3" });
        assert_eq!(args[0], "/tmp/my skill.py");
        assert_eq!(args[2], "hello; touch /tmp/never");
    }

    #[tokio::test]
    async fn execute_does_not_reparse_argument_metacharacters_in_a_shell() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("scripts")).unwrap();
        std::fs::write(
            dir.path().join("scripts/echo.sh"),
            "#!/bin/sh\nprintf '%s' \"$2\"\n",
        )
        .unwrap();
        let entry = ScriptEntry {
            name: "echo".into(),
            description: "echo".into(),
            file: "scripts/echo.sh".into(),
            timeout_secs: 10,
            schema: json!({"type": "object", "properties": {"value": {"type": "string"}}}),
        };
        let tool = SkillScriptTool::new("safe", &entry, dir.path().to_path_buf());
        let output = tool
            .execute(json!({"value": "ok; touch injected"}))
            .await
            .unwrap();
        assert_eq!(output, "ok; touch injected");
        assert!(!dir.path().join("injected").exists());
    }
}
