use crate::reflector::digester::{DigestEvent, DigestEventKind};
use std::collections::HashSet;
use std::path::Path;

pub struct DiffObserver;

#[derive(Debug, Clone)]
pub struct UserEditDiffEvent {
    pub file_path: String,
    pub diff: String,
}

impl DiffObserver {
    pub fn take_snapshot(run_id: &str, events: &[DigestEvent]) {
        let mut paths = HashSet::new();
        for ev in events {
            if ev.kind == DigestEventKind::ToolStart {
                if let Some(args) = &ev.args {
                    for key in &["file_path", "target_file", "path", "file", "TargetFile"] {
                        if let Some(val) = args.get(key) {
                            if let Some(path_str) = val.as_str() {
                                paths.insert(path_str.to_string());
                            }
                        }
                    }
                }
            }
        }

        if paths.is_empty() {
            return;
        }

        let snapshot_dir = crate::paths::get_snapshots_dir().join(run_id);
        if let Err(e) = std::fs::create_dir_all(&snapshot_dir) {
            tracing::warn!("Failed to create snapshot dir: {}", e);
            return;
        }

        for path_str in paths {
            let path = Path::new(&path_str);
            if !path.exists() || !path.is_file() {
                continue;
            }
            // limit to < 1MB
            if let Ok(metadata) = std::fs::metadata(path) {
                if metadata.len() > 1024 * 1024 {
                    continue;
                }
            }

            // Copy file to snapshot dir, preserving the absolute path structure by replacing slashes
            let safe_name = path_str.replace("/", "_");
            let dest = snapshot_dir.join(safe_name);
            if let Err(e) = std::fs::copy(path, &dest) {
                tracing::warn!("Failed to snapshot {}: {}", path_str, e);
            }
        }
    }

    pub fn check_for_diffs(previous_run_id: &str) -> Vec<UserEditDiffEvent> {
        let snapshot_dir = crate::paths::get_snapshots_dir().join(previous_run_id);
        if !snapshot_dir.exists() {
            return vec![];
        }

        let mut diffs = Vec::new();
        if let Ok(entries) = std::fs::read_dir(&snapshot_dir) {
            for entry in entries.flatten() {
                let file_name = entry.file_name().to_string_lossy().to_string();
                let orig_path_str = file_name.replace("_", "/");
                let orig_path = Path::new(&orig_path_str);

                if !orig_path.exists() {
                    continue;
                }

                if let Ok(snap_content) = std::fs::read_to_string(entry.path()) {
                    if let Ok(curr_content) = std::fs::read_to_string(orig_path) {
                        if snap_content != curr_content {
                            use similar::TextDiff;
                            let diff = TextDiff::from_lines(&snap_content, &curr_content);
                            let mut diff_bytes = Vec::new();
                            let _ = diff
                                .unified_diff()
                                .header(&orig_path_str, &orig_path_str)
                                .context_radius(3)
                                .to_writer(&mut diff_bytes);

                            if let Ok(diff_str) = String::from_utf8(diff_bytes) {
                                diffs.push(UserEditDiffEvent {
                                    file_path: orig_path_str,
                                    diff: diff_str,
                                });
                            }
                        }
                    }
                }
            }
        }

        // Prune the snapshot after checking
        let _ = std::fs::remove_dir_all(&snapshot_dir);

        diffs
    }
}
