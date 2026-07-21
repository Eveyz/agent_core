import { memo } from 'react';
import { useSelector } from 'react-redux';
import type { RootState } from '../../store';
import { selectEntryById } from '../../features/chat/chatSlice';
import { UserRow } from './UserRow';
import { SteerRow } from './SteerRow';
import { AgentTurnUI } from './AgentTurn';

interface EntryRowProps {
  entryId: string;
  defaultModel: string;
  handleRetry: (id: string, text?: string) => void;
  isProcessing: boolean;
  onSend?: (msg: string | { text: string }) => void;
}

export const EntryRow = memo(function EntryRow({
  entryId,
  defaultModel,
  handleRetry,
  isProcessing,
  onSend,
}: EntryRowProps) {
  const entry = useSelector((state: RootState) => selectEntryById(state, entryId));
  if (!entry) return null;

  if (entry.type === 'user') {
    if (entry.isSteer) {
      return <SteerRow entry={entry} />;
    }
    return <UserRow entry={entry} modelName={defaultModel} onRetry={handleRetry} isProcessing={isProcessing} />;
  } else {
    return (
      <div className="message-row agent-row">
        <AgentTurnUI entry={entry} onSend={onSend} />
      </div>
    );
  }
});
