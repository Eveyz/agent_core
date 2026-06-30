import { createSlice, createAsyncThunk, type PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import type { AgentDef, AgentHistoryEntry, AgentMemoryRecord } from './types';

// ── Thunks ──────────────────────────────────────────────────────────

export const fetchAgents = createAsyncThunk<AgentDef[]>('agents/fetchAll', async () => {
  return invoke<AgentDef[]>('list_agents');
});

export interface CreateAgentParams {
  name: string;
  description?: string;
  system_prompt?: string;
  model?: string;
  skills?: string[];
  tools?: string[];
  permission_mode?: string;
  max_iterations?: number;
  max_context_tokens?: number;
  memory_enabled?: number;
  memory_group?: string;
  icon?: string;
  color?: string;
}

export const createAgent = createAsyncThunk<AgentDef, CreateAgentParams>(
  'agents/create',
  async (params) => invoke<AgentDef>('create_agent', params as unknown as Record<string, unknown>),
);

export interface UpdateAgentParams {
  id: string;
  name?: string;
  description?: string;
  system_prompt?: string;
  model?: string;
  skills?: string[];
  tools?: string[];
  permission_mode?: string;
  permission_rules?: unknown;
  max_iterations?: number;
  max_context_tokens?: number;
  memory_enabled?: number;
  memory_group?: string;
  icon?: string;
  color?: string;
}

export const updateAgent = createAsyncThunk<AgentDef, UpdateAgentParams>(
  'agents/update',
  async (params) => {
    const { id, ...rest } = params;
    return invoke<AgentDef>('update_agent', { id, ...rest });
  },
);

export const deleteAgent = createAsyncThunk<string, string>(
  'agents/delete',
  async (id) => {
    await invoke('delete_agent', { id });
    return id;
  },
);

export const runAgentStandalone = createAsyncThunk<string, { agentId: string; input: string; sessionId?: string }>(
  'agents/runStandalone',
  async ({ agentId, input, sessionId }) =>
    invoke<string>('run_agent_standalone', { agentId, input, sessionId }),
);

export const searchAgentMemory = createAsyncThunk<
  AgentMemoryRecord[],
  { agentId: string; query: string; topK?: number }
>('agents/searchMemory', async ({ agentId, query, topK }) =>
  invoke<AgentMemoryRecord[]>('search_agent_memory', { agentId, query, topK }),
);

export const fetchAgentHistory = createAsyncThunk<
  AgentHistoryEntry[],
  { agentId: string; limit?: number }
>('agents/fetchHistory', async ({ agentId, limit }) =>
  invoke<AgentHistoryEntry[]>('get_agent_history', { agentId, limit }),
);

// ── Slice ───────────────────────────────────────────────────────────

interface AgentState {
  agents: AgentDef[];
  loading: boolean;
  error: string | null;
  selectedAgentId: string | null;
  history: AgentHistoryEntry[];
  memories: AgentMemoryRecord[];
  running: boolean;
  runOutput: string | null;
}

const initialState: AgentState = {
  agents: [],
  loading: false,
  error: null,
  selectedAgentId: null,
  history: [],
  memories: [],
  running: false,
  runOutput: null,
};

const agentSlice = createSlice({
  name: 'agents',
  initialState,
  reducers: {
    setSelectedAgent(state, action: PayloadAction<string | null>) {
      state.selectedAgentId = action.payload;
    },
    clearAgentRun(state) {
      state.running = false;
      state.runOutput = null;
    },
    clearAgentError(state) {
      state.error = null;
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(fetchAgents.pending, (state) => {
        state.loading = true;
        state.error = null;
      })
      .addCase(fetchAgents.fulfilled, (state, action) => {
        state.loading = false;
        state.agents = action.payload;
      })
      .addCase(fetchAgents.rejected, (state, action) => {
        state.loading = false;
        state.error = action.error.message ?? 'Failed to fetch agents';
      })
      .addCase(createAgent.fulfilled, (state, action) => {
        state.agents.unshift(action.payload);
      })
      .addCase(updateAgent.fulfilled, (state, action) => {
        const idx = state.agents.findIndex((a) => a.id === action.payload.id);
        if (idx >= 0) state.agents[idx] = action.payload;
      })
      .addCase(deleteAgent.fulfilled, (state, action) => {
        state.agents = state.agents.filter((a) => a.id !== action.payload);
        if (state.selectedAgentId === action.payload) state.selectedAgentId = null;
      })
      .addCase(runAgentStandalone.pending, (state) => {
        state.running = true;
        state.runOutput = null;
      })
      .addCase(runAgentStandalone.fulfilled, (state, action) => {
        state.running = false;
        state.runOutput = action.payload;
      })
      .addCase(runAgentStandalone.rejected, (state, action) => {
        state.running = false;
        state.error = action.error.message ?? 'Agent run failed';
      })
      .addCase(fetchAgentHistory.fulfilled, (state, action) => {
        state.history = action.payload;
      })
      .addCase(searchAgentMemory.fulfilled, (state, action) => {
        state.memories = action.payload;
      });
  },
});

export const { setSelectedAgent, clearAgentRun, clearAgentError } = agentSlice.actions;
export default agentSlice.reducer;
