import { useEffect } from 'react';
import { useStore } from 'react-redux';
import { RootState } from '../store';
import { useAppDispatch } from './useAppDispatch';
import { resyncRun } from '../features/chat/chatSlice';

export function useVisibilityResync() {
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();

  useEffect(() => {
    const handleFocusOrVisibility = () => {
      const state = store.getState();
      if (document.visibilityState === 'hidden') return;

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
