import { useEffect, useRef } from 'react';
import { useAppDispatch } from './useAppDispatch';
import {
  resumeSession,
  setActiveSession,
} from '../features/project/projectSlice';
import { clearChat } from '../features/chat/chatSlice';
import { setDefaultModel } from '../features/settings/settingsSlice';

interface UseSessionLoaderProps {
  projectsLoaded: boolean;
  activeProjectId: string | null;
  activeSessionId: string | null;
  scrollToBottom: (behavior?: ScrollBehavior) => void;
}

export function useSessionLoader({
  projectsLoaded,
  activeProjectId,
  activeSessionId,
  scrollToBottom,
}: UseSessionLoaderProps) {
  const dispatch = useAppDispatch();
  const requestSeqRef = useRef(0);

  useEffect(() => {
    if (!projectsLoaded || !activeProjectId) return;

    if (!activeSessionId) {
      dispatch(clearChat());
      return;
    }

    const requestSeq = ++requestSeqRef.current;
    const requestedSessionId = activeSessionId;
    dispatch(resumeSession(requestedSessionId)).then((result) => {
      if (requestSeq !== requestSeqRef.current) return;
      if (!resumeSession.fulfilled.match(result)) {
        dispatch(setActiveSession(null));
      } else {
        const modelUsed = result.payload.meta.model_used;
        if (modelUsed) {
          dispatch(setDefaultModel(modelUsed));
        }
        // Scroll to bottom after session is loaded from backend
        setTimeout(() => scrollToBottom('auto'), 150);
      }
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId, scrollToBottom]);
}
