// Workflow feature types — mirror the Rust WorkflowDef / NodeDef / EdgeDef.

export type NodeType = 'input' | 'output' | 'agent' | 'transform' | 'human_approval';
export type TrustMode = 'inherit' | 'trusted' | 'readonly';
export type OnNodeFailure = 'abort' | 'continue' | 'skip';

export type WorkflowLifecycle = 'transient' | 'draft' | 'published';
export type WorkflowSourceKind = 'runtime' | 'legacy';

export type WorkflowLibraryScope =
  | { kind: 'session'; session_id: string }
  | { kind: 'project'; project_id: string; workspace: string }
  | { kind: 'user' };

export interface PublishedWorkflowReceipt {
  workflow_id: string;
  draft_id: string;
  draft_version: number;
  revision_id: string;
  revision_number: number;
  program_hash: string;
  published_at: string;
}

export interface RuntimeWorkflowStep {
  key: string;
  instruction: string;
  after: string[];
  inputs: Record<string, unknown>;
  agent: {
    type: 'saved' | 'inline';
    agent_id?: string;
    revision_id?: string;
    blueprint?: {
      name: string;
      description?: string;
    };
  };
}

export interface RuntimeWorkflowSpec {
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  steps: RuntimeWorkflowStep[];
  result: Record<string, unknown>;
  policy: Record<string, unknown>;
}

export interface RuntimeWorkflowProgramNode {
  key: string;
  kind: {
    type: 'activity' | 'output' | 'choice' | 'wait_signal' | 'timer' | 'child_workflow' | 'for_each';
    kind?: string;
    config?: Record<string, unknown>;
    name?: string;
    delay_ms?: number;
    revision_id?: string;
  };
  inputs: Record<string, unknown>;
  after: string[];
  retry: {
    max_attempts: number;
    backoff_ms: number;
  };
  timeout_ms?: number | null;
  effect: 'pure' | 'read_only' | 'workspace_write' | 'external';
  resources: Array<{ resource: string; exclusive: boolean }>;
}

export interface RuntimeWorkflowProgram {
  schema_version: number;
  nodes: RuntimeWorkflowProgramNode[];
  result: Record<string, unknown>;
  policy: {
    max_concurrency: number;
    on_failure: string;
  };
}

export interface WorkflowLibraryEntry {
  workflow_id: string;
  name: string;
  description: string;
  scope: WorkflowLibraryScope;
  lifecycle: WorkflowLifecycle;
  source_kind: WorkflowSourceKind;
  draft_id: string;
  draft_version: number;
  draft_status: string;
  program_hash: string;
  latest_revision?: PublishedWorkflowReceipt | null;
  updated_at: string;
  workflow?: RuntimeWorkflowSpec | null;
  program?: RuntimeWorkflowProgram | null;
}

export interface WorkflowRuntimeRunSummary {
  run_id: string;
  status: string;
  trigger: string;
  created_at: string;
  updated_at: string;
  failed_nodes: string[];
}

export type NodeRunStatus = 
  | 'pending'
  | 'running'
  | 'completed'
  | 'failed'
  | 'cancelling'
  | 'cancelled'
  | 'skipped';

export type WorkflowRunStatus =
  | 'idle'
  | 'running'
  | 'cancelling'
  | 'completed'
  | 'failed'
  | 'cancelled';

export interface LiveLogEntry {
  id: string;
  type: 'thought' | 'tool_call' | 'observation' | 'error' | 'warning';
  content: string;
  timestamp: string;
}

export interface NodeDef {
  id: string;
  workflow_id: string;
  node_type: NodeType;
  label: string;
  agent_id: string;
  config: Record<string, unknown>;
  position_x: number;
  position_y: number;
  created_at: string;
}

export interface EdgeDef {
  id: string;
  workflow_id: string;
  source_node_id: string;
  target_node_id: string;
  source_handle: string;
  target_handle: string;
  label: string;
  condition: string;
  data_mapping: Record<string, unknown>;
  created_at: string;
}

export interface WorkflowDef {
  id: string;
  name: string;
  description: string;
  input_schema: Record<string, unknown>;
  trust_mode: TrustMode;
  max_concurrent: number;
  on_node_failure: OnNodeFailure;
  config: Record<string, unknown>;
  nodes: NodeDef[];
  edges: EdgeDef[];
  created_at: string;
  updated_at: string;
}

export interface WorkflowRun {
  id: string;
  workflow_id: string;
  session_id: string;
  status: string;
  input: unknown;
  output: unknown;
  error: string;
  total_token_input: number;
  total_token_output: number;
  started_at: string;
  finished_at: string | null;
  created_at: string;
}

export interface WorkflowRunNodeResult {
  id: string;
  workflow_run_id: string;
  node_id: string;
  agent_history_id: string;
  status: NodeRunStatus | string; // Allow string for backend compat initially
  input: unknown;
  output: unknown;
  error: string;
  token_input: number;
  token_output: number;
  cost_usd: number;
  latency_ms: number;
  started_at: string | null;
  finished_at: string | null;
  created_at: string;
  live_logs?: LiveLogEntry[];
}

export interface WorkflowRunResult {
  run_id: string;
  status: WorkflowRunStatus | string;
  output: unknown;
  error: string;
  total_token_input: number;
  total_token_output: number;
}

export const NODE_TYPES: { value: NodeType; label: string }[] = [
  { value: 'input', label: 'Input' },
  { value: 'agent', label: 'Agent' },
  { value: 'transform', label: 'Transform' },
  { value: 'human_approval', label: 'Human Approval' },
  { value: 'output', label: 'Output' },
];

export const TRUST_MODES: { value: TrustMode; label: string }[] = [
  { value: 'inherit', label: 'Inherit' },
  { value: 'trusted', label: 'Trusted' },
  { value: 'readonly', label: 'Read-only' },
];
