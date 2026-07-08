import { useCallback } from 'react';
import { useStore } from 'react-redux';
import type { RootState } from '../store';
import { useAppDispatch } from './useAppDispatch';
import { getFullMessages, getFullEventLog, cacheCurrentSession } from '../features/chat/chatSlice';
import { saveSessionMessages } from '../features/project/projectSlice';

/**
 * Shared session-save logic (P2-3).
 *
 * Encapsulates the common pattern: read entries/subagents from state,
 * convert to full messages + full event log, and dispatch saveSessionMessages.
 */
export function useSaveSession() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  const saveSession = useCallback(
    (params: {
      activeSessionId: string | null;
      activeProjectPath: string | null;
      defaultModel: string;
      skipIfResumed?: boolean;
      cacheAfter?: boolean;
    }) => {
      const { activeSessionId, activeProjectPath, defaultModel, skipIfResumed, cacheAfter } = params;
      if (!activeSessionId || !activeProjectPath) return;

      const chatState = store.getState().chat;
      if (skipIfResumed && chatState._resumedFromBackend) return;

      const msgs = getFullMessages(chatState);
      if (msgs.length === 0) {
        if (cacheAfter) dispatch(cacheCurrentSession(activeSessionId));
        return;
      }

      const { eventLog, processTimeMs, thoughtTimeMs } = getFullEventLog(chatState);
      dispatch(
        saveSessionMessages({
          sessionId: activeSessionId,
          messages: msgs,
          cwd: activeProjectPath,
          modelUsed: defaultModel,
          processTimeMs: processTimeMs || undefined,
          thoughtTimeMs: thoughtTimeMs || undefined,
          eventLog,
        })
      );
      if (cacheAfter) dispatch(cacheCurrentSession(activeSessionId));
    },
    [dispatch, store]
  );

  return saveSession;
}
