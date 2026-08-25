// Agent feature types — mirror the Rust AgentDef / AgentHistoryEntry structs.

export interface AgentDef {
  id: string;
  name: string;
  description: string;
  system_prompt: string;
  /** "provider/model" or "" for the global default model. */
  model: string;
  skills: string[];
  /** Tool names; empty = inherit all available tools. */
  tools: string[];
  permission_mode: string;
  permission_rules: unknown;
  max_iterations: number;
  max_context_tokens: number;
  /** 0 = stateless, 1 = standard, 2 = deep. */
  memory_enabled: number;
  /** Empty = per-agent isolated memory; non-empty = shared group. */
  memory_group: string;
  icon: string;
  color: string;
  created_at: string;
  updated_at: string;
}

export interface AgentHistoryEntry {
  id: string;
  agent_id: string;
  session_id: string;
  workflow_run_id: string;
  trigger: string;
  input: string;
  output: string;
  iterations_used: number;
  success: boolean;
  model_used: string;
  token_input: number;
  token_output: number;
  process_time_ms: number;
  created_at: string;
}

export interface AgentMemoryRecord {
  id: string;
  role: string;
  content: string;
  importance: number;
  category: string;
  created_at: string;
}

export interface AgentConversation {
  id: string;
  agent_id: string;
  project_id: string;
  session_id: string;
  unread_count: number;
  created_at: string;
  updated_at: string;
}

export interface AgentConversationMessage {
  role: string;
  content: string;
  model?: string;
  metadata?: Record<string, unknown>;
}

export interface AgentConversationView {
  conversation: AgentConversation;
  session: {
    meta: { id: string; title: string; model_used: string };
    messages: AgentConversationMessage[];
  };
  messaging: {
    next_sequence: number;
    events: AgentMessageEvent[];
  };
  swarm?: AgentSwarmSnapshot | null;
}

export interface AgentSwarmRun {
  id: string;
  goal: string;
  status: 'running' | 'completing' | 'completed' | 'cancelled' | 'needs_attention';
  max_messages: number;
  messages_used: number;
  max_turns: number;
  turns_used: number;
  max_hops: number;
  hops_used: number;
  summary: string;
  error: string;
  completion_task_id?: string | null;
  completion_turn_id?: string | null;
}

export interface AgentSwarmSnapshot {
  run: AgentSwarmRun;
  participant_agent_ids: string[];
  messages: Array<{ id: string }>;
}

interface AgentMessageEventBase {
  sequence: number;
  conversation_id: string;
  message_id?: string;
  task_id?: string;
  created_at: string;
}

export type AgentMessageEvent =
  | (AgentMessageEventBase & {
      event_type: 'message_received';
      payload: {
        from?: string;
        kind?: 'request' | 'reply' | 'notification';
        priority?: boolean;
        display_content?: string;
      };
    })
  | (AgentMessageEventBase & {
      event_type: 'message_sent';
      payload: {
        to?: string;
        kind?: 'request' | 'reply' | 'notification';
        priority?: boolean;
      };
    })
  | (AgentMessageEventBase & {
      event_type:
        | 'task_queued'
        | 'task_working'
        | 'task_completed'
        | 'task_failed'
        | 'task_cancelled'
        | 'task_needs_attention';
      payload: {
        error?: string;
        worker_id?: string;
        attempt_count?: number;
      };
    });

export interface AgentConversationSendResult {
  view: AgentConversationView;
  deliveries: Array<{
    message: {
      id: string;
      from_display_name: string;
      to_display_name: string;
      kind: 'request' | 'reply' | 'notification';
    };
    replayed: boolean;
  }>;
}

export const PERMISSION_MODES = [
  'paranoid',
  'standard',
  'developer',
  'permissive',
  'yolo',
] as const;

export const MEMORY_MODES = [
  { value: 0, label: 'Stateless' },
  { value: 1, label: 'Standard' },
  { value: 2, label: 'Deep' },
] as const;
