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
}

export interface AgentMessageEvent {
  sequence: number;
  conversation_id: string;
  event_type: string;
  message_id?: string;
  task_id?: string;
  payload: Record<string, unknown>;
  created_at: string;
}

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
