import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { agentEventsBatch, btwDelta, btwDone, btwError } from '../features/chat/chatSlice';

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
    let btwUnlisten: (() => void) | undefined;
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

      // /btw side-channel stream (independent channel — not persisted, no seq)
      const btwFn = await listen<{ btw_id: string; event_type: string; text: string; session_id?: string }>('btw-event', (event) => {
        if (!isMounted) return;
        const e = event.payload;
        if (!e) return;
        const sessionId = e.session_id;
        if (!sessionId) return;
        if (e.event_type === 'delta') dispatch(btwDelta({ sessionId, id: e.btw_id, text: e.text }));
        else if (e.event_type === 'done') dispatch(btwDone({ sessionId, id: e.btw_id }));
        else if (e.event_type === 'error') dispatch(btwError({ sessionId, id: e.btw_id, text: e.text }));
      });
      if (!isMounted) {
        btwFn();
      } else {
        btwUnlisten = btwFn;
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
      if (btwUnlisten) btwUnlisten();
    };
  }, [dispatch]);
}
