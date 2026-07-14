import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import chatReducer, { agentEventsBatch, resyncRun, clearPendingGap, userMessageSent } from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer, { findProjectIdForSession, fetchProjectSessions, touchSessionActivity } from './features/project/projectSlice';
import agentReducer from './features/agents/agentSlice';
import workflowReducer from './features/workflow/workflowSlice';
import previewReducer from './features/preview/previewSlice';

// ── Listener middleware (P2-1) ────────────────────────────────────────
// Replaces the Promise.resolve().then() side-effect that was inside the
// agentEventReceived reducer. The reducer now stores gap info in
// per-run pending gaps; this listener starts independent replay operations.
const listenerMiddleware = createListenerMiddleware();

listenerMiddleware.startListening({
  actionCreator: userMessageSent,
  effect: (action, listenerApi) => {
    const sessionId = action.payload.sessionId;
    if (sessionId) {
      listenerApi.dispatch(touchSessionActivity({ sessionId }));
    }
  },
});

listenerMiddleware.startListening({
  actionCreator: agentEventsBatch,
  effect: async (action, listenerApi) => {
    const state = listenerApi.getState() as RootState;
    for (const [runId, gap] of Object.entries(state.chat.pendingGapByRun)) {
      if (!state.chat.resyncingByRun[runId]) {
        listenerApi.dispatch(clearPendingGap(runId));
        listenerApi.dispatch(resyncRun({ runId, fromSeq: gap.fromSeq }));
      }
    }

    // Runtime snapshots and the terminal canonical commit own persistence.
    const events = action.payload;
    // ── Refresh session list from DB on run completion (final consistency) ──
    const completionEvents = events.filter((ev) => {
      if (typeof ev !== 'object' || ev === null) return false;
      const name = (ev as Record<string, unknown>).event;
      return name === 'run_completed' || name === 'run_cancelled' || name === 'run_failed';
    });
    if (completionEvents.length > 0) {
      const currentState = listenerApi.getState() as RootState;
      const projectIds = new Set<string>();
      for (const ev of completionEvents) {
        const record = ev as Record<string, unknown>;
        const runId = record.run_id as string | undefined;
        const sessionId =
          (record.session_id as string | undefined)
          || (runId ? currentState.chat.runIdToSessionId?.[runId] : undefined)
          || currentState.project.activeSessionId
          || undefined;
        if (!sessionId) continue;
        const projectId =
          findProjectIdForSession(currentState.project.sessions, sessionId)
          ?? currentState.project.activeProjectId;
        if (projectId) projectIds.add(projectId);
      }
      for (const projectId of projectIds) {
        listenerApi.dispatch(fetchProjectSessions(projectId));
      }
    }
  },
});

import { upsertProvider, deleteProvider, updateProvider, setAppearance, saveConfig, upsertMcpServer, deleteMcpServer, toggleMcpServer } from './features/settings/settingsSlice';

listenerMiddleware.startListening({
  predicate: (action) => 
    upsertProvider.match(action) || 
    deleteProvider.match(action) || 
    updateProvider.match(action) ||
    setAppearance.match(action) ||
    upsertMcpServer.match(action) ||
    deleteMcpServer.match(action) ||
    toggleMcpServer.match(action),
  effect: async (_, listenerApi) => {
    const state = listenerApi.getState() as RootState;
    if (state.settings.config) {
      // Auto-save whenever config-altering actions are dispatched.
      // Notice we don't await here to avoid blocking, the thunk handles the async call.
      listenerApi.dispatch(saveConfig(state.settings.config));
    }
  },
});

export const store = configureStore({
  reducer: {
    chat: chatReducer,
    settings: settingsReducer,
    project: projectReducer,
    agents: agentReducer,
    workflow: workflowReducer,
    preview: previewReducer,
  },
  middleware: (getDefaultMiddleware) =>
    getDefaultMiddleware().prepend(listenerMiddleware.middleware),
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;
