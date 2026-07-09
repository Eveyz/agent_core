import { RefObject, useState, memo } from 'react';
import { useSelector, shallowEqual } from 'react-redux';
import { RootState } from '../../store';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { LazyEntry } from '../chat/LazyEntry';
import { loadMorePrompts, selectActiveBtwEntries } from '../../features/chat/chatSlice';
import { useAppDispatch } from '../../hooks/useAppDispatch';

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

export const ChatArea = memo(function ChatArea({
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
  const dispatch = useAppDispatch();
  const activeSessionId = useSelector((state: RootState) => state.project.activeSessionId);
  const btwEntries = useSelector(selectActiveBtwEntries, shallowEqual);
  const goal = useSelector((state: RootState) => state.chat.goal[activeSessionId ?? '']);
  const goalCompleted = useSelector((state: RootState) => state.chat.goalCompleted[activeSessionId ?? '']);

  const allPrompts = useSelector((state: RootState) => state.chat.allPrompts[activeSessionId ?? '']);
  const visiblePromptsCount = useSelector((state: RootState) => state.chat.visiblePromptsCount[activeSessionId ?? '']);
  const [isLoadingOlder, setIsLoadingOlder] = useState(false);

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const target = e.currentTarget;
    if (
      target.scrollTop < 30 &&
      visiblePromptsCount < allPrompts.length &&
      !isLoadingOlder &&
      !isProcessing &&
      activeSessionId
    ) {
      setIsLoadingOlder(true);

      const oldScrollHeight = target.scrollHeight;
      const oldScrollTop = target.scrollTop;

      setTimeout(() => {
        dispatch(loadMorePrompts({ sessionId: activeSessionId }));

        requestAnimationFrame(() => {
          if (scrollRef.current) {
            const newScrollHeight = scrollRef.current.scrollHeight;
            const deltaHeight = newScrollHeight - oldScrollHeight;
            scrollRef.current.scrollTop = oldScrollTop + deltaHeight;
          }
          setIsLoadingOlder(false);
        });
      }, 400); // Natural loading delay
    }
  };

  return (
    <div className="chat-container">
      <div className="chat-history" ref={scrollRef} onScroll={handleScroll}>
        <div ref={contentRef} className="chat-history-content">
          {isLoadingOlder && (
            <div style={{ display: 'flex', justifyContent: 'center', alignItems: 'center', padding: '16px 0' }}>
              <div className="loader-spinner" />
            </div>
          )}
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
    </div>
  );
});
