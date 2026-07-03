import { RefObject } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
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
  const btwEntries = useSelector((state: RootState) => state.chat.btwEntries);
  const learnEntries = useSelector((state: RootState) => state.chat.learnEntries);
  const goal = useSelector((state: RootState) => state.chat.goal);
  const goalCompleted = useSelector((state: RootState) => state.chat.goalCompleted);

  return (
    <>
      <div className="chat-history" ref={scrollRef}>
        <div ref={contentRef} style={{ display: 'flex', flexDirection: 'column', gap: '24px' }}>
          {goal && (
            <div style={{
              padding: '8px 12px',
              borderRadius: 8,
              border: '1px solid var(--border-color, #333)',
              background: goalCompleted ? 'rgba(34,197,94,0.12)' : 'rgba(99,102,241,0.12)',
              fontSize: 13,
            }}>
              🎯 <strong>Goal:</strong> {goal} {goalCompleted && <span style={{ color: '#22c55e' }}>✓ completed</span>}
            </div>
          )}
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
          {learnEntries.map((e) => (
            <div key={e.id} style={{
              alignSelf: 'center',
              maxWidth: '80%',
              padding: '8px 12px',
              borderRadius: 8,
              border: '1px solid var(--border-color, #333)',
              background: 'var(--bg-secondary, #1a1a1a)',
              fontSize: 13,
            }}>
              💡 {e.status === 'pending'
                ? <span style={{ opacity: 0.7 }}>Saving learning…</span>
                : e.status === 'saved'
                  ? <><strong>Learned:</strong> {e.title}<br /><span style={{ opacity: 0.7 }}>{e.rule}</span></>
                  : <span style={{ color: '#ef4444' }}>⚠ {e.error}</span>}
            </div>
          ))}
          {btwEntries.map((e) => (
            <div key={e.id} style={{
              alignSelf: 'center',
              maxWidth: '80%',
              padding: '8px 12px',
              borderRadius: 8,
              border: '1px dashed var(--border-color, #444)',
              background: 'var(--bg-secondary, #1a1a1a)',
              fontSize: 13,
            }}>
              <div style={{ fontSize: 11, opacity: 0.6, marginBottom: 4 }}>BTW · {e.question}</div>
              <div style={{ whiteSpace: 'pre-wrap' }}>{e.answer}{e.isStreaming && '▌'}</div>
            </div>
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
