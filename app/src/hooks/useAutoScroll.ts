import { useEffect, useRef, useCallback, useState } from 'react';

export function useAutoScroll<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);

  // While true, every content mutation snaps to the bottom. Used when opening a
  // session so async rendering (markdown, code blocks, tool calls) keeps the view
  // pinned to the latest message until it settles.
  const stickRef = useRef(false);
  const stickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  // PERF-3: rAF-throttled scroll check to avoid layout thrashing.
  // Previously, the MutationObserver callback read scrollHeight and wrote
  // scrollTop synchronously on every DOM mutation (every token), causing
  // forced sync layout. Now we batch into a single rAF per frame.
  const scrollRafRef = useRef<number | null>(null);

  const snapToBottom = useCallback(() => {
    requestAnimationFrame(() => {
      const el = ref.current;
      if (!el) return;
      el.scrollTop = el.scrollHeight;
      setIsAtBottom(true);
    });
  }, []);

  const scrollToBottom = useCallback(() => {
    snapToBottom();
  }, [snapToBottom]);

  const forceStickToBottom = useCallback(() => {
    stickRef.current = true;
    snapToBottom();
    if (stickTimerRef.current) clearTimeout(stickTimerRef.current);
    stickTimerRef.current = setTimeout(() => {
      stickRef.current = false;
    }, 400);
  }, [snapToBottom]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;

    const refreshStick = () => {
      if (!stickRef.current) return;
      el.scrollTop = el.scrollHeight;
      if (stickTimerRef.current) clearTimeout(stickTimerRef.current);
      stickTimerRef.current = setTimeout(() => {
        stickRef.current = false;
      }, 400);
    };

    const handleScroll = () => {
      if (stickRef.current) return;
      const threshold = el.clientHeight;
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
      setIsAtBottom(isNearBottom);
    };

    el.addEventListener('scroll', handleScroll);

    // PERF-3: rAF-throttled scroll-to-bottom during streaming.
    // The callback is scheduled via rAF so it runs at most once per frame,
    // after layout has settled — no forced sync layout.
    const checkAndScroll = () => {
      scrollRafRef.current = null;
      if (stickRef.current) {
        refreshStick();
        return;
      }
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 200;
      if (isNearBottom) {
        el.scrollTop = el.scrollHeight;
      }
    };

    const observer = new MutationObserver(() => {
      if (scrollRafRef.current !== null) return; // already scheduled
      scrollRafRef.current = requestAnimationFrame(checkAndScroll);
    });

    observer.observe(el, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      el.removeEventListener('scroll', handleScroll);
      if (stickTimerRef.current) clearTimeout(stickTimerRef.current);
      if (scrollRafRef.current !== null) cancelAnimationFrame(scrollRafRef.current);
    };
  }, []);

  return { ref, scrollToBottom, forceStickToBottom, isAtBottom };
}
