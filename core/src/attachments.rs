//! Prompt-scoped, content-addressable image attachments.
//!
//! Images are stored under
//! `~/.agverse/sessions/<session_id>/<prompt_id>/images/<sha256>.<ext>`
//! and referenced from SQLite message metadata via a stable
//! `agverse://sessions/<session_id>/<prompt_id>/images/<sha256>.<ext>` URL.

use crate::types::ImageAttachment;
use anyhow::{Context, Result, bail};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

const AGVERSE_SCHEME: &str = "agverse://sessions/";

/// Build the stable URI stored in session message metadata.
pub fn attachment_url(session_id: &str, prompt_id: &str, filename: &str) -> String {
    format!("agverse://sessions/{session_id}/{prompt_id}/images/{filename}")
}

/// Resolve an `agverse://sessions/...` URL or absolute path to a filesystem path.
pub fn resolve_attachment_ref(reference: &str) -> Result<PathBuf> {
    let trimmed = reference.trim();
    if let Some(rest) = trimmed.strip_prefix(AGVERSE_SCHEME) {
        // rest = "{session_id}/{prompt_id}/images/{filename}"
        let path = crate::paths::get_agverse_dir().join("sessions").join(rest);
        return Ok(path);
    }
    let path = PathBuf::from(trimmed);
    if path.is_absolute() {
        return Ok(path);
    }
    bail!("unsupported attachment reference: {trimmed}");
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/bmp" => "bmp",
        _ => "png",
    }
}

fn ext_to_mime(ext: &str) -> &'static str {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "bmp" => "image/bmp",
        _ => "image/png",
    }
}

/// Soft cap to avoid huge clipboard pastes (~12MB).
pub const MAX_ATTACHMENT_BYTES: usize = 12 * 1024 * 1024;

/// Persist image bytes under the prompt images dir using a SHA-256 filename.
/// Identical content reuses the existing file (content-addressable dedup).
pub fn save_session_image(
    session_id: &str,
    prompt_id: &str,
    bytes: &[u8],
    mime_type: &str,
) -> Result<ImageAttachment> {
    if bytes.is_empty() {
        bail!("empty image");
    }
    if bytes.len() > MAX_ATTACHMENT_BYTES {
        bail!("image too large (max 12MB)");
    }

    let mime = if mime_type.starts_with("image/") {
        mime_type.to_string()
    } else {
        "image/png".to_string()
    };

    let digest = Sha256::digest(bytes);
    let sha256 = hex::encode(digest);
    let filename = format!("{sha256}.{}", mime_to_ext(&mime));
    let dir = crate::paths::prompt_images_dir(session_id, prompt_id);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create images dir {}", dir.display()))?;
    let path = dir.join(&filename);
    if !path.exists() {
        std::fs::write(&path, bytes)
            .with_context(|| format!("write attachment {}", path.display()))?;
    }

    Ok(ImageAttachment {
        path: path.to_string_lossy().to_string(),
        mime_type: mime,
        sha256: Some(sha256),
        url: Some(attachment_url(session_id, prompt_id, &filename)),
    })
}

/// Reuse an existing attachment path/URL already under a prompt images tree.
///
/// On retry, files may live under a previous prompt folder; we accept any path
/// under `sessions/<session_id>/` and re-emit a URL for the *current* prompt,
/// copying into the current prompt's `images/` when needed.
pub fn reuse_session_image(
    session_id: &str,
    prompt_id: &str,
    reference: &str,
) -> Result<ImageAttachment> {
    let path = resolve_attachment_ref(reference)?;
    ensure_under_session(session_id, &path)?;
    if !path.is_file() {
        bail!("attachment not found: {}", path.display());
    }
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("invalid attachment filename"))?
        .to_string();
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("png");
    let mime = ext_to_mime(ext);
    let sha256 = filename
        .split_once('.')
        .map(|(hash, _)| hash.to_string())
        .filter(|h| h.len() == 64 && h.chars().all(|c| c.is_ascii_hexdigit()));

    let dest_dir = crate::paths::prompt_images_dir(session_id, prompt_id);
    std::fs::create_dir_all(&dest_dir)
        .with_context(|| format!("create images dir {}", dest_dir.display()))?;
    let dest = dest_dir.join(&filename);
    if path != dest && !dest.exists() {
        std::fs::copy(&path, &dest)
            .with_context(|| format!("copy attachment into prompt images {}", dest.display()))?;
    }
    let final_path = if dest.exists() { dest } else { path };

    Ok(ImageAttachment {
        path: final_path.to_string_lossy().to_string(),
        mime_type: mime.to_string(),
        sha256,
        url: Some(attachment_url(session_id, prompt_id, &filename)),
    })
}

fn ensure_under_session(session_id: &str, path: &Path) -> Result<()> {
    let allowed = crate::paths::session_dir(session_id)
        .canonicalize()
        .unwrap_or_else(|_| crate::paths::session_dir(session_id));
    let canonical = path
        .canonicalize()
        .with_context(|| format!("canonicalize {}", path.display()))?;
    if !canonical.starts_with(&allowed) {
        bail!("attachment path not under session dir");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_url_roundtrip_shape() {
        let url = attachment_url("sess-1", "prompt-1", "abcd.png");
        assert_eq!(url, "agverse://sessions/sess-1/prompt-1/images/abcd.png");
        let resolved = resolve_attachment_ref(&url).unwrap();
        assert!(
            resolved.ends_with("sessions/sess-1/prompt-1/images/abcd.png")
                || resolved.ends_with("sessions\\sess-1\\prompt-1\\images\\abcd.png")
        );
    }
}
