
export interface FrontendMessage {
  role: string;
  content: string;
  model?: string;
  tool_calls?: any[];
  tool_call_id?: string;
  name?: string;
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

export interface SteerMessage {
  steerId: string;
  text: string;
  status: 'pending' | 'injected';
  timestamp: number;
}

export interface TodoItem {
  id: string;
  description: string;
  status: 'pending' | 'in_progress' | 'completed' | 'blocked';
}

export interface ClarificationOption {
  id: string;
  label: string;
}

export interface ClarificationQuestion {
  id: string;
  prompt: string;
  allow_multiple?: boolean;
  options: ClarificationOption[];
}

export type ClarificationAnswers = Record<string, string[]>;

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean; message_id?: string }
  | { type: 'thinking'; text: string; isStreaming: boolean; message_id?: string; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: unknown; result: string; active: boolean; is_error: boolean; startTime?: number; endTime?: number; phase?: 'preparing' | 'running'; stream_index?: number; hint_path?: string }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
  | {
      type: 'clarification';
      prompt_id: string;
      title?: string;
      questions: ClarificationQuestion[];
      status: 'pending' | 'answered' | 'cancelled';
      answers?: ClarificationAnswers;
    }
  | { type: 'error'; text: string }
  | { type: 'notice'; text: string; code?: string; severity?: string }
  | { type: 'subagent_ref'; subagent_id: string; parent_call_id?: string };

/** Same shape as TurnBlock, minus nested `subagent_ref` (subagents don't spawn further). */
export type SubagentBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean; message_id?: string }
  | { type: 'thinking'; text: string; isStreaming: boolean; message_id?: string; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: unknown; result: string; active: boolean; is_error: boolean; startTime?: number; endTime?: number; phase?: 'preparing' | 'running'; stream_index?: number; hint_path?: string }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
  | {
      type: 'clarification';
      prompt_id: string;
      title?: string;
      questions: ClarificationQuestion[];
      status: 'pending' | 'answered' | 'cancelled';
      answers?: ClarificationAnswers;
    }
  | { type: 'error'; text: string }
  | { type: 'notice'; text: string; code?: string; severity?: string };

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
  promptId?: string;
  text?: string;
  /** Model that was active when this prompt was submitted. Per-prompt, not
   * the global currently-selected model. Falls back to the global model for
   * entries created before this field existed. */
  model?: string;
  blocks?: TurnBlock[];
  startTime?: number;
  endTime?: number;
  subagentIds?: string[];
  cacheHitRate?: number;
  /** True when this user entry is a steering message (mid-run injection). */
  isSteer?: boolean;
  /** Steer message id (matches backend steer_id). */
  steerId?: string;
  /** Lifecycle status of a steer entry. */
  steerStatus?: 'pending' | 'injected';
  /** True when the prompt that spawned this turn was interrupted by a crash / restart. */
  interrupted?: boolean;
}

export interface SkillManifest {
  name: string;
  description: string;
  version?: string;
  triggers?: string[];
  tags?: string[];
  [key: string]: unknown;
}

export interface BtwEntry {
  id: string;
  question: string;
  answer: string;
  isStreaming: boolean;
  startTime: number;
  endTime?: number;
}



export interface ChatState {
  // ── Per-session state (maps keyed by sessionId) ──
  entries: Record<string, ChatEntry[]>;
  processing: Record<string, boolean>;
  subagents: Record<string, Record<string, SubagentEntry>>;
  runId: Record<string, string | null>;
  runState: Record<string, RunState | null>;
  todo: Record<string, TodoItem[]>;
  steerQueue: Record<string, SteerMessage[]>;
  allPrompts: Record<string, FrontendPrompt[]>;
  visiblePromptsCount: Record<string, number>;
  isDirty: Record<string, boolean>;
  contentRevision: Record<string, number>;
  persistedRevision: Record<string, number>;
  _resumedFromBackend: Record<string, boolean>;
  _thinkBuffers: Record<string, Record<string, string>>;
  goal: Record<string, string | null>;
  goalCompleted: Record<string, boolean>;
  viewingSubagentPath: Record<string, { id: string; name: string }[]>;
  btwEntries: Record<string, BtwEntry[]>;

  isResuming: Record<string, boolean>;
  /** turn_id for the event currently being processed, keyed by sessionId */
  _pendingTurnId: Record<string, string | undefined>;

  // ── Global / cross-session routing ──
  runIdToSessionId: Record<string, string>;
  lastSeqByRun: Record<string, number>;
  skillsCache: {
    skills: SkillManifest[];
    loadedAt: number;
    scopeKey: string;
  } | null;
  resyncingByRun: Record<string, boolean>;
  pendingGapByRun: Record<string, { fromSeq: number; toSeq: number }>;
  cacheMetricsByRun: Record<string, CacheMetrics>;
  appliedEventIdsByRun: Record<string, Record<string, true>>;
  pendingEventsByRun: Record<string, Record<number, RunEventPayload>>;
}

export interface CacheMetrics {
  total_turns: number;
  total_hit_tokens: number;
  total_miss_tokens: number;
  turns_with_hits: number;
  cumulative_hit_rate: number;
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
  | 'tool_preparing' | 'tool_started' | 'tool_update' | 'tool_ended'
  | 'approval_required' | 'approval_resolved' | 'input_requested' | 'input_resolved'
  | 'context_compacted' | 'error'
  | 'notice'
  | 'subagent_started' | 'subagent_ended'
  | 'process_spawned' | 'process_killed'
  | 'todo_updated'
  | 'cache_info' | 'cache_summary'
  | 'steer_queued'
  | 'steer_injected'
  | 'steer_cancelled'
  | 'steer_failed'
  | 'goal_set'
  | 'goal_completed'
  | 'goal_cleared';

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
  message?: { role: string; content?: string } | string;
  call_id?: string;
  name?: string;
  args?: unknown;
  partial?: string;
  result?: string;
  is_error?: boolean;
  hint_path?: string;
  prompt_id?: string;
  tool_name?: string;
  tool_input?: unknown;
  danger_level?: string;
  explanation?: string;
  title?: string;
  questions?: ClarificationQuestion[];
  answers?: ClarificationAnswers;
  question?: string;
  error?: string;
  code?: string;
  severity?: string;
  recoverable?: boolean;
  choice?: string;
  subagent_id?: string;
  role_name?: string;
  task?: string;
  success?: boolean;
  iterations_used?: number;
  child_id?: string;
  label?: string;
  items?: { id: string; description: string; status: string }[];
  hit_tokens?: number;
  miss_tokens?: number;
  hit_rate?: number;
  // Steering event fields
  steer_id?: string;
  queue_depth?: number;
  goal?: string;
  // CacheSummary fields
  total_turns?: number;
  total_hit_tokens?: number;
  total_miss_tokens?: number;
  turns_with_hits?: number;
  cumulative_hit_rate?: number;
}

export type RunState = 'created' | 'running' | 'awaiting_approval' | 'awaiting_input' | 'paused' | 'completed' | 'cancelled' | 'failed';
