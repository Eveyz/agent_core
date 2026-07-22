import { useEffect, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { useAppDispatch } from './useAppDispatch';
import {
  resumeSession,
  setActiveSession,
} from '../features/project/projectSlice';
import { plansHydrated } from '../features/chat/chatSlice';
import type { ParkedPlan, PlanDetail, TodoItem } from '../features/chat/types';

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
    dispatch(resumeSession(requestedSessionId)).then(async (result) => {
      if (requestSeq !== requestSeqRef.current) return;
      if (!resumeSession.fulfilled.match(result)) {
        dispatch(setActiveSession(null));
        return;
      }
      // Keep the global default_model (cross-session). Per-prompt models are
      // already restored on each message; do not overwrite the input selector.
      try {
        const dto = await invoke<{
          items: TodoItem[];
          parked: ParkedPlan[];
          plans?: PlanDetail[];
          active_plan_id?: string | null;
          active_plan_title?: string | null;
        }>('get_session_plans', { sessionId: requestedSessionId });
        if (requestSeq !== requestSeqRef.current) return;
        dispatch(
          plansHydrated({
            sessionId: requestedSessionId,
            items: dto.items ?? [],
            parked: dto.parked ?? [],
            plans: dto.plans ?? [],
            activePlanId: dto.active_plan_id ?? null,
            activePlanTitle: dto.active_plan_title ?? null,
          }),
        );
      } catch (e) {
        console.warn('get_session_plans failed', e);
      }
      setTimeout(() => scrollToBottom('auto'), 150);
    });
  }, [projectsLoaded, dispatch, activeProjectId, activeSessionId, scrollToBottom]);
}
