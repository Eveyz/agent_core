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
  WorkflowRunStatus,
} from './types';
import { applyNodeChanges, applyEdgeChanges, addEdge, type NodeChange, type EdgeChange, type Connection, type Node, type Edge } from '@xyflow/react';
import { nodeDefToRF, edgeDefToRF } from './converters';

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
  isExecuting: boolean;
  runStatus: WorkflowRunStatus | string;
  activeRunId: string | null;
  lastRunResult: WorkflowRunResult | null;
  runs: WorkflowRun[];
  runResults: WorkflowRunNodeResult[];
  activeNodeResults: Record<string, WorkflowRunNodeResult>;
  inspectedNodeId: string | null;
  dirty: boolean;
  
  // -- UI & Canvas State --
  nodes: Node[];
  edges: Edge[];
  selectedNodeId: string | null;
  selectedEdgeId: string | null;
  showRunView: boolean;
}

const initialState: WorkflowState = {
  workflows: [],
  activeWorkflow: null,
  loading: false,
  error: null,
  isExecuting: false,
  runStatus: 'idle',
  activeRunId: null,
  lastRunResult: null,
  runs: [],
  runResults: [],
  activeNodeResults: {},
  inspectedNodeId: null,
  dirty: false,
  nodes: [],
  edges: [],
  selectedNodeId: null,
  selectedEdgeId: null,
  showRunView: false,
};

const workflowSlice = createSlice({
  name: 'workflow',
  initialState,
  reducers: {
    setActiveWorkflow(state, action: PayloadAction<WorkflowDef | null>) {
      state.activeWorkflow = action.payload;
      state.dirty = false;
      state.selectedNodeId = null;
      state.selectedEdgeId = null;
      state.inspectedNodeId = null;
      if (action.payload) {
        state.nodes = action.payload.nodes.map(nodeDefToRF);
        state.edges = action.payload.edges.map(edgeDefToRF);
      } else {
        state.nodes = [];
        state.edges = [];
      }
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
    setInspectedNodeId(state, action: PayloadAction<string | null>) {
      state.inspectedNodeId = action.payload;
    },
    updateNodeStatus(state, action: PayloadAction<{ nodeId: string, result: WorkflowRunNodeResult }>) {
      const { nodeId, result } = action.payload;
      state.activeNodeResults[nodeId] = result;
    },
    appendNodeLog(state, action: PayloadAction<{ nodeId: string, log: any }>) {
      const { nodeId, log } = action.payload;
      if (state.activeNodeResults[nodeId]) {
        const currentLogs = state.activeNodeResults[nodeId].live_logs || [];
        state.activeNodeResults[nodeId].live_logs = [...currentLogs, log];
      }
    },
    
    // -- Canvas Interactions --
    onNodesChange(state, action: PayloadAction<NodeChange[]>) {
      state.nodes = applyNodeChanges(action.payload, state.nodes);
      state.dirty = true;
    },
    onEdgesChange(state, action: PayloadAction<EdgeChange[]>) {
      state.edges = applyEdgeChanges(action.payload, state.edges);
      state.dirty = true;
    },
    onConnect(state, action: PayloadAction<Connection>) {
      state.edges = addEdge({ ...action.payload, animated: true }, state.edges);
      state.dirty = true;
    },
    setSelectedNodeId(state, action: PayloadAction<string | null>) {
      state.selectedNodeId = action.payload;
    },
    setSelectedEdgeId(state, action: PayloadAction<string | null>) {
      state.selectedEdgeId = action.payload;
    },
    setShowRunView(state, action: PayloadAction<boolean>) {
      state.showRunView = action.payload;
    },
    addNode(state, action: PayloadAction<Node>) {
      state.nodes.push(action.payload);
      state.dirty = true;
    },
    deleteNode(state, action: PayloadAction<string>) {
      const nodeId = action.payload;
      state.nodes = state.nodes.filter(n => n.id !== nodeId);
      state.edges = state.edges.filter(e => e.source !== nodeId && e.target !== nodeId);
      if (state.selectedNodeId === nodeId) state.selectedNodeId = null;
      state.dirty = true;
    },
    updateNodeData(state, action: PayloadAction<{ nodeId: string, data: any }>) {
      const { nodeId, data } = action.payload;
      const node = state.nodes.find(n => n.id === nodeId);
      if (node) {
        node.data = data;
        state.dirty = true;
      }
    },
    updateEdgeData(state, action: PayloadAction<{ edgeId: string, updates: any }>) {
      const { edgeId, updates } = action.payload;
      const edge = state.edges.find(e => e.id === edgeId);
      if (edge) {
        Object.assign(edge, updates);
        state.dirty = true;
      }
    }
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
        state.nodes = [];
        state.edges = [];
        state.dirty = false;
      })
      .addCase(getWorkflow.fulfilled, (state, action) => {
        state.activeWorkflow = action.payload;
        state.nodes = action.payload.nodes.map(nodeDefToRF);
        state.edges = action.payload.edges.map(edgeDefToRF);
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
        state.isExecuting = true;
        state.runStatus = 'running';
        state.error = null;
        state.activeNodeResults = {};
      })
      .addCase(runWorkflow.fulfilled, (state, action) => {
        state.isExecuting = false;
        state.runStatus = action.payload.status;
        state.lastRunResult = action.payload;
      })
      .addCase(runWorkflow.rejected, (state, action) => {
        state.isExecuting = false;
        state.runStatus = 'failed';
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
  setInspectedNodeId,
  updateNodeStatus,
  appendNodeLog,
  
  onNodesChange,
  onEdgesChange,
  onConnect,
  setSelectedNodeId,
  setSelectedEdgeId,
  setShowRunView,
  addNode,
  deleteNode,
  updateNodeData,
  updateEdgeData,
} = workflowSlice.actions;
export default workflowSlice.reducer;
