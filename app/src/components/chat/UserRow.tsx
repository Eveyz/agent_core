import { memo, useState } from 'react';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import Edit2Icon from 'lucide-react/dist/esm/icons/edit-2.mjs';
import RotateCwIcon from 'lucide-react/dist/esm/icons/rotate-cw.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import type { ChatEntry } from '../../features/chat/chatSlice';

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

  const startEdit = () => {
    setEditText(entry.text ?? '');
    setEditing(true);
  };

  const cancelEdit = () => {
    setEditing(false);
  };

  const confirmEdit = () => {
    const trimmed = editText.trim();
    if (!trimmed) return;
    setEditing(false);
    onRetry?.(entry.id, trimmed);
  };

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(entry.text ?? '');
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    } catch {}
  };

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

  return (
    <div className="message-row user-row">
      <div style={flexColumnEnd}>
        <div className="user-msg">{entry.text}</div>
        <div style={flexRowMeta}>
          <span>{modelName || '—'}</span>
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
