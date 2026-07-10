import { useEffect } from 'react';
import { listen } from '@tauri-apps/api/event';
import { useAppDispatch } from './useAppDispatch';
import { openPreviewPanel, previewOpenedFromTool } from '../features/preview/previewSlice';
import type { PreviewDescriptor } from '../features/preview/previewApi';

function tryParsePreviewResult(result: string): PreviewDescriptor | null {
  try {
    const parsed = JSON.parse(result) as PreviewDescriptor;
    if (parsed?.id && parsed?.url) {
      return parsed;
    }
  } catch {
    // not JSON
  }
  return null;
}

/**
 * Opens the embedded preview panel when the agent calls the `preview` tool.
 */
export function usePreviewToolHandler(): void {
  const dispatch = useAppDispatch();

  useEffect(() => {
    let isMounted = true;
    let unlistenFn: (() => void) | undefined;

    const setup = async () => {
      const fn = await listen<Record<string, unknown>>('agent-event', (event) => {
        if (!isMounted) return;
        const payload = event.payload;
        if (!payload || typeof payload !== 'object') return;

        const eventType = payload.event as string | undefined;
        if (eventType !== 'tool_ended') return;

        const name = payload.name as string | undefined;
        if (name !== 'preview') return;

        const isError = payload.is_error as boolean | undefined;
        const result = payload.result as string | undefined;
        if (isError || !result) return;

        const descriptor = tryParsePreviewResult(result);
        if (!descriptor) return;

        dispatch(previewOpenedFromTool(descriptor));
        dispatch(openPreviewPanel());
        window.dispatchEvent(
          new CustomEvent('open-right-sidebar', { detail: { tab: 'preview' } }),
        );
      });

      if (!isMounted) {
        fn();
      } else {
        unlistenFn = fn;
      }
    };

    void setup();

    return () => {
      isMounted = false;
      unlistenFn?.();
    };
  }, [dispatch]);
}
