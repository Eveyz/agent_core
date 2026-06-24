import { useEffect, useRef, useCallback, useState } from 'react';

export function useAutoScroll<T extends HTMLElement>() {
  const ref = useRef<T | null>(null);
  const [isAtBottom, setIsAtBottom] = useState(true);

  // While true, every content mutation snaps to the bottom. Used when opening a
  // session so async rendering (markdown, code blocks, tool calls) keeps the view
  // pinned to the latest message until it settles.
  const stickRef = useRef(false);
  const stickTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

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

  // Force-stick to the bottom for a settling window. Each content mutation while
  // sticking refreshes the window, so the view stays pinned through all the async
  // reflows that happen as a loaded session renders, then releases so the user can
  // scroll freely.
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
      // Don't fight the user while we're force-sticking during load.
      if (stickRef.current) return;
      const threshold = el.clientHeight;
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < threshold;
      setIsAtBottom(isNearBottom);
    };

    el.addEventListener('scroll', handleScroll);

    const observer = new MutationObserver(() => {
      if (stickRef.current) {
        refreshStick();
        return;
      }
      const isNearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 200;
      if (isNearBottom) {
        el.scrollTop = el.scrollHeight;
      }
    });

    observer.observe(el, { childList: true, subtree: true });
    return () => {
      observer.disconnect();
      el.removeEventListener('scroll', handleScroll);
      if (stickTimerRef.current) clearTimeout(stickTimerRef.current);
    };
  }, []);

  return { ref, scrollToBottom, forceStickToBottom, isAtBottom };
}
