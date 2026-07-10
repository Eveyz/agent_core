import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { previewEventReceived } from '../features/preview/previewSlice';
import type { PreviewEvent } from '../features/preview/previewApi';

export function usePreviewEvents(): void {
  const dispatch = useAppDispatch();

  useEffect(() => {
    let isMounted = true;
    let unlisten: (() => void) | undefined;

    const setup = async () => {
      const fn = await listen<PreviewEvent>('preview-event', (event) => {
        if (!isMounted || !event.payload) return;
        dispatch(previewEventReceived(event.payload));
      });
      if (!isMounted) {
        fn();
      } else {
        unlisten = fn;
      }
    };

    void setup();

    return () => {
      isMounted = false;
      unlisten?.();
    };
  }, [dispatch]);
}
