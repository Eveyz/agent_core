//! Supervised framework process lifecycle for preview mode.

use std::collections::VecDeque;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::watch;
use uuid::Uuid;

use super::path_policy::redact_log_line;
use super::types::{FrameworkDetection, LogLine, LogPage};

const LOG_RING_CAPACITY: usize = 2000;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(60);
const GRACE_PERIOD: Duration = Duration::from_secs(3);

pub struct LogRing {
    lines: VecDeque<LogLine>,
    cursor: u64,
    bytes: usize,
    max_bytes: usize,
}

impl LogRing {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            lines: VecDeque::new(),
            cursor: 0,
            bytes: 0,
            max_bytes,
        }
    }

    pub fn push(&mut self, stream: &str, line: String) {
        let redacted = redact_log_line(&line);
        let size = redacted.len();
        self.cursor += 1;
        self.lines.push_back(LogLine {
            cursor: self.cursor,
            stream: stream.to_string(),
            line: redacted,
        });
        self.bytes += size;
        while self.bytes > self.max_bytes && !self.lines.is_empty() {
            if let Some(removed) = self.lines.pop_front() {
                self.bytes = self.bytes.saturating_sub(removed.line.len());
            }
        }
        while self.lines.len() > LOG_RING_CAPACITY {
            if let Some(removed) = self.lines.pop_front() {
                self.bytes = self.bytes.saturating_sub(removed.line.len());
            }
        }
    }

    pub fn page(&self, after: u64) -> LogPage {
        let lines: Vec<LogLine> = self
            .lines
            .iter()
            .filter(|l| l.cursor > after)
            .cloned()
            .collect();
        LogPage {
            next_cursor: self.cursor,
            lines,
        }
    }
}

pub struct FrameworkProcess {
    pub child_id: Uuid,
    pub program: String,
    pub args: Vec<String>,
    pub logs: Arc<Mutex<LogRing>>,
    ready_tx: watch::Sender<bool>,
    ready_rx: watch::Receiver<bool>,
    child: Arc<Mutex<Option<tokio::process::Child>>>,
    pgid: Option<i32>,
    #[cfg(windows)]
    _job: Option<windows_job::Job>,
}

impl FrameworkProcess {
    pub async fn spawn(
        cwd: &Path,
        program: &str,
        args: &[String],
        env_port: u16,
        max_log_bytes: usize,
    ) -> anyhow::Result<Self> {
        let child_id = Uuid::new_v4();
        let logs = Arc::new(Mutex::new(LogRing::new(max_log_bytes)));
        let (ready_tx, ready_rx) = watch::channel(false);

        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PORT", env_port.to_string())
            .env("HOST", "127.0.0.1")
            .env_remove("NODE_OPTIONS");

        apply_process_group(&mut cmd);

        #[cfg(windows)]
        let job = windows_job::Job::new()?;

        let child = cmd.spawn()?;
        let pid = child.id();
        let pgid = derive_pgid(pid);

        #[cfg(windows)]
        if let Some(pid) = pid {
            job.assign_pid(pid)?;
        }

        let child_arc = Arc::new(Mutex::new(Some(child)));
        let logs_stdout = logs.clone();
        let logs_stderr = logs.clone();
        let ready_tx_stdout = ready_tx.clone();

        if let Some(stdout) = child_arc.lock().as_mut().and_then(|c| c.stdout.take()) {
            tauri::async_runtime::spawn(async move {
                let reader = BufReader::new(stdout);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logs_stdout.lock().push("stdout", line.clone());
                    if line.contains("Local:") || line.contains("ready") || line.contains("listening") {
                        let _ = ready_tx_stdout.send(true);
                    }
                }
            });
        }

        if let Some(stderr) = child_arc.lock().as_mut().and_then(|c| c.stderr.take()) {
            tauri::async_runtime::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    logs_stderr.lock().push("stderr", line);
                }
            });
        }

        Ok(Self {
            child_id,
            program: program.to_string(),
            args: args.to_vec(),
            logs,
            ready_tx,
            ready_rx,
            child: child_arc,
            pgid,
            #[cfg(windows)]
            _job: Some(job),
        })
    }

    pub async fn wait_ready(&self, health_url: &str) -> anyhow::Result<()> {
        let start = std::time::Instant::now();
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()?;

        while start.elapsed() < STARTUP_TIMEOUT {
            if *self.ready_rx.borrow() {
                return Ok(());
            }
            if let Ok(resp) = client.get(health_url).send().await {
                if resp.status().is_success() || resp.status().as_u16() == 404 {
                    let _ = self.ready_tx.send(true);
                    return Ok(());
                }
            }
            if self.try_is_exited() {
                anyhow::bail!("framework process exited before becoming ready");
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        anyhow::bail!("framework process startup timed out")
    }

    fn try_is_exited(&self) -> bool {
        if let Some(ref mut child) = *self.child.lock() {
            matches!(child.try_wait(), Ok(Some(_)) | Err(_))
        } else {
            true
        }
    }

    pub async fn kill(&self) {
        #[cfg(unix)]
        if let Some(pgid) = self.pgid {
            unsafe {
                libc::killpg(pgid, libc::SIGTERM);
            }
            tokio::time::sleep(GRACE_PERIOD).await;
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }

        if let Some(ref mut child) = *self.child.lock() {
            let _ = child.start_kill();
            let _ = child.try_wait();
        }
    }
}

#[cfg(unix)]
fn apply_process_group(cmd: &mut Command) {
    use std::os::unix::process::CommandExt;
    cmd.process_group(0);
}

#[cfg(not(unix))]
fn apply_process_group(_cmd: &mut Command) {}

#[cfg(unix)]
fn derive_pgid(pid: Option<u32>) -> Option<i32> {
    pid.map(|p| p as i32)
}

#[cfg(not(unix))]
fn derive_pgid(_pid: Option<u32>) -> Option<i32> {
    None
}

/// Detect package manager and dev script from workspace.
pub fn detect_framework(workspace: &Path) -> FrameworkDetection {
    let pm = if workspace.join("pnpm-lock.yaml").exists() {
        Some("pnpm".into())
    } else if workspace.join("yarn.lock").exists() {
        Some("yarn".into())
    } else if workspace.join("bun.lockb").exists() || workspace.join("bun.lock").exists() {
        Some("bun".into())
    } else if workspace.join("package-lock.json").exists() {
        Some("npm".into())
    } else {
        None
    };

    let package_json = workspace.join("package.json");
    let dev_script = package_json
        .exists()
        .then(|| std::fs::read_to_string(&package_json).ok())
        .flatten()
        .and_then(|content| {
            let v: serde_json::Value = serde_json::from_str(&content).ok()?;
            v.get("scripts")
                .and_then(|s| s.get("dev"))
                .and_then(|d| d.as_str())
                .map(|s| s.to_string())
        });

    let (suggested_program, suggested_args) = match pm.as_deref() {
        Some("pnpm") => (
            Some("pnpm".into()),
            vec!["run".into(), "dev".into()],
        ),
        Some("yarn") => (
            Some("yarn".into()),
            vec!["run".into(), "dev".into()],
        ),
        Some("bun") => (
            Some("bun".into()),
            vec!["run".into(), "dev".into()],
        ),
        Some("npm") => (
            Some("npm".into()),
            vec!["run".into(), "dev".into()],
        ),
        _ => (None, vec![]),
    };

    FrameworkDetection {
        package_manager: pm,
        dev_script,
        suggested_program,
        suggested_args,
    }
}

#[cfg(windows)]
mod windows_job {
    use anyhow::Result;

    pub struct Job {
        handle: isize,
    }

    impl Job {
        pub fn new() -> Result<Self> {
            // Minimal stub: Windows Job Object assignment is platform-specific.
            // Production builds should use winapi/windows-sys for JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE.
            Ok(Self { handle: 0 })
        }

        pub fn assign_pid(&self, _pid: u32) -> Result<()> {
            Ok(())
        }
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Job handle cleanup when real implementation is added.
        }
    }
}
