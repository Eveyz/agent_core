import { memo, useMemo } from 'react';
import type { SubagentEntry } from '../../features/chat/chatSlice';
import { convertSubagentBlocks } from '../../utils/chatUtils';
import { UserRow } from './UserRow';
import { AgentTurnUI } from './AgentTurn';
import { useAutoScroll } from '../../hooks/useAutoScroll';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';

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
  const syntheticEntry = useMemo(() => ({
    id: `subagent-detail-${subagent.id}`,
    type: 'turn' as const,
    blocks: convertSubagentBlocks(subagent.blocks),
    startTime: subagent.startTime,
    endTime: subagent.endTime,
  }), [subagent.id, subagent.blocks, subagent.startTime, subagent.endTime]);

  const { scrollRef, contentRef, scrollToBottom, isAtBottom } = useAutoScroll<HTMLDivElement, HTMLDivElement>({
    deps: [subagent.blocks.length],
    isProcessing,
  });

  return (
    <div style={{ position: 'relative', display: 'flex', flexDirection: 'column', height: '100%', width: '100%' }}>
      <div className="chat-history" ref={scrollRef} style={{ flex: 1, overflowY: 'auto' }}>
        <div ref={contentRef} style={{ display: 'flex', flexDirection: 'column', gap: '24px' }}>
          <UserRow
            entry={{ id: `${subagent.id}-task`, type: 'user', text: taskText }}
            modelName={defaultModel}
            isProcessing={isProcessing}
            hideActions={true}
          />
          <div className="message-row agent-row">
            <AgentTurnUI entry={syntheticEntry} />
          </div>
        </div>
      </div>
      {!isAtBottom && (
        <button 
          className="scroll-to-bottom-btn" 
          onClick={() => scrollToBottom('auto')} 
          title="Scroll to latest"
          style={{ bottom: '24px' }}
        >
          <ChevronDownIcon size={18} />
        </button>
      )}
    </div>
  );
});
