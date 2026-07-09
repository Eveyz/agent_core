import { useEffect } from 'react';
import { useStore } from 'react-redux';
import { RootState } from '../store';
import { useAppDispatch } from './useAppDispatch';
import { getFullMessagesForSession, getTimingMetrics, resyncRun } from '../features/chat/chatSlice';
import { saveSessionMessages } from '../features/project/projectSlice';

export function useVisibilityResync() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  useEffect(() => {
    const handleFocusOrVisibility = () => {
      const state = store.getState();
      if (document.visibilityState === 'hidden') {
        const sid = state.project.activeSessionId;
        if (!sid || !state.chat.isDirty[sid]) return;
        const activeProject = state.project.projects.find((p) => p.id === state.project.activeProjectId);
        if (!activeProject?.path) return;
        const messages = getFullMessagesForSession(state.chat, sid);
        if (messages.length === 0) return;
        const entries = state.chat.entries[sid] ?? [];
        const { processTimeMs, thoughtTimeMs } = getTimingMetrics(entries);
        dispatch(saveSessionMessages({
          sessionId: sid,
          messages,
          cwd: activeProject.path,
          modelUsed: state.settings.config?.default_model || '',
          processTimeMs,
          thoughtTimeMs,
        }));
        return;
      }

      const sid = state.project.activeSessionId;
      const { isProcessing, runId } = sid ? {
        isProcessing: state.chat.processing[sid],
        runId: state.chat.runId[sid],
      } : { isProcessing: false, runId: null };
      const { lastSeqByRun } = state.chat;
      
      if (isProcessing && runId) {
        const fromSeq = lastSeqByRun[runId] ?? 0;
        dispatch(resyncRun({ runId, fromSeq }));
      }
    };

    document.addEventListener('visibilitychange', handleFocusOrVisibility);
    window.addEventListener('focus', handleFocusOrVisibility);
    window.addEventListener('pagehide', handleFocusOrVisibility);

    return () => {
      document.removeEventListener('visibilitychange', handleFocusOrVisibility);
      window.removeEventListener('focus', handleFocusOrVisibility);
      window.removeEventListener('pagehide', handleFocusOrVisibility);
    };
  }, [dispatch, store]);
}
