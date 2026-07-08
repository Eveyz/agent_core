import { useCallback } from 'react';
import { useStore } from 'react-redux';
import type { RootState } from '../store';
import { useAppDispatch } from './useAppDispatch';
import { getFullMessages, getTimingMetrics } from '../features/chat/chatSlice';
import { saveSessionMessages } from '../features/project/projectSlice';

/**
 * Shared session-save logic (P2-3).
 *
 * Encapsulates the common pattern: read entries/subagents from state,
 * convert to full messages, and dispatch saveSessionMessages.
 */
export function useSaveSession() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  const saveSession = useCallback(
    (params: {
      activeSessionId: string | null;
      activeProjectPath: string | null;
      defaultModel: string;
      cacheAfter?: boolean;
      cacheOnly?: boolean;
    }) => {
      const { activeSessionId, activeProjectPath, defaultModel, cacheAfter, cacheOnly } = params;
      if (!activeSessionId || !activeProjectPath) return;

      const chatState = store.getState().chat;
      if (chatState.activeSessionId !== activeSessionId) {
        return;
      }

      if (cacheOnly) return;

      if (!chatState.isDirty[activeSessionId]) return;

      const msgs = getFullMessages(chatState);
      if (msgs.length === 0) return;

      const entries = chatState.entries[activeSessionId] ?? [];
      const { processTimeMs, thoughtTimeMs } = getTimingMetrics(entries);
      dispatch(
        saveSessionMessages({
          sessionId: activeSessionId,
          messages: msgs,
          cwd: activeProjectPath,
          modelUsed: defaultModel,
          processTimeMs: processTimeMs || undefined,
          thoughtTimeMs: thoughtTimeMs || undefined,
        })
      );
      if (cacheAfter) {
        // No need to cache — data persists in session maps
      }
    },
    [dispatch, store]
  );

  return saveSession;
}
