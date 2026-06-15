import { memo } from 'react';
import type { ChatEntry } from '../../features/chat/chatSlice';
import { AgentTurnUI } from './AgentTurn';

export const AgentRow = memo(function AgentRow({ entry }: { entry: ChatEntry }) {
  return (
    <div className="message-row agent-row">
      <AgentTurnUI entry={entry} />
    </div>
  );
});
