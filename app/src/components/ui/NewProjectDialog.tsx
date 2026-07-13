import { memo, useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

export interface NewProjectDialogProps {
  open: boolean;
  onClose: () => void;
  onCreate: (name: string, path: string) => void;
  creating?: boolean;
}

function joinPath(parent: string, name: string): string {
  const sep = parent.includes("\\") ? "\\" : "/";
  const trimmed = parent.replace(/[\\/]+$/, "");
  return `${trimmed}${sep}${name}`;
}

function sanitizeFolderName(name: string): string {
  const trimmed = name.trim();
  if (!trimmed) return "untitled";
  const cleaned = trimmed
    .replace(/[/\\:*?"<>|]/g, "_")
    .replace(/[\u0000-\u001f]/g, "_")
    .replace(/^[.\s]+|[.\s]+$/g, "");
  return cleaned || "untitled";
}

export const NewProjectDialog = memo(function NewProjectDialog({
  open: isOpen,
  onClose,
  onCreate,
  creating = false,
}: NewProjectDialogProps) {
  const { t } = useTranslation();
  const nameRef = useRef<HTMLInputElement>(null);
  const [name, setName] = useState("");
  const [path, setPath] = useState("");
  const [documentsDir, setDocumentsDir] = useState("");
  const [pathTouched, setPathTouched] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!isOpen) return;
    setName("");
    setPath("");
    setPathTouched(false);
    setError(null);
    let cancelled = false;
    (async () => {
      try {
        const dir = await invoke<string>("get_documents_dir");
        if (!cancelled) {
          setDocumentsDir(dir);
          setPath(joinPath(dir, "untitled"));
        }
      } catch {
        if (!cancelled) setDocumentsDir("");
      }
      setTimeout(() => nameRef.current?.focus(), 50);
    })();
    return () => {
      cancelled = true;
    };
  }, [isOpen]);

  useEffect(() => {
    if (!isOpen || pathTouched) return;
    const folder = sanitizeFolderName(name || "untitled");
    if (documentsDir) {
      setPath(joinPath(documentsDir, folder));
    } else {
      invoke<string>("get_default_project_path", { name: name || "untitled" })
        .then(setPath)
        .catch(() => undefined);
    }
  }, [name, documentsDir, isOpen, pathTouched]);

  const handleBrowse = useCallback(async () => {
    const selected = await open({ directory: true, multiple: false });
    if (selected && typeof selected === "string") {
      const folder = sanitizeFolderName(name || "untitled");
      setPath(joinPath(selected, folder));
      setPathTouched(true);
    }
  }, [name]);

  const handleSubmit = useCallback(() => {
    const trimmedName = name.trim();
    const trimmedPath = path.trim();
    if (!trimmedName) {
      setError(t("sidebar.newProject.nameRequired"));
      return;
    }
    if (!trimmedPath) {
      setError(t("sidebar.newProject.pathRequired"));
      return;
    }
    setError(null);
    onCreate(trimmedName, trimmedPath);
  }, [name, path, onCreate, t]);

  useEffect(() => {
    if (!isOpen) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    document.addEventListener("keydown", handler);
    return () => document.removeEventListener("keydown", handler);
  }, [isOpen, onClose]);

  if (!isOpen) return null;

  return (
    <div className="dialog-overlay" onClick={onClose}>
      <div className="dialog-content new-project-dialog" onClick={(e) => e.stopPropagation()}>
        <h3 className="dialog-title">{t("sidebar.newProject.title")}</h3>
        <p className="dialog-message">{t("sidebar.newProject.message")}</p>

        <label className="new-project-label" htmlFor="new-project-name">
          {t("sidebar.newProject.nameLabel")}
        </label>
        <input
          id="new-project-name"
          ref={nameRef}
          className="dialog-input"
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") handleSubmit();
          }}
          placeholder={t("sidebar.newProject.namePlaceholder")}
          disabled={creating}
        />

        <label className="new-project-label" htmlFor="new-project-path">
          {t("sidebar.newProject.pathLabel")}
        </label>
        <div className="new-project-path-row">
          <input
            id="new-project-path"
            className="dialog-input"
            value={path}
            onChange={(e) => {
              setPath(e.target.value);
              setPathTouched(true);
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") handleSubmit();
            }}
            disabled={creating}
          />
          <button
            type="button"
            className="btn-cancel new-project-browse"
            onClick={handleBrowse}
            disabled={creating}
          >
            {t("sidebar.newProject.browse")}
          </button>
        </div>

        {error && <div className="new-project-error">{error}</div>}

        <div className="dialog-actions">
          <button className="btn-cancel" onClick={onClose} disabled={creating}>
            {t("sidebar.actions.cancel")}
          </button>
          <button
            className="btn-confirm btn-allow"
            onClick={handleSubmit}
            disabled={creating}
          >
            {creating ? t("sidebar.newProject.creating") : t("sidebar.newProject.create")}
          </button>
        </div>
      </div>
    </div>
  );
});
