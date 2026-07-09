import { useEffect, useRef, useCallback, useState, useLayoutEffect } from 'react';

interface UseAutoScrollOptions {
  /** When these change, pin to bottom if stick-to-bottom is enabled (new message, session switch). */
  deps: unknown[];
  /** While true, rAF loop keeps the view pinned during streaming. */
  isProcessing: boolean;
}

const BOTTOM_THRESHOLD_PX = 40;

function maxScrollTop(el: HTMLElement): number {
  return Math.max(0, el.scrollHeight - el.clientHeight);
}

function isNearBottom(el: HTMLElement, threshold = BOTTOM_THRESHOLD_PX): boolean {
  return maxScrollTop(el) - el.scrollTop <= threshold;
}

/**
 * Stick-to-bottom scrolling for the chat history pane.
 *
 * Important edge cases this handles:
 * - Never assign `scrollTop = scrollHeight` (can overshoot); always clamp to maxScroll.
 * - Do not infer "user scrolled up" from scrollTop deltas — content height can shrink
 *   during markdown/code-block remounts and falsely disable sticking (blank viewport).
 * - Ignore scroll events caused by our own programmatic pins.
 */
export function useAutoScroll<
  T extends HTMLElement,
  U extends HTMLElement = HTMLDivElement,
>(options: UseAutoScrollOptions) {
  const { deps, isProcessing } = options;
  const scrollRef = useRef<T | null>(null);
  const contentRef = useRef<U | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);

  /** When true, keep the viewport pinned to the latest content. */
  const stickToBottom = useRef(true);
  /** Suppress scroll-listener side effects while we pin programmatically. */
  const programmaticScroll = useRef(false);
  const clearProgrammaticRaf = useRef<number | null>(null);

  const markProgrammatic = useCallback(() => {
    programmaticScroll.current = true;
    if (clearProgrammaticRaf.current != null) {
      cancelAnimationFrame(clearProgrammaticRaf.current);
    }
    // Clear after the scroll event from this write has had a chance to fire.
    clearProgrammaticRaf.current = requestAnimationFrame(() => {
      clearProgrammaticRaf.current = requestAnimationFrame(() => {
        programmaticScroll.current = false;
        clearProgrammaticRaf.current = null;
      });
    });
  }, []);

  const pinToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el || el.clientHeight === 0) return;
    markProgrammatic();
    el.scrollTop = maxScrollTop(el);
  }, [markProgrammatic]);

  const scrollToBottom = useCallback((behavior: ScrollBehavior = 'smooth') => {
    const el = scrollRef.current;
    if (!el || el.clientHeight === 0) return;

    stickToBottom.current = true;
    setIsAtBottom(true);

    const top = maxScrollTop(el);
    markProgrammatic();
    if (behavior === 'auto' || behavior === 'instant') {
      el.scrollTop = top;
    } else {
      el.scrollTo({ top, behavior: 'smooth' });
      // Smooth can be interrupted by the next stream frame — snap once as fallback.
      requestAnimationFrame(() => {
        if (!scrollRef.current || !stickToBottom.current) return;
        markProgrammatic();
        scrollRef.current.scrollTop = maxScrollTop(scrollRef.current);
      });
    }
  }, [markProgrammatic]);

  // Non-streaming updates (new entry, session switch): pin once in layout.
  useLayoutEffect(() => {
    if (!stickToBottom.current || isProcessing) return;
    pinToBottom();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);

  // Streaming: pin every frame while the user hasn't scrolled away.
  useEffect(() => {
    if (!isProcessing) return;

    let id = 0;
    const tick = () => {
      const el = scrollRef.current;
      if (el && stickToBottom.current && el.clientHeight > 0) {
        const max = maxScrollTop(el);
        if (el.scrollTop < max - 1) {
          markProgrammatic();
          el.scrollTop = max;
        }
      }
      id = requestAnimationFrame(tick);
    };

    id = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(id);
      if (clearProgrammaticRaf.current != null) {
        cancelAnimationFrame(clearProgrammaticRaf.current);
        clearProgrammaticRaf.current = null;
      }
    };
  }, [isProcessing, markProgrammatic]);

  // User intent + near-bottom tracking.
  // Re-bind when processing/deps change so the listener attaches after EmptyState → ChatArea.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const handleScroll = () => {
      if (programmaticScroll.current) return;
      const near = isNearBottom(el);
      stickToBottom.current = near;
      setIsAtBottom(near);
    };

    // Wheel up is the reliable "user wants to leave the bottom" signal.
    // Do not use scrollTop deltas — height shrinks look identical to scrolling up.
    const handleWheel = (e: WheelEvent) => {
      if (e.deltaY < 0) {
        stickToBottom.current = false;
        setIsAtBottom(false);
      }
    };

    const handleTouchMove = () => {
      // Touch drag away from bottom is reflected in scroll; if still near bottom, keep sticking.
      if (programmaticScroll.current) return;
      if (!isNearBottom(el)) {
        stickToBottom.current = false;
        setIsAtBottom(false);
      }
    };

    el.addEventListener('scroll', handleScroll, { passive: true });
    el.addEventListener('wheel', handleWheel, { passive: true });
    el.addEventListener('touchmove', handleTouchMove, { passive: true });

    return () => {
      el.removeEventListener('scroll', handleScroll);
      el.removeEventListener('wheel', handleWheel);
      el.removeEventListener('touchmove', handleTouchMove);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isProcessing, ...deps]);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}
