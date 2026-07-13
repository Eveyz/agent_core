// ── Destructive command detection ───────────────────────────────────

/// Expand `~` in a path to the user's home directory.
fn expand_tilde(path: &str) -> String {
    crate::util::expand_tilde(path)
}

/// Canonicalize a path for sandbox comparison.
///
/// Absolute existing paths are canonicalized directly. For paths that do not
/// exist yet (e.g. a file `write_file` is about to create), the existing
/// parent directory is canonicalized and the file name re-attached. Relative
/// paths are resolved against the current working directory first.
fn canonicalize_target(file_path: &str) -> PathBuf {
    let p = Path::new(file_path);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(p)
    };
    if let Ok(canon) = abs.canonicalize() {
        return canon;
    }
    // File does not exist yet — canonicalize the parent and re-attach the name.
    if let Some(parent) = abs.parent() {
        if let Ok(parent_canon) = parent.canonicalize() {
            if let Some(name) = abs.file_name() {
                return parent_canon.join(name);
            }
        }
    }
    abs
}

/// Normalize a command for destructive-pattern detection: collapse all
/// Unicode whitespace (including tabs, newlines, and non-breaking spaces) to a
/// single ASCII space, and expand the `${IFS}` / `$IFS` shell-variable trick
/// used to split tokens without a literal space (e.g. `rm${IFS}-rf`).
fn normalize_command(cmd: &str) -> String {
    let mut s: String = cmd
        .chars()
        .map(|c| {
            if c.is_whitespace() || c == '\u{00a0}' {
                ' '
            } else {
                c
            }
        })
        .collect();
    s = s.replace("${IFS}", " ");
    s = s.replace("$IFS", " ");
    s
}

/// Programs that merely wrap another command (e.g. `env rm -rf /`,
/// `nohup rm`, `xargs rm`). The real target program follows them, possibly
/// after `VAR=value` assignments.
const COMMAND_WRAPPERS: &[&str] = &[
    "env", "exec", "command", "nohup", "time", "nice", "ionice", "xargs",
];

/// Return the index of the effective program in a sub-command's token list,
/// skipping wrapper prefixes and `VAR=value` env assignments.
fn effective_program_index(tokens: &[&str]) -> Option<usize> {
    let mut i = 0;
    while i < tokens.len() {
        let t = tokens[i];
        if COMMAND_WRAPPERS.contains(&t) {
            i += 1;
            continue;
        }
        // `VAR=value` assignment (env-style), but not flags like `-i`.
        if !t.starts_with('-') && t.contains('=') {
            i += 1;
            continue;
        }
        break;
    }
    (i < tokens.len()).then_some(i)
}

/// Whether a (program, args) pair is destructive on its own.
fn is_destructive_tokens(prog: &str, args: &[&str]) -> bool {
    if prog == "mkfs" || prog.starts_with("mkfs.") || prog == "mke2fs" {
        return true;
    }
    match prog {
        "rm" | "rmdir" | "del" | "deltree" | "unlink" | "shred" => true,
        "dd" | "fdisk" | "format" | "parted" | "wipefs" => true,
        "shutdown" | "reboot" | "halt" | "poweroff" | "init" | "telinit" => true,
        // Privilege escalation — always treat as destructive.
        "sudo" | "doas" | "pkexec" | "su" | "runuser" | "newgrp" => true,
        // Namespace/container escape primitives.
        "nsenter" | "unshare" | "chroot" => true,
        "chmod" => args.iter().copied().any(|a| {
            let a = a.trim_start_matches('-');
            a == "777" || a == "0777" || a == "a+rwx" || a == "a=rwx" || a == "u+rwx,go+rwx"
        }),
        "chown" | "chgrp" => args
            .iter()
            .copied()
            .any(|a| a == "-R" || a.starts_with("--recursive")),
        "install" => {
            let mut iter = args.iter().copied();
            while let Some(a) = iter.next() {
                if a == "-m" {
                    if let Some(mode) = iter.next() {
                        let m = mode.trim_start_matches('0');
                        if m == "777" || mode == "a+rwx" || mode == "a=rwx" {
                            return true;
                        }
                    }
                }
            }
            false
        }
        _ => false,
    }
}

/// Check if a command string is destructive.
///
/// This is deliberately conservative (false-positives preferred over
/// false-negatives): it inspects every sub-command in a pipeline/sequence,
/// normalizes whitespace and `$IFS` evasion, and skips wrapper prefixes like
/// `env`/`nohup`/`xargs` to reach the real program. It cannot defeat arbitrary
/// shell quoting/`$()` substitution, but it closes the common bypasses
/// (`rm\t-rf`, `rm${IFS}-rf`, `doas`, `chmod 0777`, `install -m 777`, …).
pub fn is_readonly_command(cmd: &str, sandbox_paths: &[std::path::PathBuf]) -> bool {
    let lower = normalize_command(cmd).to_lowercase();
    
    // Any shell metacharacters indicate potentially complex/unsafe commands.
    // E.g. >, >>, <, |, &, ;, $(, `
    if lower.contains('>') || lower.contains('<') || lower.contains('|')
        || lower.contains('&') || lower.contains(';') || lower.contains('`')
        || lower.contains("$(") {
        return false;
    }

    let tokens: Vec<&str> = lower.split_whitespace().collect();
    if let Some(idx) = effective_program_index(&tokens) {
        let prog = tokens[idx];
        let readonly_programs = [
            "ls", "cat", "echo", "pwd", "grep", "find", "rg", "head", "tail", "less", "more",
            "wc", "stat", "file", "which", "whereis", "whoami", "id", "groups", "uname",
            "date", "cal", "uptime", "w", "who", "du", "df", "ps", "top", "htop", "env",
            "printenv", "diff", "cmp", "tree", "sed", "awk",
            // PowerShell (Windows) — lowercase because `lower` is normalized
            "get-childitem", "gci", "dir", "get-content", "gc", "get-location", "gl",
            "select-string", "sls", "test-path", "get-item", "gi", "get-process", "gps",
            "get-date", "measure-object", "write-output", "write-host",
        ];
        
        if readonly_programs.contains(&prog) {
            // For sed/awk, only allow if they don't have write flags like -i
            if prog == "sed" || prog == "awk" {
                if tokens[idx + 1..].iter().any(|arg| arg.starts_with("-i") || *arg == "--in-place") {
                    return false;
                }
            }

            // Ensure no arguments try to read outside the sandbox via absolute paths or parent dirs
            for arg in &tokens[idx + 1..] {
                // If it's a flag, skip
                if arg.starts_with('-') {
                    continue;
                }

                if arg.contains("/../")
                    || *arg == ".."
                    || arg.starts_with("../")
                    || arg.ends_with("/..")
                    || arg.starts_with('~')
                {
                    return false;
                }
                
                if arg.starts_with('/') {
                    let p = std::path::Path::new(arg);
                    let mut allowed = false;
                    for sandbox in sandbox_paths {
                        if p.starts_with(sandbox) {
                            allowed = true;
                            break;
                        }
                    }
                    if !allowed {
                        return false;
                    }
                }
            }
            return true;
        }
    }
    false
}

pub fn is_destructive_command(cmd: &str) -> bool {
    let lower = normalize_command(cmd).to_lowercase();

    // Fork bomb.
    if lower.contains(":(){") || lower.contains(":|:&") {
        return true;
    }
    // Writing to a block device (cat foo > /dev/sda, dd … of=/dev/nvme0n1).
    let block_device =
        lower.contains("/dev/sd") || lower.contains("/dev/nvme") || lower.contains("/dev/disk");
    if block_device && (lower.contains('>') || lower.contains("of=")) {
        return true;
    }

    // Inspect each sub-command (split on `;`, `|`, `&`, newline).
    for sub in lower.split([';', '\n', '|', '&']) {
        let sub = sub.trim();
        if sub.is_empty() {
            continue;
        }
        let tokens: Vec<&str> = sub.split_whitespace().collect();
        if let Some(idx) = effective_program_index(&tokens) {
            let prog = tokens[idx];
            let args = &tokens[idx + 1..];
            if is_destructive_tokens(prog, args) {
                return true;
            }
        }
    }
    false
}

// ── Tests ───────────────────────────────────────────────────────────

