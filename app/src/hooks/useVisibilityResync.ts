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
      // We only care when the app becomes visible or focused
      if (document.visibilityState === 'hidden') return;
      
      const state = store.getState();
      const { isProcessing, runId, lastSeqByRun } = state.chat;
      
      if (isProcessing && runId) {
        const fromSeq = lastSeqByRun[runId] ?? 0;
        dispatch(resyncRun({ runId, fromSeq }));
      }
    };

    document.addEventListener('visibilitychange', handleFocusOrVisibility);
    window.addEventListener('focus', handleFocusOrVisibility);

    return () => {
      document.removeEventListener('visibilitychange', handleFocusOrVisibility);
      window.removeEventListener('focus', handleFocusOrVisibility);
    };
  }, [dispatch, store]);
}
