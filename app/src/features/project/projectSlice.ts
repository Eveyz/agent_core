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
  created_at: string;
  updated_at: string;
}

interface ProjectState {
  projects: Project[];
  sessions: Record<string, SessionMeta[]>; // project_id -> sessions
  activeProjectId: string | null;
  loading: boolean;
  error: string | null;
}

const STORAGE_KEY = 'agent_core_active_project';

const savedActiveId = localStorage.getItem(STORAGE_KEY);

const initialState: ProjectState = {
  projects: [],
  sessions: {},
  activeProjectId: savedActiveId,
  loading: false,
  error: null,
};

export const fetchProjects = createAsyncThunk('project/fetchProjects', async (_, { rejectWithValue }) => {
  try {
    const raw = await invoke<any[]>('list_projects');
    return raw.map(normalizeProject);
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const createProject = createAsyncThunk('project/createProject', async (path: string, { rejectWithValue }) => {
  try {
    const raw = await invoke<any>('create_project', { path });
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
    const raw = await invoke<any[]>('get_project_sessions', { projectId });
    return { projectId, sessions: raw.map(normalizeSession) };
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

function normalizeProject(raw: any): Project {
  return {
    id: raw?.id ?? '',
    name: raw?.name ?? '',
    path: raw?.path ?? '',
    created_at: raw?.created_at ?? '',
    updated_at: raw?.updated_at ?? '',
  };
}

function normalizeSession(raw: any): SessionMeta {
  return {
    id: raw?.id ?? '',
    title: raw?.title ?? '',
    summary: raw?.summary ?? '',
    start_time: raw?.start_time ?? '',
    end_time: raw?.end_time ?? null,
    message_count: raw?.message_count ?? 0,
    cwd: raw?.cwd ?? '',
    model_used: raw?.model_used ?? '',
    tags: raw?.tags ?? [],
    archived: raw?.archived ?? false,
    parent_session_id: raw?.parent_session_id ?? null,
    session_type: raw?.session_type ?? 'main',
    created_at: raw?.created_at ?? '',
    updated_at: raw?.updated_at ?? '',
  };
}

export const projectSlice = createSlice({
  name: 'project',
  initialState,
  reducers: {
    setActiveProject: (state, action: PayloadAction<string | null>) => {
      state.activeProjectId = action.payload;
      if (action.payload) {
        localStorage.setItem(STORAGE_KEY, action.payload);
      } else {
        localStorage.removeItem(STORAGE_KEY);
      }
    },
  },
  extraReducers: (builder) => {
    builder
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
            if (state.activeProjectId) {
              localStorage.setItem(STORAGE_KEY, state.activeProjectId);
            } else {
              localStorage.removeItem(STORAGE_KEY);
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
        localStorage.setItem(STORAGE_KEY, action.payload.id);
      })
      .addCase(deleteProject.fulfilled, (state, action) => {
        state.projects = state.projects.filter((p) => p.id !== action.payload);
        delete state.sessions[action.payload];
        if (state.activeProjectId === action.payload) {
          state.activeProjectId = state.projects[0]?.id ?? null;
          if (state.activeProjectId) {
            localStorage.setItem(STORAGE_KEY, state.activeProjectId);
          } else {
            localStorage.removeItem(STORAGE_KEY);
          }
        }
      })
      .addCase(renameProject.fulfilled, (state, action) => {
        const { projectId, newName } = action.payload;
        const p = state.projects.find((p) => p.id === projectId);
        if (p) p.name = newName;
      })
      .addCase(fetchProjectSessions.fulfilled, (state, action) => {
        state.sessions[action.payload.projectId] = action.payload.sessions;
      });
  },
});

export const { setActiveProject } = projectSlice.actions;
export default projectSlice.reducer;
