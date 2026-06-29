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
