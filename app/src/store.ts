import { configureStore } from '@reduxjs/toolkit';
import chatReducer from './features/chat/chatSlice';
import settingsReducer from './features/settings/settingsSlice';
import projectReducer from './features/project/projectSlice';

export const store = configureStore({
  reducer: {
    chat: chatReducer,
    settings: settingsReducer,
    project: projectReducer,
  },
});

export type RootState = ReturnType<typeof store.getState>;
export type AppDispatch = typeof store.dispatch;
