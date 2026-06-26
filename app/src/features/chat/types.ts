export interface TodoItem {
  id: string;
  description: string;
  status: 'pending' | 'in_progress' | 'completed' | 'blocked';
}

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean; message_id?: string }
  | { type: 'thinking'; text: string; isStreaming: boolean; message_id?: string; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: unknown; result: string; active: boolean; is_error: boolean; startTime?: number; endTime?: number }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
  | { type: 'error'; text: string }
  | { type: 'subagent_ref'; subagent_id: string; parent_call_id?: string };

export interface SubagentBlock {
  type: 'assistant' | 'thinking' | 'tool' | 'approval' | 'error';
  text?: string;
  isStreaming?: boolean;
  message_id?: string;
  startTime?: number;
  endTime?: number;
  call_id?: string;
  name?: string;
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  prompt_id?: string;
  tool_name?: string;
  tool_input?: unknown;
  danger_level?: string;
  explanation?: string;
  status?: 'pending' | 'approved' | 'denied';
}

export interface SubagentEntry {
  id: string;
  role_name?: string;
  task: string;
  status: 'working' | 'done' | 'error';
  iterations_used?: number;
  blocks: SubagentBlock[];
  startTime: number;
  endTime?: number;
}

export interface ChatEntry {
  id: string;
  type: 'user' | 'turn';
  turnId?: string;
  turnIds?: string[];
  turnIndex?: number;
  text?: string;
  blocks?: TurnBlock[];
  startTime?: number;
  endTime?: number;
  subagentIds?: string[];
}

export interface ChatState {
  entries: ChatEntry[];
  isProcessing: boolean;
  runId: string | null;
  runState: RunState | null;
  lastSeqByRun: Record<string, number>;
  subagents: Record<string, SubagentEntry>;
  viewingSubagentPath: { id: string; name: string }[];
  resyncing: boolean;
  _pendingTurnId?: string;
  entriesBySession: Record<string, ChatEntry[]>;
  processingBySession: Record<string, boolean>;
  subagentsBySession: Record<string, Record<string, SubagentEntry>>;
  runIdBySession: Record<string, string | null>;
  activeSessionId: string | null;
  _resumedFromBackend: boolean;
  _thinkBuffers: Record<string, string>;
  _pendingGap: { runId: string; fromSeq: number } | null;
  todo: TodoItem[];
}

export interface DeltaPayload {
  Text?: string;
  Thinking?: string;
}

export type RunEventType =
  | 'run_created' | 'run_started' | 'run_paused' | 'run_resumed'
  | 'run_completed' | 'run_cancelled' | 'run_failed'
  | 'state_changed'
  | 'turn_started' | 'turn_ended'
  | 'model_call_started' | 'model_streaming' | 'model_call_ended'
  | 'message_start' | 'message_update' | 'message_end'
  | 'tool_started' | 'tool_update' | 'tool_ended'
  | 'approval_required' | 'approval_resolved' | 'input_requested'
  | 'context_compacted' | 'error'
  | 'subagent_started' | 'subagent_ended'
  | 'process_spawned' | 'process_killed'
  | 'todo_updated';

export interface RunEventPayload {
  event: RunEventType;
  seq?: number;
  event_id?: string;
  run_id?: string;
  turn_id?: string;
  parent_call_id?: string;
  id?: string;
  session_id?: string;
  final_text?: string;
  reason?: string;
  from?: string;
  to?: string;
  index?: number;
  delta?: DeltaPayload;
  message_id?: string;
  text?: string;
  tool_count?: number;
  message?: { role: string; content?: string };
  call_id?: string;
  name?: string;
  args?: unknown;
  partial?: string;
  result?: string;
  is_error?: boolean;
  prompt_id?: string;
  tool_name?: string;
  tool_input?: unknown;
  danger_level?: string;
  explanation?: string;
  error?: string;
  choice?: string;
  subagent_id?: string;
  role_name?: string;
  task?: string;
  success?: boolean;
  iterations_used?: number;
  child_id?: string;
  label?: string;
  items?: { id: string; description: string; status: string }[];
}

export type RunState = 'created' | 'running' | 'awaiting_approval' | 'awaiting_input' | 'paused' | 'completed' | 'cancelled' | 'failed';

export interface EventLogEntry {
  turn_index: number;
  event_type: string;
  payload: unknown;
  started_at: string | null;
  ended_at: string | null;
}
