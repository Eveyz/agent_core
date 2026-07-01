import { RefObject } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { LazyEntry } from '../chat/LazyEntry';

interface ChatAreaProps {
  entryIds: string[];
  defaultModel: string;
  isProcessing: boolean;
  scrollRef: RefObject<HTMLDivElement | null>;
  contentRef: RefObject<HTMLDivElement | null>;
  isAtBottom: boolean;
  scrollToBottom: (behavior?: ScrollBehavior) => void;
  handleRetry: (id: string, text?: string) => void;
  onSend?: (msg: string) => void;
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
  onSend,
}: ChatAreaProps) {
  return (
    <>
      <div className="chat-history" ref={scrollRef}>
        <div ref={contentRef} style={{ display: 'flex', flexDirection: 'column', gap: '24px' }}>
          {entryIds.map((id, index) => (
            <LazyEntry
              key={id}
              entryId={id}
              defaultModel={defaultModel}
              handleRetry={handleRetry}
              isProcessing={isProcessing}
              scrollRef={scrollRef}
              forceVisible={index >= entryIds.length - 3}
              onSend={onSend}
            />
          ))}
        </div>
      </div>
      {!isAtBottom && (
        <button className="scroll-to-bottom-btn" onClick={() => scrollToBottom('auto')} title="Scroll to latest">
          <ChevronDownIcon size={18} />
        </button>
      )}
    </>
  );
}
