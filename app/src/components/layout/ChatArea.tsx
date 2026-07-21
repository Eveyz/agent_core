import { RefObject, useState, memo, useRef, useLayoutEffect, useEffect, useCallback } from 'react';
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
  onSend: (msg: string | { text: string }) => void;
}

/** loadMorePrompts adds 2 prompts → typically 4 entries (user + turn each). */
const PREPEND_FORCE_MOUNT = 4;
/** Stop preserving after this long even if height keeps changing. */
const PRESERVE_MAX_MS = 1000;
/** Consider layout settled after this quiet period with no resize. */
const PRESERVE_SETTLE_MS = 150;

interface ScrollPreserveState {
  /** Entry that should stay visually fixed while older content prepends. */
  anchorId: string;
  /** Desired viewport Y of the anchor (from getBoundingClientRect().top). */
  anchorTop: number;
}

function restoreScrollAnchor(
  scrollEl: HTMLElement,
  preserve: ScrollPreserveState,
): void {
  const anchorEl = scrollEl.querySelector(
    `[data-entry-id="${CSS.escape(preserve.anchorId)}"]`,
  ) as HTMLElement | null;
  if (!anchorEl) return;
  const delta = anchorEl.getBoundingClientRect().top - preserve.anchorTop;
  if (Math.abs(delta) > 0.5) {
    scrollEl.scrollTop += delta;
  }
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
  const allPrompts = useSelector((state: RootState) => state.chat.allPrompts[activeSessionId ?? '']);
  const visiblePromptsCount = useSelector((state: RootState) => state.chat.visiblePromptsCount[activeSessionId ?? '']);
  const [isLoadingOlder, setIsLoadingOlder] = useState(false);
  /** Force-mount newly prepended rows so placeholder→real height doesn't fight the anchor. */
  const [forceMountTopCount, setForceMountTopCount] = useState(0);
  const preserveRef = useRef<ScrollPreserveState | null>(null);

  const restoreAnchor = useCallback(() => {
    const preserve = preserveRef.current;
    const scrollEl = scrollRef.current;
    if (!preserve || !scrollEl) return;
    restoreScrollAnchor(scrollEl, preserve);
  }, [scrollRef]);

  // Correct before paint whenever prepended entries land in the DOM.
  useLayoutEffect(() => {
    if (!preserveRef.current) return;
    restoreAnchor();
  }, [entryIds, restoreAnchor]);

  // Keep correcting while LazyEntry / markdown / code blocks settle their heights.
  useEffect(() => {
    if (!isLoadingOlder) return;
    const content = contentRef.current;

    let settled = false;
    let settleTimer: number | null = null;
    let maxTimer: number | null = null;
    let ro: ResizeObserver | null = null;

    const finish = () => {
      if (settled) return;
      settled = true;
      if (settleTimer != null) clearTimeout(settleTimer);
      if (maxTimer != null) clearTimeout(maxTimer);
      ro?.disconnect();
      preserveRef.current = null;
      setForceMountTopCount(0);
      setIsLoadingOlder(false);
    };

    const scheduleSettle = () => {
      if (settleTimer != null) clearTimeout(settleTimer);
      settleTimer = window.setTimeout(finish, PRESERVE_SETTLE_MS);
    };

    if (content && typeof ResizeObserver !== 'undefined') {
      ro = new ResizeObserver(() => {
        restoreAnchor();
        scheduleSettle();
      });
      ro.observe(content);
    }

    // Kick the settle clock even if height doesn't change further.
    scheduleSettle();
    maxTimer = window.setTimeout(finish, PRESERVE_MAX_MS);

    return () => {
      settled = true;
      if (settleTimer != null) clearTimeout(settleTimer);
      if (maxTimer != null) clearTimeout(maxTimer);
      ro?.disconnect();
    };
  }, [isLoadingOlder, contentRef, restoreAnchor]);

  const handleScroll = (e: React.UIEvent<HTMLDivElement>) => {
    const target = e.currentTarget;
    if (
      target.scrollTop < 30 &&
      visiblePromptsCount < allPrompts.length &&
      !isLoadingOlder &&
      !isProcessing &&
      activeSessionId &&
      entryIds.length > 0
    ) {
      const anchorId = entryIds[0];
      const anchorEl = target.querySelector(
        `[data-entry-id="${CSS.escape(anchorId)}"]`,
      ) as HTMLElement | null;
      if (!anchorEl) return;

      preserveRef.current = {
        anchorId,
        anchorTop: anchorEl.getBoundingClientRect().top,
      };
      setForceMountTopCount(PREPEND_FORCE_MOUNT);
      setIsLoadingOlder(true);
      dispatch(loadMorePrompts({ sessionId: activeSessionId }));
    }
  };

  return (
    <div className="chat-container">
      <div className="chat-history" ref={scrollRef} onScroll={handleScroll}>
        {isLoadingOlder && (
          <div className="chat-history-load-older" aria-hidden>
            <div className="loader-spinner" />
          </div>
        )}
        <div ref={contentRef} className="chat-history-content">
          {entryIds.map((id, index) => (
            <LazyEntry
              key={id}
              entryId={id}
              defaultModel={defaultModel}
              handleRetry={handleRetry}
              isProcessing={isProcessing}
              scrollRef={scrollRef}
              forceVisible={index < forceMountTopCount || index >= entryIds.length - 3}
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
