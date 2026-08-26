//! Preview session registry and lifecycle management.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use parking_lot::RwLock;
use tauri::webview::WebviewWindowBuilder;
use tauri::{AppHandle, Emitter, Manager, WebviewUrl};
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

use super::gateway::PreviewGateway;
use super::path_policy::normalize_entrypoint;
use super::process::{detect_framework, FrameworkProcess};
use super::types::*;
use super::watcher::{PreviewWatcher, ReloadNotification};

const MAIN_WEBVIEW_LABEL: &str = "main";

pub struct PreviewManager {
    sessions: RwLock<HashMap<Uuid, Arc<PreviewSession>>>,
    quotas: PreviewQuotas,
    app_handle: Mutex<Option<AppHandle>>,
}

struct PreviewSession {
    descriptor: RwLock<PreviewDescriptor>,
    gateway: Mutex<Option<PreviewGateway>>,
    watcher: Mutex<Option<PreviewWatcher>>,
    framework: Mutex<Option<Arc<FrameworkProcess>>>,
    reload_tx: broadcast::Sender<ReloadNotification>,
    workspace_root: PathBuf,
    cancel_reload: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
}

impl PreviewManager {
    pub fn new(quotas: PreviewQuotas) -> Self {
        Self {
            sessions: RwLock::new(HashMap::new()),
            quotas,
            app_handle: Mutex::new(None),
        }
    }

    pub async fn set_app_handle(&self, handle: AppHandle) {
        *self.app_handle.lock().await = Some(handle);
    }

    pub fn list(&self, workspace_id: &str) -> Vec<PreviewDescriptor> {
        self.sessions
            .read()
            .values()
            .filter(|s| s.descriptor.read().workspace_id == workspace_id)
            .map(|s| s.descriptor.read().clone())
            .collect()
    }

    pub fn get(&self, preview_id: Uuid) -> Option<PreviewDescriptor> {
        self.sessions
            .read()
            .get(&preview_id)
            .map(|s| s.descriptor.read().clone())
    }

    /// Find an existing preview for the same workspace (and optional session).
    pub fn find_active(&self, workspace_id: &str, session_id: Option<&str>) -> Option<Uuid> {
        self.sessions.read().values().find_map(|s| {
            let d = s.descriptor.read();
            if d.workspace_id != workspace_id {
                return None;
            }
            if let Some(sid) = session_id {
                if d.session_id.as_deref() != Some(sid) {
                    return None;
                }
            }
            Some(d.id)
        })
    }

    pub async fn start(
        &self,
        workspace_root: PathBuf,
        request: PreviewStartRequest,
    ) -> Result<PreviewDescriptor> {
        self.enforce_quotas(&request.workspace_id)?;

        let canonical_root = workspace_root
            .canonicalize()
            .with_context(|| format!("invalid workspace: {}", workspace_root.display()))?;

        let mut request = request;
        request.entrypoint = Some(match request.entrypoint.as_deref() {
            Some(raw) => normalize_entrypoint(&canonical_root, raw)
                .map_err(|e| anyhow!("invalid entrypoint: {e}"))?,
            None => normalize_entrypoint(&canonical_root, "index.html")
                .map_err(|e| anyhow!("invalid entrypoint: {e}"))?,
        });

        let preview_id = Uuid::new_v4();
        let placement = request.placement.unwrap_or(PreviewPlacement::Split);
        let (reload_tx, _) = broadcast::channel(64);

        let mut proxy_target = None;
        let mut framework_proc = None;

        if request.mode == PreviewMode::Framework {
            let cmd = request
                .approved_command
                .as_ref()
                .ok_or_else(|| anyhow!("framework preview requires approved_command"))?;
            validate_framework_command(cmd)?;

            let child_listener =
                tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
            let child_port = child_listener.local_addr()?.port();
            drop(child_listener);

            let proc = FrameworkProcess::spawn(
                &canonical_root,
                &cmd.program,
                &cmd.args,
                child_port,
                self.quotas.max_log_bytes,
            )
            .await?;
            let health = format!("http://127.0.0.1:{child_port}/");
            proc.wait_ready(&health).await?;
            proxy_target = Some(format!("http://127.0.0.1:{child_port}"));
            framework_proc = Some(Arc::new(proc));
        }

        let gateway = PreviewGateway::start(
            canonical_root.clone(),
            request.mode,
            request.entrypoint.clone(),
            reload_tx.clone(),
            proxy_target,
        )
        .await?;

        let descriptor = PreviewDescriptor {
            id: preview_id,
            workspace_id: request.workspace_id.clone(),
            session_id: request.session_id.clone(),
            mode: request.mode,
            url: gateway.url.clone(),
            status: PreviewStatus::Starting,
            revision: 0,
            placement,
            entrypoint: request.entrypoint.clone(),
        };

        let session = Arc::new(PreviewSession {
            descriptor: RwLock::new(descriptor.clone()),
            gateway: Mutex::new(Some(gateway)),
            watcher: Mutex::new(None),
            framework: Mutex::new(framework_proc),
            reload_tx: reload_tx.clone(),
            workspace_root: canonical_root.clone(),
            cancel_reload: Mutex::new(None),
        });

        if request.mode == PreviewMode::Static {
            let watcher = PreviewWatcher::start(canonical_root, reload_tx)?;
            *session.watcher.lock().await = Some(watcher);
        }

        self.start_reload_forwarder(&session).await;

        {
            let mut d = session.descriptor.write();
            d.status = PreviewStatus::Ready;
        }

        self.sessions.write().insert(preview_id, session.clone());
        self.emit_event(PreviewEvent::status(preview_id, PreviewStatus::Ready))
            .await;

        let out = session.descriptor.read().clone();
        Ok(out)
    }

    async fn start_reload_forwarder(&self, session: &Arc<PreviewSession>) {
        let preview_id = session.descriptor.read().id;
        let mut rx = session.reload_tx.subscribe();
        let handle_opt = self.app_handle.lock().await.clone();

        let task = tauri::async_runtime::spawn(async move {
            while let Ok(note) = rx.recv().await {
                if let Some(ref handle) = handle_opt {
                    let _ = handle.emit_to(
                        MAIN_WEBVIEW_LABEL,
                        "preview-event",
                        PreviewEvent::reload(preview_id, note.revision, note.paths),
                    );
                }
            }
        });
        *session.cancel_reload.lock().await = Some(task);
    }

    pub async fn stop(&self, preview_id: Uuid) -> Result<()> {
        let session = self
            .sessions
            .write()
            .remove(&preview_id)
            .ok_or_else(|| anyhow!("preview not found"))?;
        self.stop_session(session).await
    }

    async fn stop_session(&self, session: Arc<PreviewSession>) -> Result<()> {
        let preview_id = session.descriptor.read().id;
        {
            let mut d = session.descriptor.write();
            d.status = PreviewStatus::Stopping;
        }
        self.emit_event(PreviewEvent::status(preview_id, PreviewStatus::Stopping))
            .await;

        if let Some(task) = session.cancel_reload.lock().await.take() {
            task.abort();
        }

        if let Some(proc) = session.framework.lock().await.take() {
            proc.kill().await;
        }

        if let Some(gw) = session.gateway.lock().await.take() {
            gw.shutdown().await;
        }

        session.watcher.lock().await.take();
        self.close_popout(preview_id).await?;

        {
            let mut d = session.descriptor.write();
            d.status = PreviewStatus::Stopped;
        }
        self.emit_event(PreviewEvent::status(preview_id, PreviewStatus::Stopped))
            .await;
        Ok(())
    }

    pub async fn restart(&self, preview_id: Uuid) -> Result<PreviewDescriptor> {
        let existing = self
            .get(preview_id)
            .ok_or_else(|| anyhow!("preview not found"))?;
        let request = PreviewStartRequest {
            workspace_id: existing.workspace_id,
            session_id: existing.session_id,
            mode: existing.mode,
            entrypoint: existing.entrypoint,
            approved_command: None,
            placement: Some(existing.placement),
        };
        let root = self
            .sessions
            .read()
            .get(&preview_id)
            .map(|s| s.workspace_root.clone())
            .ok_or_else(|| anyhow!("preview not found"))?;
        self.stop(preview_id).await?;
        self.start(root, request).await
    }

    pub async fn set_visibility(
        &self,
        preview_id: Uuid,
        placement: PreviewPlacement,
    ) -> Result<PreviewDescriptor> {
        let session = self
            .sessions
            .read()
            .get(&preview_id)
            .cloned()
            .ok_or_else(|| anyhow!("preview not found"))?;
        {
            let mut d = session.descriptor.write();
            d.placement = placement;
        }
        if placement == PreviewPlacement::Popout {
            self.open_popout(preview_id).await?;
        } else {
            self.close_popout(preview_id).await?;
        }
        let out = session.descriptor.read().clone();
        Ok(out)
    }

    pub async fn open_popout(&self, preview_id: Uuid) -> Result<()> {
        let session = self
            .sessions
            .read()
            .get(&preview_id)
            .cloned()
            .ok_or_else(|| anyhow!("preview not found"))?;
        let url = session.descriptor.read().url.clone();
        let label = format!("preview-{preview_id}-window");

        let handle = self
            .app_handle
            .lock()
            .await
            .clone()
            .ok_or_else(|| anyhow!("app handle not ready"))?;

        if handle.get_webview_window(&label).is_some() {
            return Ok(());
        }

        WebviewWindowBuilder::new(&handle, &label, WebviewUrl::External(url.parse()?))
            .title("Preview")
            .inner_size(1024.0, 768.0)
            .build()
            .map_err(|e| anyhow!("failed to open popout: {e}"))?;

        Ok(())
    }

    pub async fn close_popout(&self, preview_id: Uuid) -> Result<()> {
        let label = format!("preview-{preview_id}-window");
        if let Some(handle) = self.app_handle.lock().await.clone() {
            if let Some(win) = handle.get_webview_window(&label) {
                let _ = win.close();
            }
        }
        Ok(())
    }

    pub fn logs(&self, preview_id: Uuid, cursor: u64) -> Result<LogPage> {
        let session = {
            let sessions = self.sessions.read();
            sessions
                .get(&preview_id)
                .cloned()
                .ok_or_else(|| anyhow!("preview not found"))?
        };
        if let Some(proc) = session.framework.blocking_lock().as_ref() {
            return Ok(proc.logs.lock().page(cursor));
        }
        Ok(LogPage {
            lines: vec![],
            next_cursor: cursor,
        })
    }

    pub async fn detect_framework(&self, workspace_root: &PathBuf) -> Result<FrameworkDetection> {
        let root = workspace_root
            .canonicalize()
            .with_context(|| format!("invalid workspace: {}", workspace_root.display()))?;
        Ok(detect_framework(&root))
    }

    pub async fn shutdown_all(&self) {
        let ids: Vec<Uuid> = self.sessions.read().keys().cloned().collect();
        for id in ids {
            if let Some(session) = self.sessions.write().remove(&id) {
                let _ = self.stop_session(session).await;
            }
        }
    }

    fn enforce_quotas(&self, workspace_id: &str) -> Result<()> {
        let sessions = self.sessions.read();
        if sessions.len() >= self.quotas.max_global {
            return Err(anyhow!("global preview quota exceeded"));
        }
        let ws_count = sessions
            .values()
            .filter(|s| s.descriptor.read().workspace_id == workspace_id)
            .count();
        if ws_count >= self.quotas.max_per_workspace {
            return Err(anyhow!("workspace preview quota exceeded"));
        }
        Ok(())
    }

    async fn emit_event(&self, event: PreviewEvent) {
        if let Some(handle) = self.app_handle.lock().await.clone() {
            let _ = handle.emit_to(MAIN_WEBVIEW_LABEL, "preview-event", event);
        }
    }
}

fn validate_framework_command(cmd: &FrameworkCommandRequest) -> Result<()> {
    if cmd.program.is_empty() {
        return Err(anyhow!("empty program"));
    }
    if cmd.program.contains('/') || cmd.program.contains('\\') {
        return Err(anyhow!("program must be a bare executable name"));
    }
    for arg in &cmd.args {
        if arg.starts_with('-') && arg.contains("..") {
            return Err(anyhow!("suspicious argument"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_shell_in_program() {
        let cmd = FrameworkCommandRequest {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), "evil".into()],
        };
        assert!(validate_framework_command(&cmd).is_err());
    }

    #[test]
    fn default_quotas() {
        let q = PreviewQuotas::default();
        assert!(q.max_global >= q.max_per_workspace);
    }

    #[test]
    fn workspace_quota_allows_first_preview() {
        let mgr = PreviewManager::new(PreviewQuotas {
            max_per_workspace: 1,
            max_global: 4,
            max_log_bytes: 1024,
        });
        assert!(mgr.enforce_quotas("ws1").is_ok());
    }
}
