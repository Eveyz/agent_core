import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { agentEventReceived } from '../features/chat/chatSlice';

export function useAgentEventListener(): void {
  const dispatch = useAppDispatch();

  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;

    const setupListener = async (): Promise<void> => {
      const fn = await listen<unknown>('agent-event', (event) => {
        dispatch(agentEventReceived(event.payload as string | Record<string, unknown>));
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
      if (unlistenFn) unlistenFn();
    };
  }, [dispatch]);
}
