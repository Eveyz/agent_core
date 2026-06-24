import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { agentEventsBatch } from '../features/chat/chatSlice';

/**
 * Subscribe to the backend `agent-event` stream and dispatch events into the
 * chat slice in batches.
 *
 * Events arriving within one animation frame are collected and dispatched as
 * a single `agentEventsBatch` action. This means the reducer runs once (not
 * N times), and all downstream selectors evaluate once per frame — a critical
 * optimization for streaming where dozens of tokens arrive per second.
 *
 * Gap detection / resync is handled by the listenerMiddleware in store.ts.
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
      // Single dispatch — reducer processes all events in one pass.
      dispatch(agentEventsBatch(batch));
    };

    const scheduleFlush = (): void => {
      if (rafId !== null) return;
      rafId = requestAnimationFrame(flush);
    };

    const setupListener = async (): Promise<void> => {
      const fn = await listen<unknown>('agent-event', (event) => {
        if (!isMounted) return;
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
      if (buffer.length > 0) {
        dispatch(agentEventsBatch(buffer));
        buffer = [];
      }
      if (unlistenFn) unlistenFn();
    };
  }, [dispatch]);
}
