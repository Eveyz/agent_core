#[cfg(test)]
mod tests {
    use super::*;

    fn make_policy() -> PermissionPolicy {
        PermissionPolicy::with_builtin_defaults().with_mode(PermissionMode::Standard)
    }

    #[test]
    fn test_read_file_allowed() {
        let mut policy = make_policy();
        let result = policy.check("read_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_destructive_command_denied() {
        let mut policy = make_policy();
        let result = policy.check(
            "shell",
            r#"{"command":"rm -rf /"}"#,
            Some("rm -rf /"),
            None,
            None,
        );
        assert!(result.is_denied());
    }

    #[test]
    fn test_safe_command_auto_allowed() {
        // Safe shell commands (ls, cat, grep, …) are auto-allowed by the
        // built-in readonly-command whitelist — they no longer prompt.
        let mut policy = make_policy();
        let result = policy.check(
            "shell",
            r#"{"command":"ls -la"}"#,
            Some("ls -la"),
            None,
            None,
        );
        assert!(result.is_allowed());
    }

    #[test]
    fn test_unknown_shell_command_asks_by_default() {
        // Non-readonly, non-destructive commands fall through to the
        // System→Ask catch-all and prompt for approval.
        let mut policy = make_policy();
        let result = policy.check(
            "shell",
            r#"{"command":"make build"}"#,
            Some("make build"),
            None,
            None,
        );
        assert!(result.needs_approval());
    }

    #[test]
    fn test_whitelist_overrides_default() {
        let mut policy = make_policy();
        policy.whitelist_mut().add(WhitelistEntry::new(
            ToolPermissionPattern::simple("shell"),
            ApprovalScope::Session,
        ));
        let result = policy.check("shell", r#"{"command":"ls"}"#, Some("ls"), None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_yolo_mode_allows_everything() {
        let mut policy = make_policy();
        policy.mode = PermissionMode::Yolo;
        let result = policy.check(
            "shell",
            r#"{"command":"rm -rf /"}"#,
            Some("rm -rf /"),
            None,
            None,
        );
        assert!(result.is_allowed());
    }

    #[test]
    fn test_paranoid_mode_asks_for_read() {
        let mut policy = make_policy();
        policy.mode = PermissionMode::Paranoid;
        let result = policy.check("read_file", "{}", None, None, None);
        assert!(result.needs_approval() || result.is_allowed());
    }

    #[test]
    fn test_blacklist_overrides_whitelist() {
        let mut policy = make_policy();
        policy.whitelist_mut().add(WhitelistEntry::new(
            ToolPermissionPattern::simple("shell"),
            ApprovalScope::Persistent,
        ));
        policy.blacklist.push(ToolPermissionPattern::simple("shell"));
        let result = policy.check("shell", r#"{"command":"ls"}"#, Some("ls"), None, None);
        assert!(result.is_denied());
    }

    #[test]
    fn test_auto_allow_up_to() {
        let mut policy = make_policy();
        policy.auto_allow_up_to = Some(DangerLevel::ReadWrite);
        // write_file is ReadWrite, should now be auto-allowed
        let result = policy.check("write_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_config_rule_overrides_builtin() {
        let mut policy = make_policy();
        policy.add_rule(ConfigRule {
            pattern: ToolPermissionPattern::simple("write_file"),
            level: ApprovalLevel::Allow,
        });
        // write_file is normally Ask; config rule overrides to Allow
        let result = policy.check("write_file", "{}", None, None, None);
        assert!(result.is_allowed());
    }

    #[test]
    fn test_backward_compat_permissive() {
        let mut policy = PermissionPolicy::with_permissive_defaults();
        let result = policy.check("unknown_tool", "{}", None, None, None);
        assert!(result.is_allowed()); // permissive default
    }

    #[test]
    fn test_is_destructive_command() {
        // Basic destructive commands.
        assert!(is_destructive_command("rm -rf /"));
        assert!(is_destructive_command("sudo rm file"));
        assert!(is_destructive_command("mkfs.ext4 /dev/sda"));
        assert!(is_destructive_command("dd if=/dev/zero of=/dev/sda"));
        // Whitespace / IFS evasion that the old substring check missed.
        assert!(is_destructive_command("rm\t-rf /"));
        assert!(is_destructive_command("rm${IFS}-rf /"));
        assert!(is_destructive_command("rm\n-rf /"));
        // Escalators the old check missed.
        assert!(is_destructive_command("doas rm file"));
        assert!(is_destructive_command("pkexec reboot"));
        assert!(is_destructive_command("nsenter -t 1 -m sh"));
        assert!(is_destructive_command("chroot / /bin/sh"));
        // chmod / install variants the old check missed.
        assert!(is_destructive_command("chmod 0777 /etc"));
        assert!(is_destructive_command("chmod a=rwx file"));
        assert!(is_destructive_command(
            "install -m 777 script /usr/local/bin/script"
        ));
        // Wrapper-prefixed destructive command.
        assert!(is_destructive_command("env rm -rf /tmp"));
        assert!(is_destructive_command("nohup rm -rf /tmp &"));
        // Block-device overwrite via redirection.
        assert!(is_destructive_command("cat image.img > /dev/sda"));
        // Fork bomb.
        assert!(is_destructive_command(":(){ :|:& };:"));
        // Sub-command after a separator.
        assert!(is_destructive_command("echo hi; rm -rf /tmp"));
        // Safe commands.
        assert!(!is_destructive_command("git status"));
        assert!(!is_destructive_command("cargo build"));
        assert!(!is_destructive_command("python script.py"));
        assert!(!is_destructive_command("ls -la"));
        assert!(!is_destructive_command("chmod 644 file"));
    }

    #[test]
    fn test_sandbox_path() {
        let policy =
            PermissionPolicy::new().with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);
        assert!(policy.check_path("/etc/passwd").is_err());
        assert!(policy.check_path("/home/user/file.txt").is_err());
    }

    #[test]
    fn test_sandbox_denies_outside_in_check() {
        let mut policy = PermissionPolicy::with_builtin_defaults()
            .with_sandbox_paths(vec![PathBuf::from("/tmp/sandbox")]);
        // write_file inside the sandbox: reaches the normal Ask path.
        let inside = policy.check(
            "write_file",
            r#"{"path":"/tmp/sandbox/a.txt","content":"x"}"#,
            None,
            Some("/tmp/sandbox/a.txt"),
            None,
        );
        assert!(inside.needs_approval() || inside.is_allowed());
        // write_file outside the sandbox: hard-denied before any rule.
        let outside = policy.check(
            "write_file",
            r#"{"path":"/etc/passwd","content":"x"}"#,
            None,
            Some("/etc/passwd"),
            None,
        );
        assert!(outside.is_denied());
    }

    #[test]
    fn test_auto_allow_does_not_bypass_destructive_deny() {
        // `auto_allow_up_to = Destructive` must NOT auto-allow `rm -rf /`;
        // the built-in destructive deny should still fire.
        let mut policy = PermissionPolicy::with_builtin_defaults()
            .with_auto_allow_up_to(DangerLevel::Destructive);
        let result = policy.check(
            "shell",
            r#"{"command":"rm -rf /"}"#,
            Some("rm -rf /"),
            None,
            None,
        );
        assert!(result.is_denied());
        // A safe command at or below the auto-allow level is still allowed.
        let safe = policy.check(
            "shell",
            r#"{"command":"ls -la"}"#,
            Some("ls -la"),
            None,
            None,
        );
        assert!(safe.is_allowed());
    }
}
