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

/// Helper to check if a file path is a system artifact or user-uploaded media
/// and redirect it to the session's chat folder if so.
pub fn redirect_if_artifact(path: &str, session_id: &str) -> PathBuf {
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
        let chat_dir = get_agverse_dir().join("chats").join(session_id);
        let _ = std::fs::create_dir_all(&chat_dir);
        chat_dir.join(file_name)
    } else {
        PathBuf::from(path)
    }
}
