import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { agentEventReceived } from '../features/chat/chatSlice';

/**
 * Subscribe to the backend `agent-event` stream and dispatch each event into
 * the chat slice.
 *
 * During streaming the backend can fire dozens of `MessageUpdate` events per
 * second. Dispatching (and re-rendering) on every one is wasteful, so we buffer
 * arrivals for one animation frame and flush them together. React 18 already
 * batches dispatches within a single tick, and rAF coalesces the ticks to the
 * display refresh — this caps re-renders to ~60/s instead of one per token.
 *
 * Gap detection / resync is now handled by the listenerMiddleware in store.ts
 * (P2-1), so this hook no longer needs the window CustomEvent bridge.
 */
export function useAgentEventListener(): void {
  const dispatch = useAppDispatch();

  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;
    let buffer: Array<string | Record<string, unknown>> = [];
    let rafId: number | null = null;

    const flush = (): void => {
      rafId = null;
      if (!isMounted || buffer.length === 0) return;
      const batch = buffer;
      buffer = [];
      for (const payload of batch) {
        dispatch(agentEventReceived(payload));
      }
    };

    const scheduleFlush = (): void => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(flush);
    };

    const setupListener = async (): Promise<void> => {
      const fn = await listen<unknown>('agent-event', (event) => {
        buffer.push(event.payload as string | Record<string, unknown>);
        scheduleFlush();
      });
      if (!isMounted) {
        fn();
      } else {
        unlistenFn = fn;
      }
    };

    setupListener();

    return () => {
      isMounted = false;
      if (rafId !== null) cancelAnimationFrame(rafId);
      rafId = null;
      // Drain any buffered events so nothing is lost on unmount.
      if (buffer.length > 0) {
        for (const payload of buffer) {
          dispatch(agentEventReceived(payload));
        }
        buffer = [];
      }
      if (unlistenFn) unlistenFn();
    };
  }, [dispatch]);
}
