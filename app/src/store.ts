import { configureStore, createListenerMiddleware } from '@reduxjs/toolkit';
import chatReducer, { agentEventReceived, resyncRun, clearPendingGap } from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer from './features/project/projectSlice';

// ── Listener middleware (P2-1) ────────────────────────────────────────
// Replaces the Promise.resolve().then() side-effect that was inside the
// agentEventReceived reducer. The reducer now stores gap info in
// state.chat._pendingGap; this listener picks it up and dispatches resyncRun.
const listenerMiddleware = createListenerMiddleware();

listenerMiddleware.startListening({
  actionCreator: agentEventReceived,
  effect: async (_, listenerApi) => {
    const state = listenerApi.getState() as RootState;
    const gap = state.chat._pendingGap;
    if (gap && !state.chat.resyncing) {
      listenerApi.dispatch(clearPendingGap());
      listenerApi.dispatch(resyncRun({ runId: gap.runId, fromSeq: gap.fromSeq }));
    }
  },
});

export const store = configureStore({
  reducer: {
    chat: chatReducer,
    settings: settingsReducer,
    project: projectReducer,
  },
  middleware: (getDefaultMiddleware) =>
    getDefaultMiddleware().prepend(listenerMiddleware.middleware),
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;
