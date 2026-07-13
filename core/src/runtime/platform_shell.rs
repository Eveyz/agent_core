//! Platform-aware shell command spawning.
//!
//! - Windows: `powershell.exe -NoProfile -NonInteractive -Command <cmd>`
//! - Unix: `sh -c <cmd>`

use tokio::process::Command;

/// Whether `name` refers to the shell execution tool (current or legacy).
pub fn is_shell_tool(name: &str) -> bool {
    matches!(name, "shell" | "bash")
}

/// Program used as the shell interpreter on this platform.
pub fn shell_program() -> &'static str {
    #[cfg(windows)]
    {
        "powershell.exe"
    }
    #[cfg(not(windows))]
    {
        "sh"
    }
}

/// Short label for UI / logs (e.g. `"PowerShell"`, `"sh"`).
pub fn shell_label() -> &'static str {
    #[cfg(windows)]
    {
        "PowerShell"
    }
    #[cfg(not(windows))]
    {
        "sh"
    }
}

/// Build a [`Command`] that runs `command` in the platform shell.
///
/// Does not set cwd, stdio, or kill flags — callers configure those.
pub fn shell_command(command: &str) -> Command {
    let mut cmd = Command::new(shell_program());
    #[cfg(windows)]
    {
        cmd.arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(command);
    }
    #[cfg(not(windows))]
    {
        cmd.arg("-c").arg(command);
    }
    cmd
}
