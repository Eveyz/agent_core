//! File watcher with debounced reload notifications.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebounceEventResult};
use parking_lot::Mutex;
use tokio::sync::{broadcast, mpsc};

const DEBOUNCE_MS: u64 = 120;
const CHANNEL_CAPACITY: usize = 256;

/// Directories and file patterns to ignore when watching.
fn should_ignore(path: &Path) -> bool {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    matches!(
        name,
        ".git"
            | "node_modules"
            | "target"
            | "dist"
            | "build"
            | ".next"
            | ".turbo"
            | ".cache"
            | ".DS_Store"
    ) || name.ends_with('~')
        || name.ends_with(".swp")
        || name.ends_with(".tmp")
}

pub struct PreviewWatcher {
    _debouncer: notify_debouncer_mini::Debouncer<notify::RecommendedWatcher>,
    revision: Arc<Mutex<u64>>,
}

impl PreviewWatcher {
    pub fn start(
        root: PathBuf,
        reload_tx: broadcast::Sender<ReloadNotification>,
    ) -> anyhow::Result<Self> {
        let revision = Arc::new(Mutex::new(0));
        let revision_cb = revision.clone();
        let root_cb = root.clone();

        let (event_tx, mut event_rx) = mpsc::channel::<DebounceEventResult>(CHANNEL_CAPACITY);

        let mut debouncer = new_debouncer(
            Duration::from_millis(DEBOUNCE_MS),
            move |res: DebounceEventResult| {
                let _ = event_tx.blocking_send(res);
            },
        )?;

        debouncer
            .watcher()
            .watch(&root, RecursiveMode::Recursive)?;

        let reload_tx_task = reload_tx.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(result) = event_rx.recv().await {
                match result {
                    Ok(events) => {
                        let mut paths = Vec::new();
                        for event in events {
                            let path = event.path;
                            if should_ignore(&path) {
                                continue;
                            }
                            if !path.starts_with(&root_cb) {
                                continue;
                            }
                            if let Ok(rel) = path.strip_prefix(&root_cb) {
                                paths.push(rel.display().to_string());
                            }
                        }
                        if paths.is_empty() {
                            continue;
                        }
                        let rev = {
                            let mut r = revision_cb.lock();
                            *r += 1;
                            *r
                        };
                        let _ = reload_tx_task.send(ReloadNotification { revision: rev, paths });
                    }
                    Err(e) => {
                        eprintln!("preview watcher error: {e}");
                    }
                }
            }
        });

        Ok(Self {
            _debouncer: debouncer,
            revision,
        })
    }

    pub fn current_revision(&self) -> u64 {
        *self.revision.lock()
    }
}

#[derive(Debug, Clone)]
pub struct ReloadNotification {
    pub revision: u64,
    pub paths: Vec<String>,
}
