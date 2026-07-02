import { useEffect } from 'react';
import { useAppDispatch } from './useAppDispatch';
import {
  fetchProjectSessions,
  resumeSession,
  setActiveSession,
} from '../features/project/projectSlice';
import { clearChat, restoreOrClearSession } from '../features/chat/chatSlice';

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

  useEffect(() => {
    if (!projectsLoaded || !activeProjectId) return;

    if (!activeSessionId) {
      dispatch(clearChat());
      return;
    }

    dispatch(fetchProjectSessions(activeProjectId));
    dispatch(restoreOrClearSession(activeSessionId));
    dispatch(resumeSession(activeSessionId)).then((result) => {
      if (!resumeSession.fulfilled.match(result)) {
        dispatch(setActiveSession(null));
      } else {
        // Scroll to bottom after session is loaded from backend
        setTimeout(() => scrollToBottom('auto'), 150);
      }
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId, scrollToBottom]);
}
