import { createSlice, PayloadAction } from '@reduxjs/toolkit';
import { resumeSession } from '../project/projectSlice';

// ── Types ────────────────────────────────────────────────────────────

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean }
  | { type: 'thinking'; text: string; isStreaming: boolean; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: unknown; result: string; active: boolean; is_error: boolean }
  | { type: 'approval'; prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string; status: 'pending' | 'approved' | 'denied' }
  | { type: 'error'; text: string }
  | { type: 'subagent_ref'; subagent_id: string };

export interface SubagentBlock {
  type: 'assistant' | 'thinking' | 'tool' | 'approval' | 'error';
  text?: string;
  isStreaming?: boolean;
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
  turnIndex?: number;
  text?: string;
  blocks?: TurnBlock[];
  startTime?: number;
  endTime?: number;
  subagents?: Record<string, SubagentEntry>;
}

interface ChatState {
  entries: ChatEntry[];
  isProcessing: boolean;
  /** ID of the currently active Run (set when send_message returns). */
  runId: string | null;
  /** Current lifecycle state of the active Run. */
  runState: RunState | null;
  entriesBySession: Record<string, ChatEntry[]>;
  processingBySession: Record<string, boolean>;
  _resumedFromBackend: boolean;
}

const initialState: ChatState = {
  entries: [],
  isProcessing: false,
  runId: null,
  runState: null,
  entriesBySession: {},
  processingBySession: {},
  _resumedFromBackend: false,
};

// ── Event payload types ──────────────────────────────────────────────

interface DeltaPayload {
  Text?: string;
  Thinking?: string;
}

export interface AgentEvent {
  // Legacy AgentEvent format (inline tagged)
  TurnStart?: { turn_index: number };
  TurnEnd?: unknown;
  MessageUpdate?: { delta: DeltaPayload };
  MessageEnd?: unknown;
  ToolExecutionStart?: { tool_call_id: string; tool_name: string; args?: unknown };
  ToolExecutionUpdate?: { tool_call_id: string; partial_result: unknown };
  ToolExecutionEnd?: { tool_call_id: string; result: unknown; is_error: boolean };
  ApprovalRequired?: { prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string };
  AgentEnd?: unknown;
  AgentStart?: unknown;
  Aborted?: { reason: string };
  Error?: string;
  SubagentStart?: { subagent_id: string; role_name?: string; task: string | unknown };
  SubagentMessageUpdate?: { subagent_id: string; delta: DeltaPayload };
  SubagentToolStart?: { subagent_id: string; tool_call_id: string; tool_name: string; args?: unknown };
  SubagentToolEnd?: { subagent_id: string; tool_call_id: string; result: unknown; is_error: boolean };
  SubagentApprovalRequired?: { subagent_id: string; prompt_id: string; tool_name: string; tool_input: unknown; danger_level: string; explanation: string };
  SubagentEnd?: { subagent_id: string; success: boolean; iterations_used?: number };
}

// New RunEvent format (externally tagged: { "event": "snake_case", ...fields })
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
  | 'process_spawned' | 'process_killed';

export interface RunEventPayload {
  event: RunEventType;
  // Lifecycle
  id?: string;
  session_id?: string;
  final_text?: string;
  reason?: string;
  // State
  from?: string;
  to?: string;
  // Turn
  index?: number;
  // Model
  delta?: DeltaPayload;
  text?: string;
  tool_count?: number;
  // Messages (message_start event)
  message?: { role: string; content?: string };
  // Tools
  call_id?: string;
  name?: string;
  args?: unknown;
  partial?: string;
  result?: string;
  is_error?: boolean;
  // Approval
  prompt_id?: string;
  tool_name?: string;
  tool_input?: unknown;
  danger_level?: string;
  explanation?: string;
  // Error
  error?: string;
  // Subagent
  subagent_id?: string;
  role_name?: string;
  task?: string;
  success?: boolean;
  iterations_used?: number;
  // Process
  child_id?: string;
  label?: string;
}

export type RunState = 'created' | 'running' | 'awaiting_approval' | 'awaiting_input' | 'paused' | 'completed' | 'cancelled' | 'failed';


// ── RunEvent → AgentEvent converter ──────────────────────────────────

function runEventToAgentEvent(ev: RunEventPayload): AgentEvent {
  switch (ev.event) {
    case 'run_started':
      return {}; // no-op, like AgentStart
    case 'run_completed':
      return { AgentEnd: {} };
    case 'run_cancelled':
      return { Aborted: { reason: ev.reason ?? 'cancelled' } };
    case 'run_failed':
      return { Error: ev.error ?? 'run failed' };
    case 'error':
      return { Error: ((ev as unknown as Record<string, unknown>).message as string) ?? 'unknown error' };
    case 'turn_started':
      return { TurnStart: { turn_index: ev.index ?? 0 } };
    case 'turn_ended':
      return { TurnEnd: {} };
    case 'message_update':
      return { MessageUpdate: { delta: ev.delta ?? {} } };
    case 'message_end':
      return { MessageEnd: {} };
    case 'model_streaming':
      return { MessageUpdate: { delta: ev.delta ?? {} } };
    case 'tool_started':
      return { ToolExecutionStart: { tool_call_id: ev.call_id ?? '', tool_name: ev.name ?? '', args: ev.args } };
    case 'tool_update':
      return { ToolExecutionUpdate: { tool_call_id: ev.call_id ?? '', partial_result: ev.partial ?? '' } };
    case 'tool_ended':
      return { ToolExecutionEnd: { tool_call_id: ev.call_id ?? '', result: ev.result ?? '', is_error: ev.is_error ?? false } };
    case 'approval_required':
      return { ApprovalRequired: { prompt_id: ev.prompt_id ?? '', tool_name: ev.tool_name ?? '', tool_input: ev.tool_input, danger_level: ev.danger_level ?? '', explanation: ev.explanation ?? '' } };
    case 'subagent_started':
      return { SubagentStart: { subagent_id: ev.subagent_id ?? '', role_name: ev.role_name, task: ev.task ?? '' } };
    case 'subagent_ended':
      return { SubagentEnd: { subagent_id: ev.subagent_id ?? '', success: ev.success ?? false, iterations_used: ev.iterations_used } };
    default:
      return {};
  }
}

// ── Block helpers (shared between main agent + subagent) ─────────────

type AnyBlock = TurnBlock | SubagentBlock;

function closeStreamingBlock(blocks: AnyBlock[] | undefined): void {
  if (!blocks || blocks.length === 0) return;
  const last = blocks[blocks.length - 1];
  if (last && 'isStreaming' in last && last.isStreaming) {
    last.isStreaming = false;
    if (last.type === 'thinking') {
      last.endTime = Date.now();
    }
  }
}

function appendDeltaToBlocks(
  blocks: AnyBlock[],
  delta: DeltaPayload
): void {
  const appendToCurrent = (text: string, defaultType: 'assistant' | 'thinking') => {
    let block = blocks[blocks.length - 1];
    if (!block || !('isStreaming' in block) || !block.isStreaming || block.type !== defaultType) {
      closeStreamingBlock(blocks);
      if (defaultType === 'thinking') {
        blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
      } else {
        blocks.push({ type: 'assistant', text: '', isStreaming: true });
      }
      block = blocks[blocks.length - 1];
    }
    if (block.type === 'assistant' || block.type === 'thinking') {
      block.text += text;
    }
  };

  if (typeof delta.Text === 'string') {
    let textChunk = delta.Text;
    
    // Support DeepSeek <think> tags natively parsed from stream
    while (textChunk.includes('<think>') || textChunk.includes('</think>')) {
      const thinkStartIdx = textChunk.indexOf('<think>');
      const thinkEndIdx = textChunk.indexOf('</think>');
      
      if (thinkStartIdx !== -1 && (thinkEndIdx === -1 || thinkStartIdx < thinkEndIdx)) {
        // Handle <think>
        const before = textChunk.substring(0, thinkStartIdx);
        if (before) appendToCurrent(before, 'assistant');
        
        closeStreamingBlock(blocks);
        blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
        textChunk = textChunk.substring(thinkStartIdx + 7);
      } else if (thinkEndIdx !== -1) {
        // Handle </think>
        const before = textChunk.substring(0, thinkEndIdx);
        if (before) appendToCurrent(before, 'thinking');
        
        closeStreamingBlock(blocks);
        blocks.push({ type: 'assistant', text: '', isStreaming: true });
        textChunk = textChunk.substring(thinkEndIdx + 8);
      }
    }

    if (textChunk) {
      appendToCurrent(textChunk, 'assistant');
    }
  } else if (typeof delta.Thinking === 'string') {
    let block = blocks[blocks.length - 1];
    if (!block || block.type !== 'thinking' || !block.isStreaming) {
      closeStreamingBlock(blocks);
      blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
      block = blocks[blocks.length - 1];
    }
    if (block.type === 'thinking') {
      block.text += delta.Thinking;
    }
  }
}

const MAX_RESULT_LEN = 5000;

function truncateResult(result: string): string {
  if (result.length > MAX_RESULT_LEN) {
    return result.substring(0, MAX_RESULT_LEN) + `\n\n... [Truncated ${result.length - MAX_RESULT_LEN} characters for performance]`;
  }
  return result;
}

function stringifyResult(result: unknown): string {
  return typeof result === 'string' ? result : JSON.stringify(result);
}

function getActiveTurn(state: ChatState): ChatEntry | undefined {
  for (let i = state.entries.length - 1; i >= 0; i--) {
    const entry = state.entries[i];
    if (entry.type === 'turn' && !entry.endTime) {
      return entry;
    }
  }
  return undefined;
}

function getOrCreateSubagent(
  entry: ChatEntry,
  subagentId: string,
  roleName: string,
  task: string
): SubagentEntry {
  if (!entry.subagents) entry.subagents = {};
  if (!entry.subagents[subagentId]) {
    entry.subagents[subagentId] = {
      id: subagentId,
      role_name: roleName,
      task,
      status: 'working',
      blocks: [],
      startTime: Date.now(),
    };
  }
  return entry.subagents[subagentId];
}

// ── Event handlers ───────────────────────────────────────────────────

function handleTurnStart(state: ChatState, turnIndex: number): void {
  const last = state.entries[state.entries.length - 1];
  if (last && last.type === 'turn' && !last.endTime) {
    last.turnIndex = turnIndex;
  } else {
    state.entries.push({
      id: `turn-${turnIndex}-${Date.now()}`,
      type: 'turn',
      turnIndex,
      blocks: [],
      startTime: Date.now(),
    });
  }
}

function handleMessageUpdate(state: ChatState, delta: DeltaPayload): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    appendDeltaToBlocks(lastEntry.blocks, delta);
  }
}

function handleMessageEnd(state: ChatState): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
  }
}

function handleToolStart(
  state: ChatState,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
    lastEntry.blocks.push({
      type: 'tool',
      call_id: toolCallId,
      name: toolName,
      args,
      result: '',
      active: true,
      is_error: false,
    });
  }
}

function handleToolUpdate(state: ChatState, toolCallId: string, partialResult: unknown): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    const block = lastEntry.blocks.find((b) => b.type === 'tool' && b.call_id === toolCallId);
    if (block && block.type === 'tool') {
      block.result += typeof partialResult === 'string' ? partialResult : JSON.stringify(partialResult);
    }
  }
}

function handleToolEnd(
  state: ChatState,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    const block = lastEntry.blocks.find((b) => b.type === 'tool' && b.call_id === toolCallId);
    if (block && block.type === 'tool') {
      block.result = truncateResult(stringifyResult(result));
      block.active = false;
      block.is_error = isError;
    }
  }
}

function handleApprovalRequired(
  state: ChatState,
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
    lastEntry.blocks.push({
      type: 'approval',
      prompt_id: promptId,
      tool_name: toolName,
      tool_input: toolInput,
      danger_level: dangerLevel,
      explanation,
      status: 'pending',
    });
  }
}

function handleAgentEnd(state: ChatState): void {
  state.isProcessing = false;
  const last = state.entries[state.entries.length - 1];
  if (last && last.type === 'turn' && !last.endTime) {
    last.endTime = Date.now();
  }
}

function handleError(state: ChatState, errorText: string): void {
  state.isProcessing = false;
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
    
    const lastBlock = lastEntry.blocks[lastEntry.blocks.length - 1];
    const isRetryError = errorText.toLowerCase().includes('retrying model call');
    
    if (lastBlock && lastBlock.type === 'error') {
      const lastText = typeof lastBlock.text === 'string' ? lastBlock.text : '';
      if (isRetryError && lastText.toLowerCase().includes('retrying model call')) {
        lastBlock.text = errorText;
        return;
      }
    }
    
    lastEntry.blocks.push({ type: 'error', text: errorText });
  } else {
    state.entries.push({
      id: `error-${Date.now()}`,
      type: 'turn',
      turnIndex: 0,
      blocks: [{ type: 'error', text: errorText }],
      startTime: Date.now(),
      endTime: Date.now(),
    });
  }
}

// ── Subagent event handlers ──────────────────────────────────────────

function handleSubagentStart(
  state: ChatState,
  subagentId: string,
  roleName: string | undefined,
  task: string | unknown
): void {
  const safeTask = typeof task === 'string' ? task : JSON.stringify(task);
  const safeRoleName = typeof roleName === 'string' ? roleName : String(subagentId);
  const turn = getActiveTurn(state);
  if (turn) {
    getOrCreateSubagent(turn, subagentId, safeRoleName, safeTask);
    if (turn.blocks) {
      turn.blocks.push({ type: 'subagent_ref', subagent_id: subagentId });
    }
  }
}

function handleSubagentMessageUpdate(
  state: ChatState,
  subagentId: string,
  delta: DeltaPayload
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.subagents && turn.subagents[subagentId]) {
    appendDeltaToBlocks(turn.subagents[subagentId].blocks, delta);
  }
}

function handleSubagentToolStart(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.subagents && turn.subagents[subagentId]) {
    const sa = turn.subagents[subagentId];
    closeStreamingBlock(sa.blocks);
    sa.blocks.push({
      type: 'tool',
      call_id: toolCallId,
      name: toolName,
      args,
      result: '',
      active: true,
      is_error: false,
    });
  }
}

function handleSubagentToolEnd(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.subagents && turn.subagents[subagentId]) {
    const sa = turn.subagents[subagentId];
    const block = sa.blocks.find((b) => b.type === 'tool' && b.call_id === toolCallId);
    if (block && block.type === 'tool') {
      block.result = truncateResult(stringifyResult(result));
      block.active = false;
      block.is_error = isError;
    }
  }
}

function handleSubagentApprovalRequired(
  state: ChatState,
  subagentId: string,
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.subagents && turn.subagents[subagentId]) {
    const sa = turn.subagents[subagentId];
    closeStreamingBlock(sa.blocks);
    sa.blocks.push({
      type: 'approval',
      prompt_id: promptId,
      tool_name: toolName,
      tool_input: toolInput,
      danger_level: dangerLevel,
      explanation,
      status: 'pending',
    });
  }
}

function handleSubagentEnd(
  state: ChatState,
  subagentId: string,
  success: boolean,
  iterationsUsed?: number
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.subagents && turn.subagents[subagentId]) {
    const sa = turn.subagents[subagentId];
    sa.status = success ? 'done' : 'error';
    sa.iterations_used = iterationsUsed;
    sa.endTime = Date.now();
    sa.blocks.forEach((b) => {
      if (b.isStreaming) {
        b.isStreaming = false;
        if (b.type === 'thinking') b.endTime = Date.now();
      }
    });
  }
}

// ── Slice ────────────────────────────────────────────────────────────

export const chatSlice = createSlice({
  name: 'chat',
  initialState,
  reducers: {
    cacheCurrentSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      state.entriesBySession[sessionId] = state.entries;
      state.processingBySession[sessionId] = state.isProcessing;
    },
    restoreOrClearSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      const cached = state.entriesBySession[sessionId];
      if (cached) {
        state.entries = cached;
        state.isProcessing = state.processingBySession[sessionId] ?? false;
      } else {
        state.entries = [];
        state.isProcessing = false;
      }
      state._resumedFromBackend = false;
    },
    userMessageSent: (state, action: PayloadAction<string>) => {
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: action.payload,
      });
      state.isProcessing = true;
      state._resumedFromBackend = false;
    },
    runIdSet: (state, action: PayloadAction<string>) => {
      state.runId = action.payload;
      state.runState = 'running';
    },
    runStateChanged: (state, action: PayloadAction<RunState>) => {
      state.runState = action.payload;
      if (action.payload === 'completed' || action.payload === 'cancelled' || action.payload === 'failed') {
        state.isProcessing = false;
      }
    },
    agentEventReceived: (state, action: PayloadAction<string | Record<string, unknown>>) => {
      let event: AgentEvent;
      let raw: Record<string, unknown>;
      if (typeof action.payload === 'string') {
        if (action.payload === 'AgentStart') return;
        try {
          raw = JSON.parse(action.payload);
        } catch {
          return;
        }
      } else {
        raw = action.payload as Record<string, unknown>;
      }

      // Detect new RunEvent format: { "event": "snake_case", ... }
      if (raw && typeof raw.event === 'string') {
        const runEv = raw as unknown as RunEventPayload;
        // Handle lifecycle events directly (don't convert to AgentEvent)
        if (runEv.event === 'state_changed' && runEv.to) {
          state.runState = runEv.to as RunState;
          if (runEv.to === 'completed' || runEv.to === 'cancelled' || runEv.to === 'failed') {
            state.isProcessing = false;
          }
        }
        if (runEv.event === 'run_started') {
          state.runState = 'running';
        }
        if (runEv.event === 'run_completed' || runEv.event === 'run_cancelled' || runEv.event === 'run_failed') {
          state.isProcessing = false;
          state.runId = null;
        }
        if (runEv.event === 'run_paused') {
          state.runState = 'paused';
        }
        if (runEv.event === 'run_resumed') {
          state.runState = 'running';
        }
        event = runEventToAgentEvent(runEv);
      } else {
        event = raw as unknown as AgentEvent;
      }

      if (event.TurnStart) {
        handleTurnStart(state, event.TurnStart.turn_index);
      } else if (event.MessageUpdate) {
        handleMessageUpdate(state, event.MessageUpdate.delta);
      } else if (event.MessageEnd) {
        handleMessageEnd(state);
      } else if (event.ToolExecutionStart) {
        handleToolStart(
          state,
          event.ToolExecutionStart.tool_call_id,
          event.ToolExecutionStart.tool_name,
          event.ToolExecutionStart.args
        );
      } else if (event.ToolExecutionUpdate) {
        handleToolUpdate(state, event.ToolExecutionUpdate.tool_call_id, event.ToolExecutionUpdate.partial_result);
      } else if (event.ToolExecutionEnd) {
        handleToolEnd(
          state,
          event.ToolExecutionEnd.tool_call_id,
          event.ToolExecutionEnd.result,
          event.ToolExecutionEnd.is_error
        );
      } else if (event.ApprovalRequired) {
        handleApprovalRequired(
          state,
          event.ApprovalRequired.prompt_id,
          event.ApprovalRequired.tool_name,
          event.ApprovalRequired.tool_input,
          event.ApprovalRequired.danger_level,
          event.ApprovalRequired.explanation
        );
      } else if (event.AgentEnd) {
        handleAgentEnd(state);
      } else if (event.Error) {
        handleError(state, event.Error);
      } else if (event.SubagentStart) {
        handleSubagentStart(
          state,
          event.SubagentStart.subagent_id,
          event.SubagentStart.role_name,
          event.SubagentStart.task
        );
      } else if (event.SubagentMessageUpdate) {
        handleSubagentMessageUpdate(state, event.SubagentMessageUpdate.subagent_id, event.SubagentMessageUpdate.delta);
      } else if (event.SubagentToolStart) {
        handleSubagentToolStart(
          state,
          event.SubagentToolStart.subagent_id,
          event.SubagentToolStart.tool_call_id,
          event.SubagentToolStart.tool_name,
          event.SubagentToolStart.args
        );
      } else if (event.SubagentToolEnd) {
        handleSubagentToolEnd(
          state,
          event.SubagentToolEnd.subagent_id,
          event.SubagentToolEnd.tool_call_id,
          event.SubagentToolEnd.result,
          event.SubagentToolEnd.is_error
        );
      } else if (event.SubagentApprovalRequired) {
        handleSubagentApprovalRequired(
          state,
          event.SubagentApprovalRequired.subagent_id,
          event.SubagentApprovalRequired.prompt_id,
          event.SubagentApprovalRequired.tool_name,
          event.SubagentApprovalRequired.tool_input,
          event.SubagentApprovalRequired.danger_level,
          event.SubagentApprovalRequired.explanation
        );
      } else if (event.SubagentEnd) {
        handleSubagentEnd(
          state,
          event.SubagentEnd.subagent_id,
          event.SubagentEnd.success,
          event.SubagentEnd.iterations_used
        );
      }
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      const lastEntry = state.entries[state.entries.length - 1];
      if (lastEntry && lastEntry.type === 'turn') {
        if (lastEntry.blocks) {
          const block = lastEntry.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
          if (block && block.type === 'approval') {
            block.status = action.payload.approved ? 'approved' : 'denied';
            return;
          }
        }
        if (lastEntry.subagents) {
          for (const sa of Object.values(lastEntry.subagents)) {
            if (sa.blocks) {
              const saBlock = sa.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
              if (saBlock && saBlock.type === 'approval') {
                saBlock.status = action.payload.approved ? 'approved' : 'denied';
                return;
              }
            }
          }
        }
      }
    },
    clearChat: (state) => {
      state.entries = [];
      state.isProcessing = false;
    },
    agentAborted: (state) => {
      state.isProcessing = false;
      const last = state.entries[state.entries.length - 1];
      if (last && last.type === 'turn' && !last.endTime) {
        last.endTime = Date.now();
        if (last.blocks) {
          last.blocks.push({ type: 'error', text: '— Interrupted —' });
        }
      }
    },
    retryFromEntry: (state, action: PayloadAction<string>) => {
      const entryId = action.payload;
      const idx = state.entries.findIndex((e) => e.id === entryId);
      if (idx === -1) return;
      const userText = state.entries[idx].text ?? '';
      state.entries = state.entries.slice(0, idx);
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: userText,
      });
      state.isProcessing = true;
      state._resumedFromBackend = false;
    },
  },
  extraReducers: (builder) => {
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      if (state.entries.length > 0) return;
      const { messages, event_log } = action.payload;
      state.entries = [];
      state.isProcessing = false;

      let assistantIdx = 0;
      for (const msg of messages) {
        if (msg.role === 'user') {
          state.entries.push({
            id: `user-${Date.now()}-${Math.random()}`,
            type: 'user',
            text: msg.content,
          });
        } else if (msg.role === 'assistant') {
          const turnIdx = assistantIdx;
          assistantIdx++;
          const blocks: TurnBlock[] = [];

          if (event_log && Array.isArray(event_log)) {
            const turnEvents = event_log.filter(
              (e: EventLogEntry) =>
                e.turn_index === turnIdx &&
                (e.event_type === 'tool_call' || e.event_type === 'subagent' || e.event_type === 'thinking' || e.event_type === 'assistant')
            );
            for (const ev of turnEvents) {
              const payload: Record<string, unknown> =
                ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)
                  ? (ev.payload as Record<string, unknown>)
                  : {};
              if (ev.event_type === 'tool_call') {
                blocks.push({
                  type: 'tool',
                  call_id: `restored-${Math.random()}`,
                  name: (payload.name as string) ?? 'unknown',
                  args: payload.args ?? undefined,
                  result: (payload.args_summary as string) ?? '',
                  active: false,
                  is_error: !!payload.is_error,
                });
              } else if (ev.event_type === 'subagent') {
                const subId = payload.id as string | undefined;
                if (subId) {
                  blocks.push({
                    type: 'subagent_ref',
                    subagent_id: subId,
                  });
                }
              } else if (ev.event_type === 'thinking') {
                blocks.push({
                  type: 'thinking',
                  text: (payload.text as string) ?? '',
                  isStreaming: false,
                  startTime: payload.startTime as number | undefined,
                  endTime: payload.endTime as number | undefined,
                });
              } else if (ev.event_type === 'assistant') {
                blocks.push({
                  type: 'assistant',
                  text: (payload.text as string) ?? '',
                  isStreaming: false,
                });
              }
            }
          }

          if (!blocks.some((b) => b.type === 'assistant')) {
            blocks.push({ type: 'assistant', text: msg.content, isStreaming: false });
          }

          let startTime: number | undefined = undefined;
          let endTime: number | undefined = undefined;
          if (event_log && Array.isArray(event_log)) {
            const metaEvent = event_log.find((e: EventLogEntry) => e.turn_index === turnIdx && e.event_type === 'turn_meta');
            if (metaEvent && metaEvent.payload) {
              startTime = (metaEvent.payload as Record<string, unknown>).startTime as number | undefined;
              endTime = (metaEvent.payload as Record<string, unknown>).endTime as number | undefined;
            }
          }

          let subagents: Record<string, SubagentEntry> | undefined = undefined;
          if (event_log && Array.isArray(event_log)) {
            const subEvents = event_log.filter((e: EventLogEntry) => e.turn_index === turnIdx && e.event_type === 'subagent');
            if (subEvents.length > 0) {
              subagents = {};
              for (const ev of subEvents) {
                const payload: Record<string, unknown> =
                  ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)
                    ? (ev.payload as Record<string, unknown>)
                    : {};
                const subId = payload.id as string | undefined;
                if (subId) {
                  subagents[subId] = payload as unknown as SubagentEntry;
                }
              }
            }
          }

          state.entries.push({
            id: `turn-${turnIdx}-${Date.now()}`,
            type: 'turn',
            turnIndex: turnIdx,
            blocks,
            subagents,
            startTime,
            endTime,
          });
        }
      }

      state._resumedFromBackend = true;
    });
  },
});

// ── Need EventLogEntry type for resume logic ──────────────────────────

interface EventLogEntry {
  turn_index: number;
  event_type: string;
  payload: unknown;
  started_at: string | null;
  ended_at: string | null;
}

export const {
  userMessageSent,
  agentEventReceived,
  toolApprovalResponded,
  clearChat,
  agentAborted,
  cacheCurrentSession,
  restoreOrClearSession,
  retryFromEntry,
  runIdSet,
  runStateChanged,
} = chatSlice.actions;
export default chatSlice.reducer;

// ── Memoized entry-by-ID lookup (O(n) per state change, not O(n²)) ────

const entryMapCache = new WeakMap<ChatEntry[], Record<string, ChatEntry>>();

export function selectEntryById(state: { chat: ChatState }, entryId: string): ChatEntry | undefined {
  const entries = state.chat.entries;
  let map = entryMapCache.get(entries);
  if (!map) {
    map = {};
    for (const e of entries) map[e.id] = e;
    entryMapCache.set(entries, map);
  }
  return map[entryId];
}

export function selectPendingApprovalCount(state: { chat: ChatState }): number {
  let count = 0;
  for (const entry of state.chat.entries) {
    if (entry.type !== 'turn') continue;
    if (entry.blocks) {
      for (const b of entry.blocks) {
        if (b.type === 'approval' && b.status === 'pending') count++;
      }
    }
    if (entry.subagents) {
      for (const sa of Object.values(entry.subagents)) {
        for (const b of sa.blocks) {
          if (b.type === 'approval' && b.status === 'pending') count++;
        }
      }
    }
  }
  return count;
}

// ── Helpers ──────────────────────────────────────────────────────────

export function entriesToMessages(entries: ChatEntry[]): import('../project/projectSlice').FrontendMessage[] {
  const msgs: import('../project/projectSlice').FrontendMessage[] = [];
  for (const entry of entries) {
    if (entry.type === 'user' && entry.text) {
      msgs.push({ role: 'user', content: entry.text });
    } else if (entry.type === 'turn' && entry.blocks) {
      let assistantText = '';
      for (const block of entry.blocks) {
        if (block.type === 'assistant') {
          assistantText += block.text;
        }
      }
      if (assistantText.trim()) {
        msgs.push({ role: 'assistant', content: assistantText.trim() });
      }
    }
  }
  return msgs;
}

export function entriesToEventLog(entries: ChatEntry[]): {
  eventLog: unknown[];
  processTimeMs: number;
  thoughtTimeMs: number;
} {
  const eventLog: unknown[] = [];
  let processTimeMs = 0;
  let thoughtTimeMs = 0;
  let assistantIdx = 0;

  for (const entry of entries) {
    if (entry.type === 'turn' && entry.blocks) {
      let assistantText = '';
      for (const b of entry.blocks) {
        if (b.type === 'assistant') assistantText += b.text;
      }
      if (!assistantText.trim()) continue;

      if (entry.startTime && entry.endTime) {
        processTimeMs += entry.endTime - entry.startTime;
      }

      if (entry.startTime || entry.endTime) {
        eventLog.push({
          turn_index: assistantIdx,
          event_type: 'turn_meta',
          payload: { startTime: entry.startTime, endTime: entry.endTime },
        });
      }

      for (const b of entry.blocks) {
        if (b.type === 'thinking') {
          if (b.startTime && b.endTime) thoughtTimeMs += b.endTime - b.startTime;
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'thinking',
            payload: { text: b.text, startTime: b.startTime, endTime: b.endTime },
          });
        } else if (b.type === 'tool') {
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'tool_call',
            payload: { name: b.name, args: b.args, args_summary: b.result?.slice(0, 1000), is_error: b.is_error },
          });
        } else if (b.type === 'subagent_ref') {
          const sa = entry.subagents?.[b.subagent_id];
          if (sa) {
            eventLog.push({
              turn_index: assistantIdx,
              event_type: 'subagent',
              payload: sa,
            });
          }
        } else if (b.type === 'assistant') {
          eventLog.push({
            turn_index: assistantIdx,
            event_type: 'assistant',
            payload: { text: b.text },
          });
        }
      }
      assistantIdx++;
    }
  }

  return { eventLog, processTimeMs, thoughtTimeMs };
}
