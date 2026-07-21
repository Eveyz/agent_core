use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use serde_json::{Value, json};

use super::Tool;

/// Maximum file size we will open at all (1 MB). Larger files must be read
/// in slices via offset/limit, never loaded whole.
const MAX_FILE_SIZE_BYTES: u64 = 1_048_576;

/// Default number of lines returned when `limit` is omitted. Chosen so a typical
/// single read lands well under the actively-read char cap (hygiene::policy,
/// 24K) and the model can page forward with offset. See PLAN-0008.
const MAX_LINES_DEFAULT: usize = 300;

/// Hard cap on the returned string length (defends against pathological single
/// lines, e.g. a 1 MB minified line). Aligns with the actively-read budget in
/// hygiene::policy so the tool-layer cap and the hygiene cap agree.
const MAX_OUTPUT_CHARS: usize = 24_000;

use crate::util::floor_char_boundary;

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a text file and return its contents with line numbers. Returns up to \
300 lines by default; use `offset` and `limit` to page through larger files. \
The output is prefixed with a `[Lines X-Y in 'path']` range header and a \
continuation hint when more lines remain. Each line is shown as \
`<number>\\t<content>` — strip the leading `N\\t` prefix before using a line as \
the `old_string` for the `edit` tool (edit matches the file's raw text, not the \
line-numbered view). Refuses files larger than 1 MB and detects binary files."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Absolute path of the file to read"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Line number to start reading from (1-based). Default 1."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum number of lines to return. Default 300."
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .context("missing required parameter 'path'")?;

        let offset = args["offset"].as_u64().unwrap_or(1) as usize;
        let limit = args["limit"].as_u64().unwrap_or(MAX_LINES_DEFAULT as u64) as usize;

        if offset == 0 {
            bail!("'offset' must be >= 1 (1-based line numbers)");
        }
        if limit == 0 {
            bail!("'limit' must be >= 1");
        }

        let session_id = args.get("_session_id").and_then(|v| v.as_str());
        let prompt_id = args.get("_prompt_id").and_then(|v| v.as_str());
        let working_dir = args.get("_working_dir").and_then(|v| v.as_str());
        let resolved_path =
            crate::paths::resolve_tool_path(path, session_id, prompt_id, working_dir);
        let resolved_path_str = resolved_path.to_string_lossy().to_string();

        // 1. Size guard before opening — use tokio::fs for async I/O.
        let metadata = tokio::fs::metadata(&resolved_path)
            .await
            .with_context(|| format!("failed to stat file: {resolved_path_str}"))?;
        if metadata.len() > MAX_FILE_SIZE_BYTES {
            bail!(
                "File is too large ({} bytes > {} byte limit). Use `offset` and `limit` to read it in slices.",
                metadata.len(),
                MAX_FILE_SIZE_BYTES
            );
        }

        // 2. Read entire file via tokio::fs to avoid blocking the async runtime.
        let resolved_path_str_clone = resolved_path_str.clone();
        let content = tokio::fs::read_to_string(&resolved_path)
            .await
            .with_context(|| format!("failed to read file: {resolved_path_str_clone}"))?;

        // Binary probe: reject on NUL bytes.
        if content.as_bytes().contains(&0u8) {
            bail!("File appears to be binary (contains NUL bytes). read_file only handles text files.");
        }

        let mut out = String::new();
        let mut line_num: usize = 0;
        let mut collected: usize = 0;
        let mut last_emitted: usize = 0;
        let mut hit_char_cap = false;

        // Process lines from the already-read content
        for line in content.lines() {
            line_num += 1;

            if line_num < offset {
                continue;
            }
            if collected >= limit {
                break;
            }

            // Strip the trailing newline so numbering is stable; we re-add it.
            let trimmed = line.strip_suffix('\n').unwrap_or(&line);
            let trimmed = trimmed.strip_suffix('\r').unwrap_or(trimmed);

            // Char-budget guard for pathological single lines / large slices.
            let numbered = format!("{:>6}\t{}\n", line_num, trimmed);
            if out.len() + numbered.len() > MAX_OUTPUT_CHARS {
                // If nothing has been emitted yet, the current line alone
                // exceeds the cap — emit a truncated prefix so the model still
                // sees content and gets a continuation hint.
                if out.is_empty() {
                    let prefix_len = format!("{:>6}\t", line_num).len();
                    let budget = MAX_OUTPUT_CHARS.saturating_sub(prefix_len + 1);
                    let end_byte = floor_char_boundary(trimmed, budget);
                    out.push_str(&format!("{:>6}\t{}\n", line_num, &trimmed[..end_byte]));
                    last_emitted = line_num;
                    collected += 1;
                }
                hit_char_cap = true;
                break;
            }
            out.push_str(&numbered);
            last_emitted = line_num;
            collected += 1;
        }

        if collected == 0 && line_num == 0 {
            bail!("File is empty: {path}");
        }

        let start = offset;
        // last_emitted is the actual last line written; fall back to start when
        // the file had exactly one matching line (or hit a cap after one line).
        let end = if last_emitted >= start {
            last_emitted
        } else {
            start
        };

        // 3. Assemble header + continuation hint.
        let mut result = String::new();
        result.push_str(&format!("[Lines {start}-{end} in '{path}']\n"));
        result.push_str(&out);

        // Continuation: more lines exist if we stopped because of `limit` or the
        // char cap (not EOF). line_num reflects how far we actually scanned.
        let stopped_early = collected >= limit || hit_char_cap;
        let more_after = line_num > end;
        if hit_char_cap {
            // Char-cap hit (e.g. one enormous line): always surface a truncation
            // marker. Point to the next line for continuation if more lines exist.
            let next = end + 1;
            if more_after {
                result.push_str(&format!(
                    "\n[Output truncated at {MAX_OUTPUT_CHARS} chars. Re-read with offset={next} to continue.]\n"
                ));
            } else {
                result.push_str(&format!(
                    "\n[Output truncated at {MAX_OUTPUT_CHARS} chars; remaining content of this line omitted.]\n"
                ));
            }
        } else if stopped_early && more_after {
            let next = end + 1;
            result.push_str(&format!(
                "\n[Showing {collected} of more lines. Re-read with offset={next} to continue.]\n"
            ));
        }

        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("read_file_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        std::fs::write(&p, content).unwrap();
        p
    }

    #[tokio::test]
    async fn reads_full_small_file_with_line_numbers() {
        let p = write_tmp("small.txt", "alpha\nbeta\ngamma\n");
        let out = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await.unwrap();
        assert!(out.contains("[Lines 1-3 in"));
        assert!(out.contains("     1\talpha"));
        assert!(out.contains("     2\tbeta"));
        assert!(out.contains("     3\tgamma"));
        // No continuation hint — file fully read.
        assert!(!out.contains("Re-read with offset"));
    }

    #[tokio::test]
    async fn offset_limit_returns_slice() {
        let content: String = (1..=50).map(|i| format!("line {i}\n")).collect();
        let p = write_tmp("slice.txt", &content);
        let out = ReadFileTool
            .execute(json!({"path": p.to_string_lossy(), "offset": 10, "limit": 5}))
            .await
            .unwrap();
        assert!(out.contains("[Lines 10-14 in"));
        assert!(out.contains("    10\tline 10"));
        assert!(out.contains("    14\tline 14"));
        assert!(!out.contains("line 9\n"));
        assert!(!out.contains("line 15"));
        assert!(out.contains("Re-read with offset=15"));
    }

    #[tokio::test]
    async fn default_limit_caps_at_300_with_hint() {
        let content: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        let p = write_tmp("big.txt", &content);
        let out = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await.unwrap();
        assert!(out.contains("[Lines 1-300 in"));
        assert!(out.contains("line 300"));
        assert!(!out.contains("line 301"));
        assert!(out.contains("Re-read with offset=301"));
    }

    #[tokio::test]
    async fn continuation_offset_follows_correctly() {
        let content: String = (1..=500).map(|i| format!("line {i}\n")).collect();
        let p = write_tmp("big2.txt", &content);
        let out = ReadFileTool
            .execute(json!({"path": p.to_string_lossy(), "offset": 301, "limit": 300}))
            .await
            .unwrap();
        assert!(out.contains("[Lines 301-500 in"));
        assert!(out.contains("line 500"));
        // Last page — no continuation hint.
        assert!(!out.contains("Re-read with offset"));
    }

    #[tokio::test]
    async fn rejects_binary_nul_bytes() {
        let p = write_tmp("bin.dat", "ab\x00cd\nef\n");
        let res = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("binary"));
    }

    #[tokio::test]
    async fn rejects_invalid_utf8() {
        // Invalid UTF-8 sequence (lone continuation byte).
        let dir = std::env::temp_dir().join("read_file_tests");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("bad.txt");
        std::fs::write(&p, b"valid\n\xff\xfe\nmore\n").unwrap();
        let res = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn rejects_oversize_file_without_reading() {
        // Write a 2 MB file.
        let p = write_tmp("huge.txt", &"x".repeat(2 * 1_048_576));
        let res = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await;
        assert!(res.is_err());
        let msg = res.unwrap_err().to_string();
        assert!(msg.contains("too large"));
        assert!(msg.contains("offset"));
    }

    #[tokio::test]
    async fn rejects_zero_offset() {
        let p = write_tmp("z.txt", "x\n");
        let res = ReadFileTool
            .execute(json!({"path": p.to_string_lossy(), "offset": 0})
            )
            .await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn empty_file_errors() {
        let p = write_tmp("empty.txt", "");
        let res = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn offset_beyond_end_returns_empty_range() {
        let p = write_tmp("short.txt", "only\none\n");
        let res = ReadFileTool
            .execute(json!({"path": p.to_string_lossy(), "offset": 999})
            )
            .await;
        // No lines collected and file is non-empty → no error, empty body under header.
        let out = res.unwrap();
        assert!(out.contains("[Lines 999-999 in"));
        assert!(!out.contains("Re-read with offset"));
    }

    #[tokio::test]
    async fn line_numbers_are_right_aligned() {
        let content: String = (1..=1500).map(|i| format!("l{i}\n")).collect();
        let p = write_tmp("aligned.txt", &content);
        let out = ReadFileTool
            .execute(json!({"path": p.to_string_lossy(), "offset": 1000, "limit": 10})
            )
            .await
            .unwrap();
        assert!(out.contains("  1000\tl1000"));
        assert!(out.contains("  1009\tl1009"));
    }

    #[tokio::test]
    async fn single_huge_line_char_capped() {
        // One line bigger than MAX_OUTPUT_CHARS but file under 1 MB.
        let line = "x".repeat(MAX_OUTPUT_CHARS + 5000);
        let p = write_tmp("oneline.txt", &line);
        let out = ReadFileTool.execute(json!({"path": p.to_string_lossy()})).await.unwrap();
       assert!(out.contains("[Lines 1-1 in"));
       assert!(out.contains("truncated"));
        // Single-line file: no following lines, so the cap notes omitted
        // line content rather than offering an offset continuation.
        assert!(out.contains("remaining content of this line omitted"));
    }
}
