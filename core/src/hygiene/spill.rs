//! Spill oversized incidental tool results to disk before conversation ingest
//! (PLAN-0016). Live UI still receives the full body via `ToolEnded`; persisted
//! history and the model window store a tail-heavy truncation + spill path.

use std::fs;
use std::path::Path;

use super::policy;

/// Prepare a tool result for `append_conversation`.
///
/// For oversized incidental output: write the full body to `spill_path` (best
/// effort) and return a tail-heavy truncation that references that path.
/// Otherwise returns `content` unchanged (or actively-read/instruction
/// truncation without spill).
pub fn prepare_tool_result_for_storage(
    tool_name: Option<&str>,
    content: &str,
    spill_path: &Path,
) -> String {
    if !policy::should_spill_at_ingest(tool_name, content) {
        // Safety-net truncate for actively-read over char cap (no spill).
        return policy::truncate_content(tool_name, content).unwrap_or_else(|| content.to_string());
    }

    let spill_str = spill_path.display().to_string();
    if let Err(e) = write_spill(spill_path, content) {
        tracing::warn!(
            path = %spill_str,
            error = %e,
            "Failed to spill oversized tool output; truncating without spill path"
        );
        return policy::truncate_content(tool_name, content).unwrap_or_else(|| content.to_string());
    }

    policy::truncate_content_with_spill(tool_name, content, Some(&spill_str))
        .unwrap_or_else(|| content.to_string())
}

fn write_spill(path: &Path, content: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn spill_and_truncate_oversized_shell() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("call1.txt");
        let big: String = (0..policy::INCIDENTAL_MAX_LINES + 500)
            .map(|i| format!("line number {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(policy::incidental_over_budget(&big));

        let out = prepare_tool_result_for_storage(Some("shell"), &big, &path);
        assert!(path.exists());
        assert_eq!(fs::read_to_string(&path).unwrap(), big);
        assert!(out.contains("truncated"));
        assert!(out.contains("tail-heavy"));
        assert!(out.contains(path.to_str().unwrap()));
        assert!(out.contains("read_file"));
        assert!(out.contains(&format!(
            "line number {}",
            policy::INCIDENTAL_MAX_LINES + 499
        )));
        assert!(out.len() < big.len());
    }

    #[test]
    fn short_content_unchanged_no_file() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("call2.txt");
        let small = "hello";
        let out = prepare_tool_result_for_storage(Some("shell"), small, &path);
        assert_eq!(out, small);
        assert!(!path.exists());
    }

    #[test]
    fn read_file_not_spilled() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("call3.txt");
        // Over incidental budget but actively-read — no spill at ingest.
        let big = "x".repeat(20_000);
        let out = prepare_tool_result_for_storage(Some("read_file"), &big, &path);
        assert!(!path.exists());
        assert_eq!(out, big); // under ACTIVE_READ_MAX_CHARS
    }
}
