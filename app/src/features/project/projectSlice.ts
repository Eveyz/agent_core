import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';

export interface Project {
  id: string;
  name: string;
  path: string;
  pinned?: boolean;
  pinned_at?: string;
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
  pinned_goal?: string | null;
  goal_completed?: boolean;
  pinned?: boolean;
  pinned_at?: string;
  created_at: string;
  updated_at: string;
}

export interface FrontendMessage {
  role: string;
  content: string;
  /** Model that ran this prompt; persisted per-message so each restored
   * message shows its own model instead of the global current one. */
  model?: string;
  tool_calls?: any[];
  tool_call_id?: string;
  name?: string;
  /** Prompt this message belongs to (from backend prompts table). */
  prompt_id?: string;
  metadata?: Record<string, any>;
}

export interface FrontendPrompt {
  id: string;
  session_id: string;
  turn_index: number;
  model: string;
  status: string;
  token_usage: Record<string, unknown>;
  started_at: string | null;
  ended_at: string | null;
  created_at: string;
  messages: FrontendMessage[];
}

export interface ResumeSessionResult {
  meta: SessionMeta;
  messages: FrontendMessage[];
  prompts: FrontendPrompt[];
}

interface ProjectState {
  projects: Project[];
  sessions: Record<string, SessionMeta[]>; // project_id -> sessions
  activeProjectId: string | null;
  activeSessionId: string | null;
  loading: boolean;
  error: string | null;
}

const STORAGE_KEY = 'agent_core_active_project';
const SESSION_KEY = 'agent_core_active_session';

const savedActiveId = localStorage.getItem(STORAGE_KEY);
const savedActiveSessionId = localStorage.getItem(SESSION_KEY);

const saveQueueBySession: Record<string, Promise<unknown>> = {};
const saveGenerationBySession: Record<string, number> = {};
const lastAppliedSaveGenerationBySession: Record<string, number> = {};

/** Sort sessions by last activity (newest first), stable tie-breaker on id. */
export function sortSessionsByActivity(sessions: SessionMeta[]): SessionMeta[] {
  return [...sessions].sort((a, b) => {
    const diff = new Date(b.updated_at).getTime() - new Date(a.updated_at).getTime();
    if (diff !== 0) return diff;
    return b.id.localeCompare(a.id);
  });
}

/** Find which project owns a session in the loaded sidebar cache. */
export function findProjectIdForSession(
  sessions: Record<string, SessionMeta[]>,
  sessionId: string,
): string | null {
  for (const [projectId, list] of Object.entries(sessions)) {
    if (list.some((s) => s.id === sessionId)) return projectId;
  }
  return null;
}

function patchSessionActivity(
  state: ProjectState,
  sessionId: string,
  updatedAt: string,
  messageCount?: number,
): void {
  for (const [projectId, list] of Object.entries(state.sessions)) {
    const session = list.find((s) => s.id === sessionId);
    if (!session) continue;
    const incoming = new Date(updatedAt).getTime();
    const current = new Date(session.updated_at).getTime();
    if (incoming >= current) {
      session.updated_at = updatedAt;
    }
    if (messageCount !== undefined) {
      session.message_count = messageCount;
    }
    state.sessions[projectId] = sortSessionsByActivity(list);
    return;
  }
}

const initialState: ProjectState = {
  projects: [],
  sessions: {},
  activeProjectId: savedActiveId,
  activeSessionId: savedActiveSessionId,
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

export const createNewProject = createAsyncThunk(
  'project/createNewProject',
  async ({ name, path }: { name: string; path: string }, { rejectWithValue }) => {
    try {
      const raw = await invoke<Record<string, unknown>>('create_new_project', { name, path });
      return normalizeProject(raw);
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const setProjectPinned = createAsyncThunk(
  'project/setProjectPinned',
  async ({ projectId, pinned }: { projectId: string; pinned: boolean }, { rejectWithValue }) => {
    try {
      await invoke('set_project_pinned', { projectId, pinned });
      return { projectId, pinned, pinnedAt: pinned ? new Date().toISOString() : '' };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const setSessionPinned = createAsyncThunk(
  'project/setSessionPinned',
  async (
    { sessionId, projectId, pinned }: { sessionId: string; projectId: string; pinned: boolean },
    { rejectWithValue }
  ) => {
    try {
      await invoke('set_session_pinned', { sessionId, pinned });
      return { sessionId, projectId, pinned, pinnedAt: pinned ? new Date().toISOString() : '' };
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

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
  async ({ sessionId, messages, cwd, modelUsed, processTimeMs, thoughtTimeMs }: {
    sessionId: string;
    messages: FrontendMessage[];
    cwd: string;
    modelUsed: string;
    processTimeMs?: number;
    thoughtTimeMs?: number;
  }, { rejectWithValue }) => {
    const generation = (saveGenerationBySession[sessionId] ?? 0) + 1;
    saveGenerationBySession[sessionId] = generation;
    const previous = saveQueueBySession[sessionId] ?? Promise.resolve();
    try {
      const task = previous
        .catch(() => undefined)
        .then(() => invoke<{ updated_at: string }>('save_session_messages', {
          sessionId,
          messagesJson: JSON.stringify(messages),
          cwd,
          modelUsed,
          processTimeMs: processTimeMs ?? null,
          thoughtTimeMs: thoughtTimeMs ?? null,
        }));
      saveQueueBySession[sessionId] = task;
      const result = await task;
      if (saveQueueBySession[sessionId] === task) {
        delete saveQueueBySession[sessionId];
      }
      return { sessionId, messageCount: messages.length, updated_at: result.updated_at, generation };
    } catch (e) {
      if (saveGenerationBySession[sessionId] === generation) {
        delete saveQueueBySession[sessionId];
      }
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
    pinned: Boolean(raw.pinned),
    pinned_at: (raw.pinned_at as string) ?? '',
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
    pinned_goal: (raw.pinned_goal as string | null | undefined) ?? null,
    goal_completed: Boolean(raw.goal_completed),
    pinned: Boolean(raw.pinned),
    pinned_at: (raw.pinned_at as string) ?? '',
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
    /** Optimistic sidebar bump when the user sends a message (before DB save). */
    touchSessionActivity: (state, action: PayloadAction<{ sessionId: string; updatedAt?: string }>) => {
      patchSessionActivity(state, action.payload.sessionId, action.payload.updatedAt ?? new Date().toISOString());
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
            const nextProject = state.projects.find((p) => p.id !== '__adhoc_chat__') || state.projects[0];
            state.activeProjectId = nextProject?.id ?? null;
            state.activeSessionId = null;
            if (state.activeProjectId) {
              localStorage.setItem(STORAGE_KEY, state.activeProjectId);
            } else {
              localStorage.removeItem(STORAGE_KEY);
              localStorage.removeItem(SESSION_KEY);
            }
          }
        } else if (state.projects.length > 0) {
          const nextProject = state.projects.find((p) => p.id !== '__adhoc_chat__') || state.projects[0];
          state.activeProjectId = nextProject.id;
          localStorage.setItem(STORAGE_KEY, nextProject.id);
        }
      })
      .addCase(fetchProjects.rejected, (state, action) => {
        state.loading = false;
        state.error = action.payload as string;
      })
      .addCase(createProject.fulfilled, (state, action) => {
        const existing = state.projects.find((p) => p.id === action.payload.id);
        if (!existing) {
          state.projects.unshift(action.payload);
        }
        state.activeProjectId = action.payload.id;
        state.activeSessionId = null;
        localStorage.setItem(STORAGE_KEY, action.payload.id);
        localStorage.removeItem(SESSION_KEY);
      })
      .addCase(createNewProject.fulfilled, (state, action) => {
        const existing = state.projects.find((p) => p.id === action.payload.id);
        if (!existing) {
          state.projects.unshift(action.payload);
        }
        state.activeProjectId = action.payload.id;
        state.activeSessionId = null;
        localStorage.setItem(STORAGE_KEY, action.payload.id);
        localStorage.removeItem(SESSION_KEY);
      })
      .addCase(setProjectPinned.fulfilled, (state, action) => {
        const p = state.projects.find((proj) => proj.id === action.payload.projectId);
        if (p) {
          p.pinned = action.payload.pinned;
          p.pinned_at = action.payload.pinnedAt;
        }
      })
      .addCase(setSessionPinned.fulfilled, (state, action) => {
        const { sessionId, projectId, pinned, pinnedAt } = action.payload;
        const list = state.sessions[projectId];
        if (!list) return;
        const session = list.find((s) => s.id === sessionId);
        if (session) {
          session.pinned = pinned;
          session.pinned_at = pinnedAt;
        }
      })
      .addCase(deleteProject.fulfilled, (state, action) => {
        state.projects = state.projects.filter((p) => p.id !== action.payload);
        delete state.sessions[action.payload];
        if (state.activeProjectId === action.payload) {
          state.activeProjectId = state.projects[0]?.id ?? null;
          state.activeSessionId = null;
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
        state.sessions[action.payload.projectId] = sortSessionsByActivity(action.payload.sessions);
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
          }
        }
      })
      .addCase(saveSessionMessages.fulfilled, (state, action) => {
        const { sessionId, messageCount, updated_at, generation } = action.payload;
        const latestGeneration = saveGenerationBySession[sessionId] ?? 0;
        if (generation < latestGeneration) return;
        lastAppliedSaveGenerationBySession[sessionId] = generation;
        patchSessionActivity(state, sessionId, updated_at, messageCount);
      });
  },
});

export function __testSetSaveGeneration(sessionId: string, generation: number): void {
  saveGenerationBySession[sessionId] = generation;
}

export const {
  setActiveProject,
  setActiveSession,
  touchSessionActivity,
} = projectSlice.actions;
export default projectSlice.reducer;
