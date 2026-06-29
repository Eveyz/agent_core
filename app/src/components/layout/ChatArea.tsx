import { RefObject } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import TodoPanel from '../chat/TodoPanel';
import { EntryRow } from '../chat/EntryRow';

interface ChatAreaProps {
  entryIds: string[];
  defaultModel: string;
  isProcessing: boolean;
  scrollRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  isAtBottom: boolean;
  scrollToBottom: () => void;
  handleRetry: (id: string, text?: string) => void;
}

export function ChatArea({
  entryIds,
  defaultModel,
  isProcessing,
  scrollRef,
  contentRef,
  isAtBottom,
  scrollToBottom,
  handleRetry,
}: ChatAreaProps) {
  return (
    <>
      <TodoPanel />
      <div className="chat-history" ref={scrollRef}>
        <div ref={contentRef} style={{ display: 'flex', flexDirection: 'column', gap: '24px' }}>
          {entryIds.map((id) => (
            <EntryRow
              key={id}
              entryId={id}
              defaultModel={defaultModel}
              handleRetry={handleRetry}
              isProcessing={isProcessing}
            />
          ))}
        </div>
      </div>
      {!isAtBottom && (
        <button className="scroll-to-bottom-btn" onClick={scrollToBottom} title="Scroll to latest">
          <ChevronDownIcon size={18} />
        </button>
      )}
    </>
  );
}
