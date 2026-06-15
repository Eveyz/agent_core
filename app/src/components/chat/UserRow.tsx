import { memo } from 'react';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import Edit2Icon from 'lucide-react/dist/esm/icons/edit-2.mjs';
import type { ChatEntry } from '../../features/chat/chatSlice';

const flexColumnEnd = { display: 'flex', flexDirection: 'column' as const, alignItems: 'flex-end', gap: '6px' };
const flexRowMeta = { display: 'flex', gap: '12px', color: '#555', fontSize: '11px', paddingRight: '4px' };
const cursorPointer = { cursor: 'pointer' as const };

export const UserRow = memo(function UserRow({ entry, modelName }: { entry: ChatEntry; modelName?: string }) {
  return (
    <div className="message-row user-row">
      <div style={flexColumnEnd}>
        <div className="user-msg">{entry.text}</div>
        <div style={flexRowMeta}>
          <span>{modelName || '—'}</span>
          <CopyIcon size={12} style={cursorPointer} />
          <Edit2Icon size={12} style={cursorPointer} />
        </div>
      </div>
    </div>
  );
});
