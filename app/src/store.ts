import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import chatReducer, { agentEventsBatch, resyncRun, clearPendingGap, userMessageSent } from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer, { findProjectIdForSession, saveSessionMessages, fetchProjectSessions, touchSessionActivity } from './features/project/projectSlice';
import agentReducer from './features/agents/agentSlice';
import workflowReducer from './features/workflow/workflowSlice';
import previewReducer from './features/preview/previewSlice';

import { getFullMessagesForSession, getTimingMetrics } from './features/chat/chatSlice';

// ── Per-session save throttle ─────────────────────────────────────────
// Prevents overlapping saves from racing: the backend does DELETE + INSERT,
// so concurrent saves for the same session cause data loss (old data can
// overwrite newer data). We throttle to at most one save per session every
// SAVE_THROTTLE_MS.
const SAVE_THROTTLE_MS = 2_000;
const lastSaveBySession: Record<string, number> = {};

// ── Listener middleware (P2-1) ────────────────────────────────────────
// Replaces the Promise.resolve().then() side-effect that was inside the
// agentEventReceived reducer. The reducer now stores gap info in
// state.chat._pendingGap; this listener picks it up and dispatches resyncRun.
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
    const gap = state.chat._pendingGap;
    if (gap && !state.chat.resyncing) {
      listenerApi.dispatch(clearPendingGap());
      listenerApi.dispatch(resyncRun({ runId: gap.runId, fromSeq: gap.fromSeq }));
    }

    // Real-time intermediate saving: save when any tool or message (thinking) ends.
    const events = action.payload;
    const shouldSave = events.some((ev) => {
      if (typeof ev === 'object' && ev !== null) {
        const eventName = ev.event;
        return eventName === 'tool_ended' || eventName === 'message_end';
      }
      return false;
    });

    if (shouldSave && events.length > 0) {
      // Resolve which session these events belong to
      const firstEvent = events.find((ev) => typeof ev === 'object' && ev !== null && (ev.session_id || ev.run_id)) as Record<string, unknown> | undefined;
      if (!firstEvent) return;
      const runId = firstEvent.run_id as string;
      let targetSessionId: string | undefined = (firstEvent.session_id as string) || (state.chat.runIdToSessionId && state.chat.runIdToSessionId[runId]);
      if (!targetSessionId) {
        targetSessionId = state.project.activeSessionId ?? undefined;
      }

      if (targetSessionId) {
        // Throttle: skip if we saved this session recently
        const now = Date.now();
        const lastSave = lastSaveBySession[targetSessionId] ?? 0;
        if (now - lastSave < SAVE_THROTTLE_MS) return;
        lastSaveBySession[targetSessionId] = now;

        // Resolve project and path for this session
        let targetProjectPath: string | undefined;
        for (const [projectId, list] of Object.entries(state.project.sessions)) {
          if (list.some((s) => s.id === targetSessionId)) {
            const project = state.project.projects.find((p) => p.id === projectId);
            targetProjectPath = project?.path;
            break;
          }
        }
        if (!targetProjectPath) {
          const activeProject = state.project.projects.find((p) => p.id === state.project.activeProjectId);
          targetProjectPath = activeProject?.path;
        }

        const chatState = state.chat;
        const defaultModel = state.settings.config?.default_model || '';

        if (targetProjectPath) {
          const msgs = getFullMessagesForSession(chatState, targetSessionId);
          const entries = chatState.entries[targetSessionId] ?? [];
          const { processTimeMs, thoughtTimeMs } = getTimingMetrics(entries);
          if (msgs.length > 0) {
            listenerApi.dispatch(
              saveSessionMessages({
                sessionId: targetSessionId,
                messages: msgs,
                cwd: targetProjectPath,
                modelUsed: defaultModel,
                processTimeMs: processTimeMs || undefined,
                thoughtTimeMs: thoughtTimeMs || undefined,
              })
            );
          }
        }
      }
    }

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
