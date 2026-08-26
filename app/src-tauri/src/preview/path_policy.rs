//! Canonical workspace path validation for the preview gateway.

use std::path::{Component, Path, PathBuf};

use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum PathPolicyError {
    #[error("empty path")]
    Empty,
    #[error("invalid path encoding")]
    InvalidEncoding,
    #[error("path traversal rejected")]
    Traversal,
    #[error("absolute path rejected")]
    Absolute,
    #[error("path outside workspace")]
    OutsideRoot,
    #[error("symlink escape rejected")]
    SymlinkEscape,
    #[error("entrypoint not found")]
    NotFound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPolicyOptions {
    /// When true, reject any symlink in the resolved path chain.
    pub deny_all_symlinks: bool,
}

impl Default for PathPolicyOptions {
    fn default() -> Self {
        Self {
            deny_all_symlinks: false,
        }
    }
}

/// Percent-decode a URL path segment exactly once.
pub fn decode_request_path(raw: &str) -> Result<String, PathPolicyError> {
    if raw.is_empty() {
        return Err(PathPolicyError::Empty);
    }
    if raw.contains('\0') {
        return Err(PathPolicyError::InvalidEncoding);
    }
    if raw.contains('\\') {
        return Err(PathPolicyError::Traversal);
    }
    let decoded = percent_encoding::percent_decode_str(raw)
        .decode_utf8()
        .map_err(|_| PathPolicyError::InvalidEncoding)?;
    if decoded.contains('\0') || decoded.contains('\\') {
        return Err(PathPolicyError::InvalidEncoding);
    }
    Ok(decoded.into_owned())
}

/// Reject dangerous relative path components before joining to root.
pub fn sanitize_relative_path(rel: &str) -> Result<PathBuf, PathPolicyError> {
    if rel.is_empty() {
        return Ok(PathBuf::from("index.html"));
    }
    if rel.starts_with('/') || rel.starts_with("\\\\") {
        return Err(PathPolicyError::Absolute);
    }
    // Windows drive prefixes: C:\ or C:/
    if rel.len() >= 2 {
        let bytes = rel.as_bytes();
        if bytes[1] == b':' {
            return Err(PathPolicyError::Absolute);
        }
    }

    let path = Path::new(rel);
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Normal(seg) => {
                if seg == "." || seg == ".." {
                    return Err(PathPolicyError::Traversal);
                }
                out.push(seg);
            }
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(PathPolicyError::Traversal);
            }
        }
    }
    if out.as_os_str().is_empty() {
        out.push("index.html");
    }
    Ok(out)
}

/// Resolve a request-relative path under a canonical workspace root.
pub fn resolve_under_root(
    root: &Path,
    rel: &str,
    options: PathPolicyOptions,
) -> Result<PathBuf, PathPolicyError> {
    let decoded = decode_request_path(rel)?;
    let relative = sanitize_relative_path(&decoded)?;
    let joined = root.join(&relative);
    canonicalize_with_policy(&joined, root, options)
}

/// Canonicalize an existing path and verify it stays within root.
pub fn canonicalize_with_policy(
    candidate: &Path,
    root: &Path,
    options: PathPolicyOptions,
) -> Result<PathBuf, PathPolicyError> {
    let canonical_root = root.canonicalize().map_err(|_| PathPolicyError::NotFound)?;

    if !candidate.exists() {
        return Err(PathPolicyError::NotFound);
    }

    if options.deny_all_symlinks && path_contains_symlink(candidate) {
        return Err(PathPolicyError::SymlinkEscape);
    }

    let canonical = candidate
        .canonicalize()
        .map_err(|_| PathPolicyError::NotFound)?;

    if !canonical.starts_with(&canonical_root) {
        return Err(PathPolicyError::OutsideRoot);
    }

    Ok(canonical)
}

/// Normalize an entrypoint to a workspace-relative path.
///
/// Accepts relative paths (`index.html`) or absolute paths under the workspace root.
pub fn normalize_entrypoint(root: &Path, raw: &str) -> Result<String, PathPolicyError> {
    let trimmed = raw.trim();
    let rel = if trimmed.is_empty() {
        "index.html".to_string()
    } else {
        let path = Path::new(trimmed);
        if path.is_absolute() {
            let canonical_root = root.canonicalize().map_err(|_| PathPolicyError::NotFound)?;
            let abs = path.canonicalize().map_err(|_| PathPolicyError::NotFound)?;
            if !abs.starts_with(&canonical_root) {
                return Err(PathPolicyError::OutsideRoot);
            }
            abs.strip_prefix(&canonical_root)
                .map_err(|_| PathPolicyError::OutsideRoot)?
                .display()
                .to_string()
        } else {
            sanitize_relative_path(trimmed)?.display().to_string()
        }
    };

    let _ = resolve_under_root(root, &rel, PathPolicyOptions::default())?;
    Ok(rel)
}

fn path_contains_symlink(path: &Path) -> bool {
    let mut current = path.to_path_buf();
    loop {
        if std::fs::symlink_metadata(&current)
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false)
        {
            return true;
        }
        if !current.pop() {
            break;
        }
    }
    false
}

/// Build default CSP for preview responses.
pub fn default_preview_csp(extra_connect: &[&str]) -> String {
    let mut connect = vec!["'self'", "ws://127.0.0.1:*", "ws://localhost:*"];
    connect.extend(extra_connect.iter().copied());
    format!(
        "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; \
         img-src 'self' data: blob:; font-src 'self' data:; connect-src {}; \
         worker-src 'self' blob:; frame-src 'none'; object-src 'none'; base-uri 'none'; \
         form-action 'none'",
        connect.join(" ")
    )
}

/// Redact likely secrets from log lines.
pub fn redact_log_line(line: &str) -> String {
    let patterns = [
        ("api_key=", "***"),
        ("API_KEY=", "***"),
        ("token=", "***"),
        ("password=", "***"),
        ("secret=", "***"),
        ("Bearer ", "Bearer ***"),
    ];
    let mut out = line.to_string();
    for (needle, replacement) in patterns {
        if let Some(idx) = out.find(needle) {
            let start = idx + needle.len();
            if start < out.len() {
                if let Some(end) = out[start..].find(|c: char| c.is_whitespace()) {
                    out.replace_range(start..start + end, replacement);
                } else {
                    out.replace_range(start.., replacement);
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn rejects_traversal_segments() {
        assert_eq!(
            sanitize_relative_path("../etc/passwd").unwrap_err(),
            PathPolicyError::Traversal
        );
        assert_eq!(
            sanitize_relative_path("foo/../../secret").unwrap_err(),
            PathPolicyError::Traversal
        );
    }

    #[test]
    fn rejects_windows_absolute_prefix() {
        assert_eq!(
            sanitize_relative_path(r"C:\Windows\System32").unwrap_err(),
            PathPolicyError::Absolute
        );
    }

    #[test]
    fn rejects_percent_encoded_traversal_after_decode() {
        let rel = decode_request_path("%2e%2e%2f").unwrap();
        assert_eq!(
            sanitize_relative_path(&rel).unwrap_err(),
            PathPolicyError::Traversal
        );
    }

    #[test]
    fn resolves_index_for_empty_path() {
        let p = sanitize_relative_path("").unwrap();
        assert_eq!(p, PathBuf::from("index.html"));
    }

    #[test]
    fn symlink_outside_root_rejected() {
        let dir = tempdir().unwrap();
        let root = dir.path().join("workspace");
        let outside = dir.path().join("outside.txt");
        fs::write(&outside, "secret").unwrap();
        fs::create_dir(&root).unwrap();
        let link = root.join("escape.txt");
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(&outside, &link).unwrap();
            let err =
                resolve_under_root(&root, "escape.txt", PathPolicyOptions::default()).unwrap_err();
            assert_eq!(err, PathPolicyError::OutsideRoot);
        }
    }

    #[test]
    fn csp_includes_self_and_ws() {
        let csp = default_preview_csp(&[]);
        assert!(csp.contains("connect-src 'self'"));
        assert!(csp.contains("ws://127.0.0.1:*"));
    }

    #[test]
    fn normalize_absolute_entrypoint_under_root() {
        let dir = tempfile::tempdir().unwrap();
        let html = dir.path().join("index.html");
        std::fs::write(&html, "<html></html>").unwrap();
        let abs = html.canonicalize().unwrap();
        let rel = normalize_entrypoint(dir.path(), &abs.to_string_lossy()).unwrap();
        assert_eq!(rel, "index.html");
    }

    #[test]
    fn rejects_absolute_entrypoint_outside_root() {
        let dir = tempfile::tempdir().unwrap();
        let outside = std::env::temp_dir().join("outside-preview-test.html");
        std::fs::write(&outside, "<html></html>").unwrap();
        let err = normalize_entrypoint(dir.path(), &outside.to_string_lossy()).unwrap_err();
        assert_eq!(err, PathPolicyError::OutsideRoot);
        let _ = std::fs::remove_file(outside);
    }

    #[test]
    fn redacts_common_secrets() {
        let line = "token=abc123 password=secret api_key=xyz";
        let redacted = redact_log_line(line);
        assert!(!redacted.contains("abc123"));
        assert!(!redacted.contains("secret"));
    }
}
