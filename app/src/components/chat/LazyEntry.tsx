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
  onSend?: (msg: string) => void;
}

// Height used for off-screen placeholders so the scrollbar and the
// auto-scroll math (scrollHeight / scrollTop) stay roughly correct without
// mounting the — expensive — entry. Only affects entries that have never
// scrolled into view; once visible an entry stays mounted at its real height.
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
 * Once an entry becomes visible it stays mounted for the lifetime of the
 * session (we never swap it back to a placeholder). This preserves component
 * state (e.g. expanded tool blocks) and avoids remount jank; it is strictly
 * better than the previous "mount everything immediately" behaviour.
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
  const [visible, setVisible] = useState(forceVisible);

  useEffect(() => {
    if (visible) return;
    const el = wrapperRef.current;
    if (!el) return;
    const root = scrollRef.current ?? null;
    const observer = new IntersectionObserver(
      (entries) => {
        for (const e of entries) {
          if (e.isIntersecting) {
            setVisible(true);
            break;
          }
        }
      },
      { root, rootMargin: ROOT_MARGIN, threshold: 0 },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, [visible, scrollRef]);

  if (visible) {
    return (
      <EntryRow
        entryId={entryId}
        defaultModel={defaultModel}
        handleRetry={handleRetry}
        isProcessing={isProcessing}
        onSend={onSend}
      />
    );
  }

  return (
    <div
      ref={wrapperRef}
      className="lazy-entry-placeholder"
      style={{ minHeight: PLACEHOLDER_MIN_HEIGHT }}
    />
  );
});
