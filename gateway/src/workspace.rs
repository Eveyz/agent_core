//! Workspace provisioning: host cwd or git clone.

use crate::error::ApiError;
use crate::models::{EnvInput, RepoInput};
use crate::store::{WorkspaceRecord, workspaces_dir};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};
use tokio::process::Command;

#[derive(Debug, Clone)]
pub struct ProvisionedWorkspace {
    pub record: WorkspaceRecord,
    pub repos: Vec<RepoInput>,
}

/// Resolve and prepare a workspace for a new agent.
///
/// Rules:
/// - If `repos` is non-empty → `git` workspace (clone first repo).
/// - Else if `env.type == "git"` → require `repos`.
/// - Else → `host` workspace using `env.cwd` (required).
pub async fn provision(
    agent_id: &str,
    env: Option<&EnvInput>,
    repos: &[RepoInput],
) -> Result<ProvisionedWorkspace, ApiError> {
    let env_type = env
        .map(|e| e.env_type.as_str())
        .unwrap_or(if repos.is_empty() { "host" } else { "git" });

    match env_type {
        "git" => provision_git(agent_id, env, repos).await,
        "host" => provision_host(env),
        other => Err(ApiError::BadRequest(format!(
            "unsupported env.type '{other}' (expected host|git)"
        ))),
    }
}

fn provision_host(env: Option<&EnvInput>) -> Result<ProvisionedWorkspace, ApiError> {
    let cwd = env
        .and_then(|e| e.cwd.as_deref())
        .ok_or_else(|| ApiError::BadRequest("env.cwd is required for host workspaces".into()))?;

    let path = PathBuf::from(cwd);
    if !path.is_absolute() {
        return Err(ApiError::BadRequest(
            "env.cwd must be an absolute path".into(),
        ));
    }
    if !path.is_dir() {
        return Err(ApiError::BadRequest(format!(
            "env.cwd does not exist or is not a directory: {}",
            path.display()
        )));
    }

    // Ensure artifacts/ exists for the artifacts API.
    let artifacts = path.join("artifacts");
    if let Err(err) = std::fs::create_dir_all(&artifacts) {
        return Err(ApiError::Internal(format!(
            "failed to create artifacts dir: {err}"
        )));
    }

    Ok(ProvisionedWorkspace {
        record: WorkspaceRecord {
            workspace_type: "host".into(),
            path: path.to_string_lossy().to_string(),
        },
        repos: Vec::new(),
    })
}

async fn provision_git(
    agent_id: &str,
    env: Option<&EnvInput>,
    repos: &[RepoInput],
) -> Result<ProvisionedWorkspace, ApiError> {
    let repo = repos
        .first()
        .ok_or_else(|| ApiError::BadRequest("repos[0] is required for git workspaces".into()))?;

    let dest = if let Some(cwd) = env.and_then(|e| e.cwd.as_ref()) {
        let p = PathBuf::from(cwd);
        if !p.is_absolute() {
            return Err(ApiError::BadRequest(
                "env.cwd must be an absolute path".into(),
            ));
        }
        p
    } else {
        workspaces_dir().join(agent_id)
    };

    if dest.exists() {
        if dest.join(".git").exists() {
            // Reuse existing clone.
        } else if dest.is_dir()
            && std::fs::read_dir(&dest)
                .map(|mut d| d.next().is_none())
                .unwrap_or(false)
        {
            // empty dir ok
        } else {
            return Err(ApiError::Conflict(format!(
                "workspace path already exists and is not an empty/git dir: {}",
                dest.display()
            )));
        }
    } else {
        std::fs::create_dir_all(dest.parent().unwrap_or(Path::new("/")))
            .map_err(|e| ApiError::Internal(format!("mkdir {}: {e}", dest.display())))?;
    }

    if !dest.join(".git").exists() {
        clone_repo(&repo.url, repo.starting_ref.as_deref(), &dest)
            .await
            .map_err(|e| ApiError::Internal(format!("git clone failed: {e}")))?;
    }

    let artifacts = dest.join("artifacts");
    let _ = std::fs::create_dir_all(&artifacts);

    Ok(ProvisionedWorkspace {
        record: WorkspaceRecord {
            workspace_type: "git".into(),
            path: dest.to_string_lossy().to_string(),
        },
        repos: repos.to_vec(),
    })
}

async fn clone_repo(url: &str, starting_ref: Option<&str>, dest: &Path) -> Result<()> {
    let mut args = vec!["clone".to_string(), "--depth".to_string(), "1".to_string()];
    if let Some(r) = starting_ref {
        args.push("--branch".to_string());
        args.push(r.to_string());
    }
    args.push(url.to_string());
    args.push(dest.to_string_lossy().to_string());

    let output = Command::new("git")
        .args(&args)
        .output()
        .await
        .context("spawn git")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("git clone exited {:?}: {stderr}", output.status.code());
    }
    Ok(())
}

/// Resolve a relative artifact path under `{workspace}/artifacts/` safely.
pub fn resolve_artifact_path(workspace: &Path, relative: &str) -> Result<PathBuf, ApiError> {
    if relative.is_empty()
        || relative.contains('\0')
        || Path::new(relative)
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ApiError::BadRequest(
            "invalid artifact path (must be relative, no ..)".into(),
        ));
    }
    if Path::new(relative).is_absolute() {
        return Err(ApiError::BadRequest(
            "artifact path must be relative to artifacts/".into(),
        ));
    }

    let root = workspace.join("artifacts");
    let full = root.join(relative);
    let root_canon = root
        .canonicalize()
        .map_err(|_| ApiError::NotFound("artifacts directory not found".into()))?;
    let full_canon = full
        .canonicalize()
        .map_err(|_| ApiError::NotFound(format!("artifact not found: {relative}")))?;
    if !full_canon.starts_with(&root_canon) {
        return Err(ApiError::BadRequest(
            "artifact path escapes workspace".into(),
        ));
    }
    if !full_canon.is_file() {
        return Err(ApiError::NotFound(format!(
            "artifact not found: {relative}"
        )));
    }
    Ok(full_canon)
}
