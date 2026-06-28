import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';

export interface Project {
  id: string;
  name: string;
  path: string;
  created_at: string;
  updated_at: string;
}

export interface SessionMeta {
  id: string;
  title: string;
  summary: string;
  start_time: string;
  end_time: string | null;
  message_count: number;
  cwd: string;
  model_used: string;
  tags: string[];
  archived: boolean;
  parent_session_id: string | null;
  session_type: string;
  process_time_ms: number;
  thought_time_ms: number;
  created_at: string;
  updated_at: string;
}

export interface FrontendMessage {
  role: string;
  content: string;
}

export interface EventLogEntry {
  turn_index: number;
  event_type: string;
  payload: unknown;
  started_at: string | null;
  ended_at: string | null;
}

export interface ResumeSessionResult {
  meta: SessionMeta;
  messages: FrontendMessage[];
  event_log: EventLogEntry[];
}

interface ProjectState {
  projects: Project[];
  sessions: Record<string, SessionMeta[]>; // project_id -> sessions
  activeProjectId: string | null;
  activeSessionId: string | null;
  sessionMessages: FrontendMessage[];  // messages of current session
  loading: boolean;
  error: string | null;
}

const STORAGE_KEY = 'agent_core_active_project';
const SESSION_KEY = 'agent_core_active_session';

const savedActiveId = localStorage.getItem(STORAGE_KEY);
const savedActiveSessionId = localStorage.getItem(SESSION_KEY);

const initialState: ProjectState = {
  projects: [],
  sessions: {},
  activeProjectId: savedActiveId,
  activeSessionId: savedActiveSessionId,
  sessionMessages: [],
  loading: false,
  error: null,
};

// ── Project thunks ───────────────────────────────────────────────────

export const fetchProjects = createAsyncThunk('project/fetchProjects', async (_, { rejectWithValue }) => {
  try {
    const raw = await invoke<Record<string, unknown>[]>('list_projects');
    return raw.map(normalizeProject);
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const createProject = createAsyncThunk('project/createProject', async (path: string, { rejectWithValue }) => {
  try {
    const raw = await invoke<Record<string, unknown>>('create_project', { path });
    return normalizeProject(raw);
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const deleteProject = createAsyncThunk('project/deleteProject', async (projectId: string, { rejectWithValue }) => {
  try {
    await invoke('delete_project', { projectId });
    return projectId;
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const renameProject = createAsyncThunk(
  'project/renameProject',
  async ({ projectId, newName }: { projectId: string; newName: string }, { rejectWithValue }) => {
    try {
      await invoke('rename_project', { projectId, newName });
      return { projectId, newName };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const fetchProjectSessions = createAsyncThunk('project/fetchProjectSessions', async (projectId: string, { rejectWithValue }) => {
  try {
    const raw = await invoke<Record<string, unknown>[]>('get_project_sessions', { projectId });
    return { projectId, sessions: raw.map(normalizeSession) };
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

// ── Session thunks ───────────────────────────────────────────────────

export const createSession = createAsyncThunk(
  'project/createSession',
  async (projectId: string, { rejectWithValue }) => {
    try {
      const raw = await invoke<Record<string, unknown>>('create_session', { projectId });
      return { projectId, session: normalizeSession(raw) };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const deleteSession = createAsyncThunk(
  'project/deleteSession',
  async ({ sessionId, projectId }: { sessionId: string; projectId: string }, { rejectWithValue }) => {
    try {
      await invoke('delete_session', { sessionId });
      return { sessionId, projectId };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const renameSession = createAsyncThunk(
  'project/renameSession',
  async ({ sessionId, projectId, newTitle }: { sessionId: string; projectId: string; newTitle: string }, { rejectWithValue }) => {
    try {
      await invoke('rename_session', { sessionId, newTitle });
      return { sessionId, projectId, newTitle };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const saveSessionMessages = createAsyncThunk(
  'project/saveSessionMessages',
  async ({ sessionId, messages, cwd, modelUsed, processTimeMs, thoughtTimeMs, eventLog }: {
    sessionId: string;
    messages: FrontendMessage[];
    cwd: string;
    modelUsed: string;
    processTimeMs?: number;
    thoughtTimeMs?: number;
    eventLog?: unknown[];
  }, { rejectWithValue }) => {
    try {
      await invoke('save_session_messages', {
        sessionId,
        messagesJson: JSON.stringify(messages),
        cwd,
        modelUsed,
        processTimeMs: processTimeMs ?? null,
        thoughtTimeMs: thoughtTimeMs ?? null,
        eventLogJson: eventLog ? JSON.stringify(eventLog) : null,
      });
      return { sessionId, messageCount: messages.length };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const resumeSession = createAsyncThunk(
  'project/resumeSession',
  async (sessionId: string, { rejectWithValue }) => {
    try {
      const result = await invoke<ResumeSessionResult>('resume_session', { sessionId });
      return result;
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

// ── Normalizers ──────────────────────────────────────────────────────

function normalizeProject(raw: Record<string, unknown>): Project {
  return {
    id: (raw.id as string) ?? '',
    name: (raw.name as string) ?? '',
    path: (raw.path as string) ?? '',
    created_at: (raw.created_at as string) ?? '',
    updated_at: (raw.updated_at as string) ?? '',
  };
}

function normalizeSession(raw: Record<string, unknown>): SessionMeta {
  return {
    id: (raw.id as string) ?? '',
    title: (raw.title as string) ?? '',
    summary: (raw.summary as string) ?? '',
    start_time: (raw.start_time as string) ?? '',
    end_time: (raw.end_time as string) ?? null,
    message_count: (raw.message_count as number) ?? 0,
    cwd: (raw.cwd as string) ?? '',
    model_used: (raw.model_used as string) ?? '',
    tags: (raw.tags as string[]) ?? [],
    archived: (raw.archived as boolean) ?? false,
    parent_session_id: (raw.parent_session_id as string) ?? null,
    session_type: (raw.session_type as string) ?? 'main',
    process_time_ms: (raw.process_time_ms as number) ?? 0,
    thought_time_ms: (raw.thought_time_ms as number) ?? 0,
    created_at: (raw.created_at as string) ?? '',
    updated_at: (raw.updated_at as string) ?? '',
  };
}

// ── Slice ────────────────────────────────────────────────────────────

export const projectSlice = createSlice({
  name: 'project',
  initialState,
  reducers: {
    setActiveProject: (state, action: PayloadAction<string | null>) => {
      if (state.activeProjectId === action.payload) return;
      state.activeProjectId = action.payload;
      // Clear session when switching projects
      state.activeSessionId = null;
      state.sessionMessages = [];
      if (action.payload) {
        localStorage.setItem(STORAGE_KEY, action.payload);
        localStorage.removeItem(SESSION_KEY);
      } else {
        localStorage.removeItem(STORAGE_KEY);
        localStorage.removeItem(SESSION_KEY);
      }
    },
    setActiveSession: (state, action: PayloadAction<string | null>) => {
      state.activeSessionId = action.payload;
      if (action.payload) {
        localStorage.setItem(SESSION_KEY, action.payload);
      } else {
        localStorage.removeItem(SESSION_KEY);
      }
    },
    clearSessionMessages: (state) => {
      state.sessionMessages = [];
    },
    setSessionMessages: (state, action: PayloadAction<FrontendMessage[]>) => {
      state.sessionMessages = action.payload;
    },
  },
  extraReducers: (builder) => {
    builder
      // ── Projects ──
      .addCase(fetchProjects.pending, (state) => {
        state.loading = true;
        state.error = null;
      })
      .addCase(fetchProjects.fulfilled, (state, action) => {
        state.loading = false;
        state.projects = action.payload;
        if (state.activeProjectId) {
          const stillExists = state.projects.some((p) => p.id === state.activeProjectId);
          if (!stillExists) {
            state.activeProjectId = state.projects[0]?.id ?? null;
            state.activeSessionId = null;
            state.sessionMessages = [];
            if (state.activeProjectId) {
              localStorage.setItem(STORAGE_KEY, state.activeProjectId);
            } else {
              localStorage.removeItem(STORAGE_KEY);
              localStorage.removeItem(SESSION_KEY);
            }
          }
        } else if (state.projects.length > 0) {
          state.activeProjectId = state.projects[0].id;
          localStorage.setItem(STORAGE_KEY, state.projects[0].id);
        }
      })
      .addCase(fetchProjects.rejected, (state, action) => {
        state.loading = false;
        state.error = action.payload as string;
      })
      .addCase(createProject.fulfilled, (state, action) => {
        state.projects.unshift(action.payload);
        state.activeProjectId = action.payload.id;
        state.activeSessionId = null;
        state.sessionMessages = [];
        localStorage.setItem(STORAGE_KEY, action.payload.id);
        localStorage.removeItem(SESSION_KEY);
      })
      .addCase(deleteProject.fulfilled, (state, action) => {
        state.projects = state.projects.filter((p) => p.id !== action.payload);
        delete state.sessions[action.payload];
        if (state.activeProjectId === action.payload) {
          state.activeProjectId = state.projects[0]?.id ?? null;
          state.activeSessionId = null;
          state.sessionMessages = [];
          if (state.activeProjectId) {
            localStorage.setItem(STORAGE_KEY, state.activeProjectId);
          } else {
            localStorage.removeItem(STORAGE_KEY);
            localStorage.removeItem(SESSION_KEY);
          }
        }
      })
      .addCase(renameProject.fulfilled, (state, action) => {
        const { projectId, newName } = action.payload;
        const p = state.projects.find((p) => p.id === projectId);
        if (p) p.name = newName;
      })
      // ── Sessions listing ──
      .addCase(fetchProjectSessions.fulfilled, (state, action) => {
        state.sessions[action.payload.projectId] = action.payload.sessions.sort(
          (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime()
        );
      })
      // ── Session CRUD ──
      .addCase(createSession.fulfilled, (state, action) => {
        const { projectId, session } = action.payload;
        if (!state.sessions[projectId]) {
          state.sessions[projectId] = [];
        }
        state.sessions[projectId].unshift(session);
        state.activeProjectId = projectId;
        state.activeSessionId = session.id;
        state.sessionMessages = [];
        localStorage.setItem(STORAGE_KEY, projectId);
        localStorage.setItem(SESSION_KEY, session.id);
      })
      .addCase(deleteSession.fulfilled, (state, action) => {
        const { sessionId, projectId } = action.payload;
        if (state.sessions[projectId]) {
          state.sessions[projectId] = state.sessions[projectId].filter((s) => s.id !== sessionId);
        }
        if (state.activeSessionId === sessionId) {
          state.activeSessionId = state.sessions[projectId]?.[0]?.id ?? null;
          state.sessionMessages = [];
          if (state.activeSessionId) {
            localStorage.setItem(SESSION_KEY, state.activeSessionId);
          } else {
            localStorage.removeItem(SESSION_KEY);
          }
        }
      })
      .addCase(renameSession.fulfilled, (state, action) => {
        const { sessionId, projectId, newTitle } = action.payload;
        if (state.sessions[projectId]) {
          const s = state.sessions[projectId].find((s) => s.id === sessionId);
          if (s) {
            s.title = newTitle;
            s.updated_at = new Date().toISOString();
          }
        }
      })
      .addCase(saveSessionMessages.fulfilled, (state, action) => {
        const sessionId = action.payload.sessionId;
        for (const [projectId, list] of Object.entries(state.sessions)) {
          const s = list.find((s) => s.id === sessionId);
          if (s) {
            s.message_count = action.payload.messageCount;
            s.updated_at = new Date().toISOString();
            // Re-sort sessions by updated_at to move active session to top
            state.sessions[projectId] = list.sort(
              (a, b) => new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime()
            );
            break;
          }
        }
      })
      .addCase(resumeSession.fulfilled, (state, action) => {
        state.sessionMessages = action.payload.messages;
      });
  },
});

export const { setActiveProject, setActiveSession, clearSessionMessages, setSessionMessages } = projectSlice.actions;
export default projectSlice.reducer;
