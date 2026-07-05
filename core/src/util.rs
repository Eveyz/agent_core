//! Shared utility functions used across the crate.

use std::path::PathBuf;

/// Find the largest valid UTF-8 character boundary at or before `idx`.
/// Prevents panics when slicing strings at arbitrary byte offsets.
pub fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx >= s.len() {
        return s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

/// Expand a tilde (`~`) path to an absolute path using the user's home
/// directory. Falls back to the original path if HOME/USERPROFILE is
/// unavailable.
pub fn expand_tilde(path: &str) -> String {
    if path.starts_with('~') {
        if let Ok(home) = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
            if path == "~" {
                return home;
            }
            if path.starts_with("~/") {
                return format!("{}/{}", home, &path[2..]);
            }
        }
    }
    path.to_string()
}
