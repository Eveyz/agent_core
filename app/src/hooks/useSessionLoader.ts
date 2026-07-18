import { useEffect, useRef } from 'react';
import { useAppDispatch } from './useAppDispatch';
import {
  resumeSession,
  setActiveSession,
} from '../features/project/projectSlice';

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

    // No active session — leave other sessions' in-memory caches intact.
    // New-session creation briefly nulls activeSessionId; clearing here would
    // wipe the previous session's cache mid-save.
    if (!activeSessionId) return;

    const requestSeq = ++requestSeqRef.current;
    const requestedSessionId = activeSessionId;
    dispatch(resumeSession(requestedSessionId)).then((result) => {
      if (requestSeq !== requestSeqRef.current) return;
      if (!resumeSession.fulfilled.match(result)) {
        dispatch(setActiveSession(null));
      } else {
        // Keep the global default_model (cross-session). Per-prompt models are
        // already restored on each message; do not overwrite the input selector.
        setTimeout(() => scrollToBottom('auto'), 150);
      }
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId, scrollToBottom]);
}
