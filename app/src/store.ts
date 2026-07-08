import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import chatReducer, { agentEventsBatch, resyncRun, clearPendingGap } from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer from './features/project/projectSlice';
import agentReducer from './features/agents/agentSlice';
import workflowReducer from './features/workflow/workflowSlice';

import { getFullMessages, getTimingMetrics } from './features/chat/chatSlice';
import { saveSessionMessages } from './features/project/projectSlice';

// ── Listener middleware (P2-1) ────────────────────────────────────────
// Replaces the Promise.resolve().then() side-effect that was inside the
// agentEventReceived reducer. The reducer now stores gap info in
// state.chat._pendingGap; this listener picks it up and dispatches resyncRun.
const listenerMiddleware = createListenerMiddleware();

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

    if (shouldSave) {
      const chatState = state.chat;
      const activeSessionId = state.project.activeSessionId;
      const activeProjectId = state.project.activeProjectId;
      const activeProject = state.project.projects.find((p) => p.id === activeProjectId);
      const activeProjectPath = activeProject?.path;
      const defaultModel = state.settings.config?.default_model || '';

      if (activeSessionId && activeProjectPath) {
        const msgs = getFullMessages(chatState);
        const { processTimeMs, thoughtTimeMs } = getTimingMetrics(chatState.entries);
        if (msgs.length > 0) {
          listenerApi.dispatch(
            saveSessionMessages({
              sessionId: activeSessionId,
              messages: msgs,
              cwd: activeProjectPath,
              modelUsed: defaultModel,
              processTimeMs: processTimeMs || undefined,
              thoughtTimeMs: thoughtTimeMs || undefined,
            })
          );
        }
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
  },
  middleware: (getDefaultMiddleware) =>
    getDefaultMiddleware().prepend(listenerMiddleware.middleware),
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;
