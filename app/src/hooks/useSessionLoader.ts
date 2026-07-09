import { useEffect, useRef } from 'react';
import { useAppDispatch } from './useAppDispatch';
import {
  resumeSession,
  setActiveSession,
} from '../features/project/projectSlice';
import { setDefaultModel } from '../features/settings/settingsSlice';

/** Session placeholders stored before a real model is chosen; don't overwrite global default. */
const PLACEHOLDER_SESSION_MODELS = new Set(['default', 'unknown']);

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
        const modelUsed = result.payload.meta.model_used;
        if (modelUsed && !PLACEHOLDER_SESSION_MODELS.has(modelUsed)) {
          dispatch(setDefaultModel(modelUsed));
        }
        // Scroll to bottom after session is loaded from backend
        setTimeout(() => scrollToBottom('auto'), 150);
      }
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId, scrollToBottom]);
}
