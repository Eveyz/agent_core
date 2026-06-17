import { useEffect, useRef } from 'react';
import { useAppDispatch, useAppSelector } from './useAppDispatch';
import { useStore } from 'react-redux';
import type { RootState } from '../store';
import { entriesToMessages, entriesToEventLog } from '../features/chat/chatSlice';
import { saveSessionMessages } from '../features/project/projectSlice';

interface AutoSaveParams {
  activeSessionId: string | null;
  activeProjectPath: string | null;
  defaultModel: string;
}

export function useAutoSaveSession({
  activeSessionId,
  activeProjectPath,
  defaultModel,
}: AutoSaveParams): void {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  const isProcessing = useAppSelector((state) => state.chat.isProcessing);
  const resumedFromBackend = useAppSelector((state) => state.chat._resumedFromBackend);

  const lastAgentEndRef = useRef(false);

  useEffect(() => {
    if (isProcessing) {
      lastAgentEndRef.current = false;
      return;
    }

    if (resumedFromBackend) return;

    const state = store.getState();
    const entries = state.chat.entries;

    if (!lastAgentEndRef.current && entries.length > 0) {
      lastAgentEndRef.current = true;
      if (activeSessionId && activeProjectPath) {
        const msgs = entriesToMessages(entries);
        if (msgs.length > 0) {
          const { eventLog, processTimeMs, thoughtTimeMs } = entriesToEventLog(entries);
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
        }
      }
    }
  }, [isProcessing, resumedFromBackend, activeSessionId, activeProjectPath, defaultModel, dispatch, store]);
}
