import { useRef, useEffect, useState, memo, RefObject } from 'react';
import { EntryRow } from './EntryRow';

interface LazyEntryProps {
  entryId: string;
  defaultModel: string;
  handleRetry: (id: string, text?: string) => void;
  isProcessing: boolean;
  scrollRef: RefObject<HTMLElement | null>;
  /** Always render these last N entries (the active/streaming ones at the
   * bottom) without waiting for the IntersectionObserver, so streaming
   * output mounts instantly. */
  forceVisible: boolean;
  onSend?: (msg: string | { text: string }) => void;
}

// Height used for off-screen placeholders so the scrollbar and the
// auto-scroll math (scrollHeight / scrollTop) stay roughly correct without
// mounting the expensive entry. A measured height replaces this estimate after
// first render so entries can be safely unmounted again outside the window.
const PLACEHOLDER_MIN_HEIGHT = 160;
// Pre-render this far outside the viewport so normal scrolling never shows a
// placeholder flash.
const ROOT_MARGIN = '400px 0px 400px 0px';

/**
 * Lazily mounts EntryRow only when it is near the viewport.
 *
 * Restoring a long session previously mounted every entry (and every code
 * block / markdown parse inside it) at once, freezing the UI. By deferring
 * off-screen entries to cheap placeholder divs, the initial mount cost is
 * bounded to what is actually visible.
 *
 * The scroll container is unchanged, so useAutoScroll's scrollTop/scrollHeight
 * logic keeps working untouched — placeholders occupy height in the flow.
 */
export const LazyEntry = memo(function LazyEntry({
  entryId,
  defaultModel,
  handleRetry,
  isProcessing,
  scrollRef,
  forceVisible,
  onSend,
}: LazyEntryProps) {
  const wrapperRef = useRef<HTMLDivElement | null>(null);
  const measuredHeight = useRef(PLACEHOLDER_MIN_HEIGHT);
  const [nearViewport, setNearViewport] = useState(forceVisible);
  const visible = forceVisible || nearViewport;

  // When a parent briefly force-mounts (e.g. prepended older prompts), keep the
  // entry marked near-viewport so clearing forceVisible does not collapse it
  // back to a placeholder and shift scroll.
  useEffect(() => {
    if (forceVisible) setNearViewport(true);
  }, [forceVisible]);

  useEffect(() => {
    const el = wrapperRef.current;
    if (!el) return;
    const root = scrollRef.current ?? null;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          setNearViewport(e.isIntersecting);
        }
      },
      { root, rootMargin: ROOT_MARGIN, threshold: 0 },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [scrollRef]);

  useEffect(() => {
    if (!visible || !wrapperRef.current) return;
    const observer = new ResizeObserver(([entry]) => {
      if (entry.contentRect.height > 0) measuredHeight.current = entry.contentRect.height;
    });
    observer.observe(wrapperRef.current);
    return () => observer.disconnect();
  }, [visible]);

  return (
    <div
      ref={wrapperRef}
      data-entry-id={entryId}
      className={visible ? 'lazy-entry-mounted' : 'lazy-entry-placeholder'}
      style={visible ? undefined : { minHeight: measuredHeight.current }}
    >
      {visible ? (
      <EntryRow
        entryId={entryId}
        defaultModel={defaultModel}
        handleRetry={handleRetry}
        isProcessing={isProcessing}
        onSend={onSend}
      />
      ) : null}
    </div>
  );
});
