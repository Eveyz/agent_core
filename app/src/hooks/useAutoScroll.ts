import { useEffect, useRef, useCallback, useState, useLayoutEffect } from 'react';

export function useAutoScroll<T extends HTMLElement, U extends HTMLElement = HTMLDivElement>(dependencies: any[]) {
  const scrollRef = useRef<T | null>(null);
  const contentRef = useRef<U | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);
  
  // A synchronous lock to track if we should snap to bottom
  const isAutoScrollEnabled = useRef(true);

  // Expose a way to force scroll to bottom, e.g. on session switch
  const scrollToBottom = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
    isAutoScrollEnabled.current = true;
    setIsAtBottom(true);
  }, []);

  // 1. Synchronously snap to bottom whenever dependencies change (e.g. new chat messages)
  // useLayoutEffect runs before the browser paints, ensuring no flicker.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    if (isAutoScrollEnabled.current) {
      el.scrollTop = el.scrollHeight;
    }
  }, dependencies);

  // 2. Track user scrolling to disable/enable the lock
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const handleScroll = () => {
      // 20px threshold allows for subpixel rendering and small layout shifts
      const threshold = 20;
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight <= threshold;
      
      isAutoScrollEnabled.current = isNearBottom;
      setIsAtBottom(isNearBottom);
    };

    // passive: true improves scroll performance
    el.addEventListener('scroll', handleScroll, { passive: true });
    
    return () => el.removeEventListener('scroll', handleScroll);
  }, []);

  // 3. Catch async dimension changes like images loading
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;

    const targetEl = contentRef.current || el;

    const resizeObserver = new ResizeObserver(() => {
      if (isAutoScrollEnabled.current) {
        el.scrollTop = el.scrollHeight;
      }
    });

    resizeObserver.observe(targetEl);

    return () => resizeObserver.disconnect();
  }, []);

  return { scrollRef, contentRef, scrollToBottom, isAtBottom };
}
