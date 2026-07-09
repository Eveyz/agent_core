import { useCallback } from 'react';
import { useStore } from 'react-redux';
import type { RootState } from '../store';
import { useAppDispatch } from './useAppDispatch';
import { getFullMessagesForSession, getTimingMetrics } from '../features/chat/chatSlice';
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
      /** Skip isDirty check — used before retry so DB matches truncated UI. */
      force?: boolean;
    }) => {
      const { activeSessionId, activeProjectPath, defaultModel, cacheOnly, force } = params;
      if (!activeSessionId || !activeProjectPath) return;

      const chatState = store.getState().chat;
      if (!force && !chatState.isDirty[activeSessionId]) return;
      if (cacheOnly) return;

      const msgs = getFullMessagesForSession(chatState, activeSessionId);
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
    },
    [dispatch, store]
  );

  /** Awaitable save — returns true when messages were persisted to SQLite. */
  const saveSessionNow = useCallback(
    async (params: {
      activeSessionId: string | null;
      activeProjectPath: string | null;
      defaultModel: string;
      force?: boolean;
    }): Promise<boolean> => {
      const { activeSessionId, activeProjectPath, defaultModel, force } = params;
      if (!activeSessionId || !activeProjectPath) return false;

      const chatState = store.getState().chat;
      if (!force && !chatState.isDirty[activeSessionId]) return true;

      const msgs = getFullMessagesForSession(chatState, activeSessionId);
      if (msgs.length === 0) return false;

      const entries = chatState.entries[activeSessionId] ?? [];
      const { processTimeMs, thoughtTimeMs } = getTimingMetrics(entries);
      const result = await dispatch(
        saveSessionMessages({
          sessionId: activeSessionId,
          messages: msgs,
          cwd: activeProjectPath,
          modelUsed: defaultModel,
          processTimeMs: processTimeMs || undefined,
          thoughtTimeMs: thoughtTimeMs || undefined,
        })
      );
      return saveSessionMessages.fulfilled.match(result);
    },
    [dispatch, store]
  );

  return { saveSession, saveSessionNow };
}
