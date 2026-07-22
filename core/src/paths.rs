use std::path::PathBuf;

/// Gets the root `.agverse` directory. Usually `~/.agverse`.
pub fn get_agverse_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".agverse")
}

/// Gets the memory database path. Usually `~/.agverse/memory.db`.
pub fn get_memory_db_path() -> PathBuf {
    get_agverse_dir().join("memory.db")
}


/// Gets the run event logs directory. Usually `~/.agverse/runs/`.
pub fn get_runs_dir() -> PathBuf {
    get_agverse_dir().join("runs")
}

/// Gets the reflector skills directory. Usually `~/.agverse/skills/`.
pub fn get_skills_dir() -> PathBuf {
    get_agverse_dir().join("skills")
}

/// Gets the diff observer snapshots directory. Usually `~/.agverse/snapshots/`.
pub fn get_snapshots_dir() -> PathBuf {
    get_agverse_dir().join("snapshots")
}

/// Gets the CLI history directory. Usually `~/.agverse_history/`.
pub fn get_cli_history_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .join(".agverse_history")
}

/// Gets the global project instruction file path. Usually `~/.agverse/agverse.md`.
pub fn get_global_agverse_md_path() -> PathBuf {
    get_agverse_dir().join("agverse.md")
}

/// Session directory: `~/.agverse/sessions/<session_id>/`.
pub fn session_dir(session_id: &str) -> PathBuf {
    get_agverse_dir().join("sessions").join(session_id)
}

/// Oversized tool-output spills for a session: `…/sessions/<id>/tool_spills/`.
pub fn session_tool_spills_dir(session_id: &str) -> PathBuf {
    session_dir(session_id).join("tool_spills")
}

/// Global fallback spills when no session id: `~/.agverse/tool_spills/`.
pub fn global_tool_spills_dir() -> PathBuf {
    get_agverse_dir().join("tool_spills")
}

/// Absolute path for a single spilled tool result (`<call_id>.txt`).
pub fn tool_spill_path(session_id: Option<&str>, call_id: &str) -> PathBuf {
    let safe: String = call_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = if safe.is_empty() {
        format!("{}.txt", uuid_like())
    } else {
        format!("{safe}.txt")
    };
    match session_id {
        Some(sid) => session_tool_spills_dir(sid).join(name),
        None => global_tool_spills_dir().join(name),
    }
}

fn uuid_like() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}")
}

/// Mid-run crash snapshot: `~/.agverse/sessions/<session_id>/messages.json`.
pub fn session_messages_snapshot_path(session_id: &str) -> PathBuf {
    session_dir(session_id).join("messages.json")
}

/// Prompt folder: `~/.agverse/sessions/<session_id>/<prompt_id>/`.
pub fn prompt_dir(session_id: &str, prompt_id: &str) -> PathBuf {
    session_dir(session_id).join(prompt_id)
}

/// User image attachments for a prompt: `…/<prompt_id>/images/`.
pub fn prompt_images_dir(session_id: &str, prompt_id: &str) -> PathBuf {
    prompt_dir(session_id, prompt_id).join("images")
}

/// Redirect system artifacts / generated media into the prompt folder.
pub fn redirect_if_artifact(path: &str, session_id: &str, prompt_id: &str) -> PathBuf {
    let path_obj = std::path::Path::new(path);
    let file_name = path_obj.file_name().and_then(|n| n.to_str()).unwrap_or("");
    let name_lower = file_name.to_lowercase();

    // System artifacts: plan.md, implementation_plan.md, walkthrough.md, task.md
    // Or media files (.png, .jpg, .jpeg, .webp, .gif)
    let is_artifact = name_lower == "plan.md"
        || name_lower == "implementation_plan.md"
        || name_lower == "walkthrough.md"
        || name_lower == "task.md"
        || name_lower.ends_with(".png")
        || name_lower.ends_with(".jpg")
        || name_lower.ends_with(".jpeg")
        || name_lower.ends_with(".webp")
        || name_lower.ends_with(".gif");

    if is_artifact {
        let dir = prompt_dir(session_id, prompt_id);
        let _ = std::fs::create_dir_all(&dir);
        dir.join(file_name)
    } else {
        PathBuf::from(path)
    }
}

/// Resolve a tool path under an optional working directory while preserving
/// the prompt artifact redirect behavior for generated plans/media.
pub fn resolve_tool_path(
    path: &str,
    session_id: Option<&str>,
    prompt_id: Option<&str>,
    working_dir: Option<&str>,
) -> PathBuf {
    let redirected = match (session_id, prompt_id) {
        (Some(sid), Some(pid)) => redirect_if_artifact(path, sid, pid),
        _ => PathBuf::from(path),
    };

    if redirected.is_absolute() {
        redirected
    } else if let Some(wd) = working_dir {
        PathBuf::from(wd).join(redirected)
    } else {
        redirected
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_and_prompt_layout() {
        let sid = "sess-a";
        let pid = "prompt-b";
        let session = session_dir(sid);
        assert!(session.ends_with("sessions/sess-a") || session.ends_with("sessions\\sess-a"));
        assert_eq!(prompt_dir(sid, pid), session.join(pid));
        assert_eq!(prompt_images_dir(sid, pid), session.join(pid).join("images"));
        assert_eq!(
            session_messages_snapshot_path(sid),
            session.join("messages.json")
        );
    }

    #[test]
    fn tool_spill_path_layout() {
        let p = tool_spill_path(Some("sess-a"), "call/1:xyz");
        assert!(
            p.ends_with("sessions/sess-a/tool_spills/call_1_xyz.txt")
                || p.ends_with("sessions\\sess-a\\tool_spills\\call_1_xyz.txt")
        );
        let global = tool_spill_path(None, "abc");
        assert!(
            global.ends_with("tool_spills/abc.txt") || global.ends_with("tool_spills\\abc.txt")
        );
    }

    #[test]
    fn redirect_artifacts_into_prompt_dir() {
        let sid = "s1";
        let pid = "p1";
        let redirected = redirect_if_artifact("plan.md", sid, pid);
        assert_eq!(redirected, prompt_dir(sid, pid).join("plan.md"));
        let normal = redirect_if_artifact("src/main.rs", sid, pid);
        assert_eq!(normal, PathBuf::from("src/main.rs"));
    }
}
