import { memo } from 'react';
import { useSelector } from 'react-redux';
import type { RootState } from '../../store';
import { selectEntryById } from '../../features/chat/chatSlice';
import { UserRow } from './UserRow';
import { AgentTurnUI } from './AgentTurn';

interface EntryRowProps {
  entryId: string;
  defaultModel: string;
  handleRetry: (id: string, text?: string) => void;
  isProcessing: boolean;
}

export const EntryRow = memo(function EntryRow({
  entryId,
  defaultModel,
  handleRetry,
  isProcessing,
}: EntryRowProps) {
  const entry = useSelector((state: RootState) => selectEntryById(state, entryId));
  if (!entry) return null;

  if (entry.type === 'user') {
    return <UserRow entry={entry} modelName={defaultModel} onRetry={handleRetry} isProcessing={isProcessing} />;
  } else {
    return (
      <div className="message-row agent-row">
        <AgentTurnUI entry={entry} />
      </div>
    );
  }
});
