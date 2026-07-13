//! ProcessSupervisor — owns all child processes spawned by a Run.
//!
//! ## Why this exists
//!
//! `kill_on_drop(true)` is a false safety net:
//! - `tokio::select!` cancellation may delay when the `Child` is actually dropped.
//! - Even when dropped, only the *direct* child is killed — grandchildren
//!   spawned via `sh -c "a | b"` become orphans.
//!
//! ProcessSupervisor fixes both by:
//! 1. Placing each child in its own **process group** (Unix: `setpgid(0, 0)`).
//! 2. Killing the entire group with `killpg(pgid, SIGKILL)`.
//! 3. Calling `wait()` to reap zombies.
//! 4. RAII: `Drop` kills everything, so a Run being dropped never leaks.
//!
//! ## Usage
//!
//! A Run owns its own `ProcessSupervisor`. When the Run is cancelled or
//! dropped, `kill_all()` runs automatically.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::io;
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout};
use uuid::Uuid;

/// Handle to a spawned child process, tracked by the supervisor.
#[derive(Debug)]
pub struct SupervisedChild {
    /// The tokio child handle. `None` after it has been taken/killed.
    child: Option<Child>,
    /// Process group ID. On Unix this equals the child PID (because we use
    /// `process_group(0)`). `None` on non-Unix or if we failed to get the PID.
    pgid: Option<i32>,
    /// Human-readable label for debugging / events (e.g. `"shell: cargo build"`).
    pub label: String,
    /// When the process was spawned.
    pub spawned_at: std::time::Instant,
}

impl SupervisedChild {
    /// The OS process ID, if the child is still alive.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(|c| c.id())
    }

    /// Take the stdin handle (for writing to the process).
    pub fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.as_mut().and_then(|c| c.stdin.take())
    }

    /// Take the stdout handle (for reading output).
    pub fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.as_mut().and_then(|c| c.stdout.take())
    }

    /// Take the stderr handle.
    pub fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.as_mut().and_then(|c| c.stderr.take())
    }

    /// Check if the process has exited (non-blocking).
    pub fn try_is_exited(&mut self) -> bool {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(_)) => true,
                Ok(None) => false,
                Err(_) => true,
            }
        } else {
            true
        }
    }

    /// Try to get the exit code if the process has exited (non-blocking).
    /// Returns `Some(code)` if exited, `None` if still running.
    /// This calls `try_wait` internally, which reaps the zombie.
    pub fn try_exit_code(&mut self) -> Option<i32> {
        if let Some(ref mut child) = self.child {
            match child.try_wait() {
                Ok(Some(status)) => Some(status.code().unwrap_or(-1)),
                Ok(None) => None,
                Err(_) => Some(-1),
            }
        } else {
            Some(-1)
        }
    }

    /// Wait for the process to exit and return its exit status.
    pub async fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        if let Some(ref mut child) = self.child {
            child.wait().await
        } else {
            Err(io::Error::new(io::ErrorKind::Other, "child already taken"))
        }
    }
}

/// Manages all child processes for a single Run.
///
/// Each Run owns one of these. When the Run is cancelled or dropped,
/// `kill_all()` ensures no child processes survive.
pub struct ProcessSupervisor {
    children: HashMap<String, SupervisedChild>,
}

impl ProcessSupervisor {
    pub fn new() -> Self {
        Self {
            children: HashMap::new(),
        }
    }

    /// Number of currently tracked (alive) child processes.
    pub fn active_count(&self) -> usize {
        self.children.len()
    }

    /// Spawn a platform shell command under supervision.
    ///
    /// Windows: PowerShell; Unix: `sh -c`. The command runs in its own process
    /// group (Unix) so that `kill` can terminate the entire process tree
    /// (including piped commands like `a | b`).
    pub fn spawn_shell(&mut self, command: &str, cwd: &str) -> Result<String> {
        let mut cmd = crate::runtime::platform_shell::shell_command(command);
        cmd.current_dir(cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false); // we manage lifecycle ourselves

        self.apply_process_group(&mut cmd);

        let child = cmd.spawn().with_context(|| {
            format!(
                "failed to spawn {} command: {command}",
                crate::runtime::platform_shell::shell_label()
            )
        })?;

        let pid = child.id();
        let pgid = self.derive_pgid(pid);

        let child_id = Uuid::new_v4().to_string();
        let label = format!("shell: {}", truncate_label(command, 80));

        tracing::debug!(child_id = %child_id, pid = ?pid, pgid = ?pgid, label = %label, "spawned supervised process");

        self.children.insert(
            child_id.clone(),
            SupervisedChild {
                child: Some(child),
                pgid,
                label,
                spawned_at: std::time::Instant::now(),
            },
        );

        Ok(child_id)
    }

    /// Backward-compatible alias for [`Self::spawn_shell`].
    #[deprecated(note = "use spawn_shell instead")]
    pub fn spawn_bash(&mut self, command: &str, cwd: &str) -> Result<String> {
        self.spawn_shell(command, cwd)
    }

    /// Spawn an arbitrary command (used by MCP stdio transport).
    pub fn spawn_process(
        &mut self,
        program: &str,
        args: &[String],
        cwd: Option<&str>,
    ) -> Result<String> {
        let mut cmd = tokio::process::Command::new(program);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(false);

        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }

        self.apply_process_group(&mut cmd);

        let child = cmd
            .spawn()
            .with_context(|| format!("failed to spawn process: {program} {args:?}"))?;

        let pid = child.id();
        let pgid = self.derive_pgid(pid);

        let child_id = Uuid::new_v4().to_string();
        let label = format!("{}: {}", program, args.join(" "));

        tracing::debug!(child_id = %child_id, pid = ?pid, pgid = ?pgid, label = %label, "spawned supervised process");

        self.children.insert(
            child_id.clone(),
            SupervisedChild {
                child: Some(child),
                pgid,
                label,
                spawned_at: std::time::Instant::now(),
            },
        );

        Ok(child_id)
    }

    /// Get a mutable reference to a tracked child (to take stdin/stdout).
    pub fn get_child(&mut self, child_id: &str) -> Option<&mut SupervisedChild> {
        self.children.get_mut(child_id)
    }

    /// Kill a specific child process (and its process group) and remove it.
    pub fn kill(&mut self, child_id: &str) -> Result<()> {
        let mut sc = match self.children.remove(child_id) {
            Some(c) => c,
            None => return Ok(()), // already gone
        };
        self.kill_child(&mut sc, "explicit kill")?;
        Ok(())
    }

    /// Kill all tracked child processes. Called on cancel / drop.
    pub fn kill_all(&mut self) {
        if self.children.is_empty() {
            return;
        }
        let count = self.children.len();
        tracing::info!(count, "killing all supervised processes");

        let ids: Vec<String> = self.children.keys().cloned().collect();
        for id in ids {
            if let Some(mut sc) = self.children.remove(&id) {
                if let Err(e) = self.kill_child(&mut sc, "kill_all") {
                    tracing::warn!(child_id = %id, error = %e, "failed to kill child");
                }
            }
        }
    }

    /// Kill a single child: SIGTERM the process group, then SIGKILL + reap.
    fn kill_child(&self, sc: &mut SupervisedChild, reason: &str) -> Result<()> {
        // 1. Kill the process group (SIGKILL — we're tearing down).
        #[cfg(unix)]
        if let Some(pgid) = sc.pgid {
            tracing::debug!(pgid, reason, "sending SIGKILL to process group");
            // SAFETY: killpg with SIGKILL on a valid pgid is safe.
            unsafe {
                libc::killpg(pgid, libc::SIGKILL);
            }
        }

        // 2. Also kill the direct child (belt-and-suspenders, and for non-Unix).
        if let Some(ref mut child) = sc.child {
            // start_kill is non-async, safe to call in sync context (incl. Drop)
            let _ = child.start_kill();
        }

        // 3. Reap: we can't await in Drop, but we can try_wait to avoid zombies
        //    when called from non-Drop contexts. In Drop, the OS will reap
        //    when the parent process exits, or tokio's reaper handles it.
        if let Some(ref mut child) = sc.child {
            let _ = child.try_wait();
        }

        Ok(())
    }

    /// On Unix, place the child in a new process group so we can kill the
    /// entire tree later. `process_group(0)` calls `setpgid(0, 0)` in the
    /// child, making it a process group leader (pgid == pid).
    #[cfg(unix)]
    fn apply_process_group(&self, cmd: &mut tokio::process::Command) {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }

    #[cfg(not(unix))]
    fn apply_process_group(&self, _cmd: &mut tokio::process::Command) {
        // Windows: process groups are handled via Job Objects (future work).
    }

    /// Derive the process group ID from the PID.
    /// With `process_group(0)`, the child becomes its own group leader,
    /// so pgid == pid.
    #[cfg(unix)]
    fn derive_pgid(&self, pid: Option<u32>) -> Option<i32> {
        pid.map(|p| p as i32)
    }

    #[cfg(not(unix))]
    fn derive_pgid(&self, _pid: Option<u32>) -> Option<i32> {
        None
    }
}

impl Default for ProcessSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for ProcessSupervisor {
    fn drop(&mut self) {
        // RAII safety net: even if cancel_and_cleanup wasn't called,
        // dropping the supervisor kills everything.
        if !self.children.is_empty() {
            self.kill_all();
        }
    }
}

fn truncate_label(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        let end = s.floor_char_boundary(max);
        &s[..end]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    const LONG_SLEEP: &str = "Start-Sleep -Seconds 100";
    #[cfg(not(windows))]
    const LONG_SLEEP: &str = "sleep 100";

    #[tokio::test]
    async fn spawn_and_kill_shell() {
        let mut sup = ProcessSupervisor::new();
        // Long sleep would hang if not killed — we kill it immediately.
        let id = sup.spawn_shell(LONG_SLEEP, ".").unwrap();
        assert_eq!(sup.active_count(), 1);
        sup.kill(&id).unwrap();
        assert_eq!(sup.active_count(), 0);
    }

    #[tokio::test]
    async fn kill_all_clears_everything() {
        let mut sup = ProcessSupervisor::new();
        let _id1 = sup.spawn_shell(LONG_SLEEP, ".").unwrap();
        let _id2 = sup.spawn_shell(LONG_SLEEP, ".").unwrap();
        assert_eq!(sup.active_count(), 2);
        sup.kill_all();
        assert_eq!(sup.active_count(), 0);
    }

    #[tokio::test]
    async fn drop_kills_all_processes() {
        let mut sup = ProcessSupervisor::new();
        let _id = sup.spawn_shell(LONG_SLEEP, ".").unwrap();
        assert_eq!(sup.active_count(), 1);
        // Drop should kill the process without hanging.
        drop(sup);
    }

    #[tokio::test]
    async fn quick_command_completes() {
        let mut sup = ProcessSupervisor::new();
        let id = sup.spawn_shell("echo hello", ".").unwrap();
        let child = sup.get_child(&id).unwrap();
        // Take stdout and read it
        let _stdout = child.take_stdout();
        // Let it finish
        let _ = child.wait().await;
    }

    #[test]
    fn kill_nonexistent_is_ok() {
        let mut sup = ProcessSupervisor::new();
        // Killing a non-existent id should be a no-op, not an error.
        sup.kill("does-not-exist").unwrap();
    }

    #[test]
    fn truncate_label_works() {
        assert_eq!(truncate_label("short", 10), "short");
        assert_eq!(truncate_label("a very long command line", 10), "a very lon");
    }
}
