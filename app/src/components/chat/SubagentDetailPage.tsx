import { memo } from 'react';
import type { SubagentEntry } from '../../features/chat/chatSlice';
import { convertSubagentBlocks } from '../../utils/chatUtils';
import { UserRow } from './UserRow';
import { AgentTurnUI } from './AgentTurn';

interface SubagentDetailPageProps {
  subagent: SubagentEntry;
  isProcessing: boolean;
  defaultModel: string;
}

export const SubagentDetailPage = memo(function SubagentDetailPage({
  subagent,
  isProcessing,
  defaultModel,
}: SubagentDetailPageProps) {
  const taskText = typeof subagent.task === 'string' ? subagent.task : JSON.stringify(subagent.task);
  const syntheticEntry = {
    id: `subagent-detail-${subagent.id}`,
    type: 'turn' as const,
    blocks: convertSubagentBlocks(subagent.blocks),
    startTime: subagent.startTime,
    endTime: subagent.endTime,
  };

  return (
    <div className="chat-history">
      <UserRow
        entry={{ id: `${subagent.id}-task`, type: 'user', text: taskText }}
        modelName={defaultModel}
        isProcessing={isProcessing}
      />
      <div className="message-row agent-row">
        <AgentTurnUI entry={syntheticEntry} />
      </div>
    </div>
  );
});
