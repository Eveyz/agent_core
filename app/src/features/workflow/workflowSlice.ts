import { createSlice, createAsyncThunk, type PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import type {
  WorkflowDef,
  WorkflowRun,
  WorkflowRunNodeResult,
  WorkflowRunResult,
  NodeDef,
  EdgeDef,
  TrustMode,
  OnNodeFailure,
} from './types';

// ── Thunks ──────────────────────────────────────────────────────────

export const fetchWorkflows = createAsyncThunk<WorkflowDef[]>('workflow/fetchAll', async () => {
  return invoke<WorkflowDef[]>('list_workflows');
});

export const createWorkflow = createAsyncThunk<WorkflowDef, { name: string; description?: string }>(
  'workflow/create',
  async (params) => invoke<WorkflowDef>('create_workflow', params),
);

export const getWorkflow = createAsyncThunk<WorkflowDef, string>(
  'workflow/get',
  async (id) => invoke<WorkflowDef>('get_workflow', { id }),
);

export interface SaveWorkflowParams {
  id: string;
  name: string;
  description?: string;
  nodes: NodeDef[];
  edges: EdgeDef[];
  trust_mode?: TrustMode;
  max_concurrent?: number;
  on_node_failure?: OnNodeFailure;
  input_schema?: Record<string, unknown>;
  config?: Record<string, unknown>;
}

export const saveWorkflow = createAsyncThunk<WorkflowDef, SaveWorkflowParams>(
  'workflow/save',
  async (params) => invoke<WorkflowDef>('save_workflow', params as unknown as Record<string, unknown>),
);

export const deleteWorkflow = createAsyncThunk<string, string>(
  'workflow/delete',
  async (id) => {
    await invoke('delete_workflow', { id });
    return id;
  },
);

export const runWorkflow = createAsyncThunk<
  WorkflowRunResult,
  { workflowId: string; input?: unknown; sessionId?: string }
>('workflow/run', async ({ workflowId, input, sessionId }) =>
  invoke<WorkflowRunResult>('run_workflow', { workflowId, input, sessionId }),
);

export const cancelWorkflowRun = createAsyncThunk<void, string>(
  'workflow/cancel',
  async (runId) => invoke('cancel_workflow_run', { runId }),
);

export const fetchWorkflowRuns = createAsyncThunk<
  WorkflowRun[],
  { workflowId: string; limit?: number }
>('workflow/fetchRuns', async ({ workflowId, limit }) =>
  invoke<WorkflowRun[]>('list_workflow_runs', { workflowId, limit }),
);

export const fetchWorkflowRunResults = createAsyncThunk<WorkflowRunNodeResult[], string>(
  'workflow/fetchRunResults',
  async (runId) => invoke<WorkflowRunNodeResult[]>('get_workflow_run_results', { runId }),
);

// ── Slice ───────────────────────────────────────────────────────────

interface WorkflowState {
  workflows: WorkflowDef[];
  activeWorkflow: WorkflowDef | null;
  loading: boolean;
  error: string | null;
  running: boolean;
  lastRunResult: WorkflowRunResult | null;
  runs: WorkflowRun[];
  runResults: WorkflowRunNodeResult[];
  dirty: boolean;
}

const initialState: WorkflowState = {
  workflows: [],
  activeWorkflow: null,
  loading: false,
  error: null,
  running: false,
  lastRunResult: null,
  runs: [],
  runResults: [],
  dirty: false,
};

const workflowSlice = createSlice({
  name: 'workflow',
  initialState,
  reducers: {
    setActiveWorkflow(state, action: PayloadAction<WorkflowDef | null>) {
      state.activeWorkflow = action.payload;
      state.dirty = false;
    },
    markDirty(state) {
      state.dirty = true;
    },
    markClean(state) {
      state.dirty = false;
    },
    updateActiveWorkflowNodes(state, action: PayloadAction<{ nodes: NodeDef[]; edges: EdgeDef[] }>) {
      if (state.activeWorkflow) {
        state.activeWorkflow.nodes = action.payload.nodes;
        state.activeWorkflow.edges = action.payload.edges;
        state.dirty = true;
      }
    },
    clearWorkflowError(state) {
      state.error = null;
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(fetchWorkflows.pending, (state) => {
        state.loading = true;
        state.error = null;
      })
      .addCase(fetchWorkflows.fulfilled, (state, action) => {
        state.loading = false;
        state.workflows = action.payload;
      })
      .addCase(fetchWorkflows.rejected, (state, action) => {
        state.loading = false;
        state.error = action.error.message ?? 'Failed to fetch workflows';
      })
      .addCase(createWorkflow.fulfilled, (state, action) => {
        state.workflows.unshift(action.payload);
        state.activeWorkflow = action.payload;
        state.dirty = false;
      })
      .addCase(getWorkflow.fulfilled, (state, action) => {
        state.activeWorkflow = action.payload;
        state.dirty = false;
      })
      .addCase(saveWorkflow.fulfilled, (state, action) => {
        state.activeWorkflow = action.payload;
        state.dirty = false;
        const idx = state.workflows.findIndex((w) => w.id === action.payload.id);
        if (idx >= 0) state.workflows[idx] = { ...action.payload };
      })
      .addCase(deleteWorkflow.fulfilled, (state, action) => {
        state.workflows = state.workflows.filter((w) => w.id !== action.payload);
        if (state.activeWorkflow?.id === action.payload) state.activeWorkflow = null;
      })
      .addCase(runWorkflow.pending, (state) => {
        state.running = true;
        state.error = null;
      })
      .addCase(runWorkflow.fulfilled, (state, action) => {
        state.running = false;
        state.lastRunResult = action.payload;
      })
      .addCase(runWorkflow.rejected, (state, action) => {
        state.running = false;
        state.error = action.error.message ?? 'Workflow run failed';
      })
      .addCase(fetchWorkflowRuns.fulfilled, (state, action) => {
        state.runs = action.payload;
      })
      .addCase(fetchWorkflowRunResults.fulfilled, (state, action) => {
        state.runResults = action.payload;
      });
  },
});

export const {
  setActiveWorkflow,
  markDirty,
  markClean,
  updateActiveWorkflowNodes,
  clearWorkflowError,
} = workflowSlice.actions;
export default workflowSlice.reducer;
