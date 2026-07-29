import { RefObject, useState, memo, useRef, useLayoutEffect, useEffect, useCallback, useMemo } from 'react';
import { useSelector, shallowEqual } from 'react-redux';
import { RootState } from '../../store';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import { LazyEntry } from '../chat/LazyEntry';
import { loadMorePrompts, selectActiveBtwEntries } from '../../features/chat/chatSlice';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import {
  entriesThroughAnchor,
  PrependScrollAnchor,
} from './prependScrollAnchor';

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
  const prependAnchorRef = useRef(new PrependScrollAnchor());
  const entriesBeforePrependRef = useRef<Set<string> | null>(null);
  const seenPrependedEntriesRef = useRef(new Set<string>());
  const pendingReadyEntriesRef = useRef<Set<string> | null>(null);
  const entryHeightLedgerRef = useRef(new Map<string, number>());
  const settleFramesRef = useRef<number[]>([]);
  const layoutRevisionRef = useRef(0);

  const restoreAnchor = useCallback(() => {
    const scrollEl = scrollRef.current;
    if (!scrollEl) return;
    prependAnchorRef.current.restore(scrollEl);
  }, [scrollRef]);

  const cancelSettleFrames = useCallback(() => {
    for (const frame of settleFramesRef.current) cancelAnimationFrame(frame);
    settleFramesRef.current = [];
  }, []);

  const finishInitialPrependLayout = useCallback(() => {
    cancelSettleFrames();
    const expectedRevision = layoutRevisionRef.current;
    const first = requestAnimationFrame(() => {
      if (layoutRevisionRef.current !== expectedRevision) {
        finishInitialPrependLayout();
        return;
      }
      restoreAnchor();
      const second = requestAnimationFrame(() => {
        if (layoutRevisionRef.current !== expectedRevision) {
          finishInitialPrependLayout();
          return;
        }
        restoreAnchor();
        settleFramesRef.current = [];
        pendingReadyEntriesRef.current = null;
        entriesBeforePrependRef.current = null;
        seenPrependedEntriesRef.current.clear();
        prependAnchorRef.current.cancel();
        setIsLoadingOlder(false);
      });
      settleFramesRef.current = [second];
    });
    settleFramesRef.current = [first];
  }, [cancelSettleFrames, restoreAnchor]);

  const scheduleInitialPrependSettle = useCallback(() => {
    if (!isLoadingOlder || pendingReadyEntriesRef.current?.size !== 0) return;
    finishInitialPrependLayout();
  }, [finishInitialPrependLayout, isLoadingOlder]);

  const handleEntryHeightChange = useCallback((entryId: string, height: number) => {
    entryHeightLedgerRef.current.set(entryId, height);
  }, []);

  const handleEntryReady = useCallback((entryId: string) => {
    const pending = pendingReadyEntriesRef.current;
    if (!pending?.delete(entryId) || pending.size !== 0) return;
    scheduleInitialPrependSettle();
  }, [scheduleInitialPrependSettle]);

  // Correct before paint whenever prepended entries land in the DOM.
  useLayoutEffect(() => {
    if (!prependAnchorRef.current.isActive()) return;
    if (isLoadingOlder) {
      const before = entriesBeforePrependRef.current ?? new Set<string>();
      const pending = pendingReadyEntriesRef.current ?? new Set<string>();
      for (const entryId of entryIds) {
        if (!before.has(entryId) && !seenPrependedEntriesRef.current.has(entryId)) {
          seenPrependedEntriesRef.current.add(entryId);
          pending.add(entryId);
        }
      }
      if (pending.size > 0) pendingReadyEntriesRef.current = pending;
    }
    restoreAnchor();
    if (pendingReadyEntriesRef.current !== null) {
      scheduleInitialPrependSettle();
    }
  }, [entryIds, isLoadingOlder, restoreAnchor, scheduleInitialPrependSettle]);

  // Keep correcting while the user is idle at the prepend boundary. There is
  // intentionally no timeout: syntax highlighting and images can resolve well
  // after one second. Explicit wheel/touch/pointer intent cancels preservation.
  useEffect(() => {
    const content = contentRef.current;
    if (!content || typeof ResizeObserver === 'undefined') return;
    const ro = new ResizeObserver(() => {
      if (!prependAnchorRef.current.isActive()) return;
      layoutRevisionRef.current += 1;
      // ResizeObserver is delivered after layout and before paint. Correcting
      // here prevents a displaced frame from ever becoming visible.
      restoreAnchor();
      if (pendingReadyEntriesRef.current?.size === 0) {
        scheduleInitialPrependSettle();
      }
    });
    ro.observe(content);

    return () => {
      ro.disconnect();
    };
  }, [contentRef, restoreAnchor, scheduleInitialPrependSettle]);

  useEffect(() => () => cancelSettleFrames(), [cancelSettleFrames]);

  useEffect(() => {
    prependAnchorRef.current.cancel();
    entriesBeforePrependRef.current = null;
    seenPrependedEntriesRef.current.clear();
    pendingReadyEntriesRef.current = null;
    entryHeightLedgerRef.current.clear();
    cancelSettleFrames();
    setIsLoadingOlder(false);
  }, [activeSessionId, cancelSettleFrames]);

  const forceMountIds = useMemo(
    () => isLoadingOlder
      ? entriesThroughAnchor(entryIds, prependAnchorRef.current.anchorId())
      : new Set<string>(),
    [entryIds, isLoadingOlder],
  );

  const cancelPrependPreservation = useCallback(() => {
    prependAnchorRef.current.cancel();
    entriesBeforePrependRef.current = null;
    seenPrependedEntriesRef.current.clear();
    pendingReadyEntriesRef.current = null;
    cancelSettleFrames();
    setIsLoadingOlder(false);
  }, [cancelSettleFrames]);

  useEffect(() => {
    if (isProcessing) cancelPrependPreservation();
  }, [cancelPrependPreservation, isProcessing]);

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
      if (!prependAnchorRef.current.capture(target, anchorId)) return;
      entriesBeforePrependRef.current = new Set(entryIds);
      pendingReadyEntriesRef.current = null;
      cancelSettleFrames();
      setIsLoadingOlder(true);
      dispatch(loadMorePrompts({ sessionId: activeSessionId }));
    }
  };

  const handleScrollKeyDown = (event: React.KeyboardEvent<HTMLDivElement>) => {
    if ([
      'ArrowUp',
      'ArrowDown',
      'PageUp',
      'PageDown',
      'Home',
      'End',
      ' ',
    ].includes(event.key)) {
      cancelPrependPreservation();
    }
  };

  return (
    <div className="chat-container">
      <div
        className="chat-history"
        ref={scrollRef}
        onScroll={handleScroll}
        onWheel={cancelPrependPreservation}
        onTouchMove={cancelPrependPreservation}
        onPointerDown={cancelPrependPreservation}
        onKeyDown={handleScrollKeyDown}
      >
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
              forceVisible={forceMountIds.has(id) || index >= entryIds.length - 3}
              estimatedHeight={entryHeightLedgerRef.current.get(id)}
              onHeightChange={handleEntryHeightChange}
              onReady={handleEntryReady}
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
        <button
          className="scroll-to-bottom-btn"
          onClick={() => {
            cancelPrependPreservation();
            scrollToBottom('auto');
          }}
          title="Scroll to latest"
        >
          <ChevronDownIcon size={18} />
        </button>
      )}
    </div>
  );
});
