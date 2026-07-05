import { useRef, useCallback, useEffect } from 'react';

/**
 * A timeout that automatically clears on unmount or re-invoke.
 * Returns a `setTimer` function that accepts (fn, ms) — calling it again
 * before the previous timer fires cancels the old one.
 */
export function useTimeout() {
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setTimer = useCallback((fn: () => void, ms: number) => {
    if (timerRef.current) clearTimeout(timerRef.current);
    timerRef.current = setTimeout(() => {
      timerRef.current = null;
      fn();
    }, ms);
  }, []);

  useEffect(() => () => {
    if (timerRef.current) clearTimeout(timerRef.current);
  }, []);

  return setTimer;
}
