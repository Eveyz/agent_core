import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import chatReducer, { agentEventsBatch, resyncRun, clearPendingGap } from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer from './features/project/projectSlice';
import agentReducer from './features/agents/agentSlice';
import workflowReducer from './features/workflow/workflowSlice';

// ── Listener middleware (P2-1) ────────────────────────────────────────
// Replaces the Promise.resolve().then() side-effect that was inside the
// agentEventReceived reducer. The reducer now stores gap info in
// state.chat._pendingGap; this listener picks it up and dispatches resyncRun.
const listenerMiddleware = createListenerMiddleware();

listenerMiddleware.startListening({
  actionCreator: agentEventsBatch,
  effect: async (_, listenerApi) => {
    const state = listenerApi.getState() as RootState;
    const gap = state.chat._pendingGap;
    if (gap && !state.chat.resyncing) {
      listenerApi.dispatch(clearPendingGap());
      listenerApi.dispatch(resyncRun({ runId: gap.runId, fromSeq: gap.fromSeq }));
    }
  },
});

import { upsertProvider, deleteProvider, updateProvider, setAppearance, saveConfig } from './features/settings/settingsSlice';

listenerMiddleware.startListening({
  predicate: (action) => 
    upsertProvider.match(action) || 
    deleteProvider.match(action) || 
    updateProvider.match(action) ||
    setAppearance.match(action),
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
