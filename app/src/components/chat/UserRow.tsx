import { memo, useState, useEffect, useCallback } from 'react';
import { invoke } from '@tauri-apps/api/core';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import Edit2Icon from 'lucide-react/dist/esm/icons/edit-2.mjs';
import RotateCwIcon from 'lucide-react/dist/esm/icons/rotate-cw.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import type { ChatEntry } from '../../features/chat/chatSlice';
import { ImageLightbox } from './ImageLightbox';

const flexColumnEnd = { display: 'flex', flexDirection: 'column' as const, alignItems: 'flex-end', gap: '6px' };
const flexRowMeta = { display: 'flex', gap: '12px', color: 'var(--text-tertiary)', fontSize: '11px', paddingRight: '4px' };
const cursorPointer = { cursor: 'pointer' as const };

export const UserRow = memo(function UserRow({ entry, modelName, onRetry, isProcessing, hideActions }: {
  entry: ChatEntry;
  modelName?: string;
  onRetry?: (entryId: string, editedText?: string) => void;
  isProcessing?: boolean;
  hideActions?: boolean;
}) {
  const [editing, setEditing] = useState(false);
  const [editText, setEditText] = useState('');
  const [copied, setCopied] = useState(false);
  const [lightboxSrc, setLightboxSrc] = useState<string | null>(null);
  const [resolvedPreviews, setResolvedPreviews] = useState<Record<string, string>>({});

  useEffect(() => {
    let cancelled = false;
    const resolve = async () => {
      const next: Record<string, string> = {};
      for (const img of entry.images ?? []) {
        if (img.previewUrl) {
          next[img.id] = img.previewUrl;
          continue;
        }
        const reference = img.url || img.path;
        if (!reference) continue;
        try {
          const dataUrl = await invoke<string>('read_attachment_data_url', { path: reference });
          if (!cancelled) next[img.id] = dataUrl;
        } catch {
          // ignore missing attachments
        }
      }
      if (!cancelled) setResolvedPreviews(next);
    };
    void resolve();
    return () => { cancelled = true; };
  }, [entry.images]);

  const confirmEdit = () => {
    const trimmed = editText.trim();
    const hasImages = (entry.images?.length ?? 0) > 0;
    if (!trimmed && !hasImages) return;
    setEditing(false);
    onRetry?.(entry.id, trimmed);
  };

  const startEdit = () => {
    setEditText(entry.text ?? '');
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
  };

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(entry.text ?? '');
      setCopied(true);
    } catch {}
  }, [entry.text]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  if (editing) {
    return (
      <div className="message-row user-row user-row-editing">
        <textarea
          className="user-msg-edit"
          value={editText}
          onChange={(e) => setEditText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); confirmEdit(); }
            if (e.key === 'Escape') cancelEdit();
          }}
          autoFocus
        />
        <div style={{ ...flexRowMeta, justifyContent: 'flex-end' }}>
          <span style={cursorPointer} onClick={confirmEdit}>
            <CheckIcon size={12} /> Send
          </span>
          <span style={cursorPointer} onClick={cancelEdit}>
            <XIcon size={12} /> Cancel
          </span>
        </div>
      </div>
    );
  }

  const images = entry.images ?? [];

  return (
    <div className="message-row user-row">
      <ImageLightbox src={lightboxSrc} onClose={() => setLightboxSrc(null)} />
      <div style={flexColumnEnd}>
        {images.length > 0 && (
          <div className="user-msg-images">
            {images.map((img) => {
              const src = resolvedPreviews[img.id] || img.previewUrl;
              if (!src) return null;
              return (
                <button
                  key={img.id}
                  type="button"
                  className="user-msg-image-thumb"
                  onClick={() => setLightboxSrc(src)}
                  aria-label="View image"
                >
                  <img src={src} alt="" />
                </button>
              );
            })}
          </div>
        )}
        {entry.text?.trim() ? (
          <div className="user-msg">{entry.text.trimEnd()}</div>
        ) : null}
        <div style={flexRowMeta}>
          <span>{entry.model || modelName || '—'}</span>
          <span style={cursorPointer} onClick={handleCopy}>
            {copied ? '✓ Copied' : <><CopyIcon size={12} /></>}
          </span>
          {!hideActions && (
            <>
              <Edit2Icon size={12} style={cursorPointer} onClick={startEdit} />
              <RotateCwIcon
                size={12}
                style={{ ...cursorPointer, opacity: isProcessing ? 0.4 : 1 }}
                onClick={isProcessing ? undefined : () => onRetry?.(entry.id)}
              />
            </>
          )}
        </div>
      </div>
    </div>
  );
});
