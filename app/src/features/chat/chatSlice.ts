import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession } from '../project/projectSlice';

// ── Types ────────────────────────────────────────────────────────────

export type TurnBlock =
  | { type: 'assistant'; text: string; isStreaming: boolean }
  | { type: 'thinking'; text: string; isStreaming: boolean; startTime?: number; endTime?: number }
  | { type: 'tool'; call_id: string; name: string; args?: unknown; result: string; active: boolean; is_error: boolean; startTime?: number; endTime?: number }
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
  /** Latest backend-assigned turn id (R7). Events carrying this id route here. */
  turnId?: string;
  /** All turn ids associated with this chat entry (e.g., iterations). */
  turnIds?: string[];
  turnIndex?: number;
  text?: string;
  blocks?: TurnBlock[];
  startTime?: number;
  endTime?: number;
  subagentIds?: string[];
}

interface ChatState {
  entries: ChatEntry[];
  isProcessing: boolean;
  /** ID of the currently active Run (set when send_message returns). */
  runId: string | null;
  /** Current lifecycle state of the active Run. */
  runState: RunState | null;
  /** Per-Run last seen event seq (Stage 0 gap detection). */
  lastSeqByRun: Record<string, number>;
  /** Global dictionary of all subagents, decoupled from the turn tree (R8). */
  subagents: Record<string, SubagentEntry>;
  /** Drill-down navigation stack for the subagent detail page (Stage 2). */
  viewingSubagentPath: { id: string; name: string }[];
  /** True while a gap-resync is in flight (prevents re-entrancy). */
  resyncing: boolean;
  /** Transient: turn_id from the event currently being processed (R7). */
  _pendingTurnId?: string;
  entriesBySession: Record<string, ChatEntry[]>;
  processingBySession: Record<string, boolean>;
  subagentsBySession: Record<string, Record<string, SubagentEntry>>;
  _resumedFromBackend: boolean;
}

const initialState: ChatState = {
  entries: [],
  isProcessing: false,
  runId: null,
  runState: null,
  lastSeqByRun: {},
  subagents: {},
  viewingSubagentPath: [],
  resyncing: false,
  entriesBySession: {},
  processingBySession: {},
  subagentsBySession: {},
  _resumedFromBackend: false,
};

// ── Event payload types ──────────────────────────────────────────────

interface DeltaPayload {
  Text?: string;
  Thinking?: string;
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
  // Identity + ordering (Stage 0 envelope)
  seq?: number;
  event_id?: string;
  run_id?: string;
  turn_id?: string;
  parent_call_id?: string;
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
  // Approval resolution (approval_resolved event)
  choice?: string;
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

// ── Block helpers (shared between main agent + subagent) ─────────────

type AnyBlock = TurnBlock | SubagentBlock;

function closeStreamingBlock(blocks: AnyBlock[] | undefined): void {
  if (!blocks || blocks.length === 0) return;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if ('isStreaming' in block && block.isStreaming) {
      block.isStreaming = false;
      if (block.type === 'thinking') {
        block.endTime = Date.now();
      }
    } else {
      break;
    }
  }
}

function appendDeltaToBlocks(
  blocks: AnyBlock[],
  delta: DeltaPayload
): void {
  const appendToType = (text: string, targetType: 'assistant' | 'thinking') => {
    let targetBlock = null;
    for (let i = blocks.length - 1; i >= 0; i--) {
      const b = blocks[i];
      if ('isStreaming' in b && b.isStreaming) {
        if (b.type === targetType) {
          targetBlock = b;
          break;
        }
      } else {
        break;
      }
    }

    if (!targetBlock) {
      if (targetType === 'thinking') {
        const lastBlock = blocks[blocks.length - 1];
        if (lastBlock && lastBlock.type === 'assistant' && 'isStreaming' in lastBlock && lastBlock.isStreaming) {
          blocks.splice(blocks.length - 1, 0, { type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 2];
        } else {
          blocks.push({ type: 'thinking', text: '', isStreaming: true, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 1];
        }
      } else {
        blocks.push({ type: 'assistant', text: '', isStreaming: true });
        targetBlock = blocks[blocks.length - 1];
      }
    }
    
    if (targetBlock.type === 'assistant' || targetBlock.type === 'thinking') {
      targetBlock.text += text;
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
        if (before) appendToType(before, 'assistant');
        textChunk = textChunk.substring(thinkStartIdx + 7);
      } else if (thinkEndIdx !== -1) {
        // Handle </think>
        const before = textChunk.substring(0, thinkEndIdx);
        if (before) appendToType(before, 'thinking');
        textChunk = textChunk.substring(thinkEndIdx + 8);
      }
    }

    if (textChunk) {
      appendToType(textChunk, 'assistant');
    }
  } 
  
  if (typeof delta.Thinking === 'string') {
    appendToType(delta.Thinking, 'thinking');
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
  // R7: if the current event carries a turn_id, route by it.
  if (state._pendingTurnId) {
    const byId = state.entries.find(
      (e) => e.type === 'turn' && (e.turnId === state._pendingTurnId || e.turnIds?.includes(state._pendingTurnId!))
    );
    if (byId && byId.type === 'turn') return byId;
  }
  for (let i = state.entries.length - 1; i >= 0; i--) {
    const entry = state.entries[i];
    if (entry.type === 'turn' && !entry.endTime) {
      return entry;
    }
  }
  return undefined;
}

function getOrCreateSubagent(
  state: ChatState,
  subagentId: string,
  roleName: string,
  task: string
): SubagentEntry {
  if (!state.subagents[subagentId]) {
    state.subagents[subagentId] = {
      id: subagentId,
      role_name: roleName,
      task,
      status: 'working',
      blocks: [],
      startTime: Date.now(),
    };
  }
  return state.subagents[subagentId];
}

// ── Event handlers ───────────────────────────────────────────────────

function handleTurnStart(state: ChatState, turnIndex: number, turnId?: string): void {
  // If we have a turn_id and an open turn already uses it, just update its index.
  if (turnId) {
    const existing = state.entries.find(
      (e) => e.type === 'turn' && (e.turnId === turnId || e.turnIds?.includes(turnId))
    );
    if (existing && existing.type === 'turn') {
      existing.turnIndex = turnIndex;
      return;
    }
  }
  const last = state.entries[state.entries.length - 1];
  if (last && last.type === 'turn' && !last.endTime) {
    // Adopt the last open turn (either unassigned, or from a previous iteration).
    last.turnIndex = turnIndex;
    if (turnId) {
      last.turnId = turnId;
      if (!last.turnIds) last.turnIds = [];
      if (!last.turnIds.includes(turnId)) last.turnIds.push(turnId);
    }
  } else {
    state.entries.push({
      id: `turn-${turnIndex}-${Date.now()}`,
      type: 'turn',
      turnId,
      turnIds: turnId ? [turnId] : [],
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
      startTime: Date.now(),
    });
  }
}

function handleToolUpdate(state: ChatState, toolCallId: string, partialResult: unknown): void {
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    let block = undefined;
    for (let i = lastEntry.blocks.length - 1; i >= 0; i--) {
      const b = lastEntry.blocks[i];
      if (b.type === 'tool' && (toolCallId ? b.call_id === toolCallId : b.active)) {
        block = b;
        break;
      }
    }
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
    let block = undefined;
    for (let i = lastEntry.blocks.length - 1; i >= 0; i--) {
      const b = lastEntry.blocks[i];
      if (b.type === 'tool' && (toolCallId ? b.call_id === toolCallId : b.active)) {
        block = b;
        break;
      }
    }
    if (block && block.type === 'tool') {
      block.active = false;
      block.is_error = isError;
      block.endTime = Date.now();
      block.result = truncateResult(stringifyResult(result));
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
  // Close ALL open turns, not just the last one. The backend emits a fresh
  // TurnStarted (with a new turn_id) per iteration, so a single Run can have
  // multiple open turn entries. Leaving earlier ones open causes them to stay
  // stuck in "Processed..." forever.
  for (const entry of state.entries) {
    if (entry.type === 'turn' && !entry.endTime) {
      entry.endTime = Date.now();
      stopDanglingSubagents(state, entry);
    }
  }
}

function handleError(state: ChatState, errorText: string): void {
  state.isProcessing = false;
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
    
    // Stop any dangling subagents owned by this turn
    stopDanglingSubagents(state, lastEntry);
    
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
    getOrCreateSubagent(state, subagentId, safeRoleName, safeTask);
    if (!turn.subagentIds) turn.subagentIds = [];
    if (!turn.subagentIds.includes(subagentId)) turn.subagentIds.push(subagentId);
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
  const sa = state.subagents[subagentId];
  if (sa) {
    appendDeltaToBlocks(sa.blocks, delta);
  }
}

function handleSubagentToolStart(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
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

function handleSubagentToolUpdate(state: ChatState, subagentId: string, toolCallId: string, partialResult: unknown): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    let block = undefined;
    for (let i = sa.blocks.length - 1; i >= 0; i--) {
      const b = sa.blocks[i];
      if (b.type === 'tool' && (toolCallId ? b.call_id === toolCallId : b.active)) {
        block = b;
        break;
      }
    }
    if (block && block.type === 'tool') {
      block.result += typeof partialResult === 'string' ? partialResult : JSON.stringify(partialResult);
    }
  }
}

function handleSubagentToolEnd(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    let block = undefined;
    for (let i = sa.blocks.length - 1; i >= 0; i--) {
      const b = sa.blocks[i];
      if (b.type === 'tool' && (toolCallId ? b.call_id === toolCallId : b.active)) {
        block = b;
        break;
      }
    }
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
  const sa = state.subagents[subagentId];
  if (sa) {
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
  const sa = state.subagents[subagentId];
  if (sa) {
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

function handleTurnEnded(state: ChatState): void {
  // An iteration ended: finalize any still-streaming block on the active turn
  // so it doesn't bleed into the next iteration. (Previously a no-op black hole.)
  const lastEntry = state.entries[state.entries.length - 1];
  if (lastEntry && lastEntry.type === 'turn' && lastEntry.blocks) {
    closeStreamingBlock(lastEntry.blocks);
  }
}

function stopDanglingSubagents(state: ChatState, turn: ChatEntry): void {
  const ids = turn.subagentIds;
  if (!ids) return;
  for (const id of ids) {
    const sa = state.subagents[id];
    if (sa && sa.status === 'working') {
      sa.status = 'error';
      sa.endTime = Date.now();
      sa.blocks.forEach((b) => {
        if (b.isStreaming) {
          b.isStreaming = false;
          if (b.type === 'thinking') b.endTime = Date.now();
        }
      });
    }
  }
}

function resolveApprovalBlock(state: ChatState, promptId: string, choice?: string): void {
  if (!promptId) return;
  // The backend's ApprovalChoice serializes to a string for the variants the
  // UI emits (AllowOnce/AllowSession/AllowPersistent/Deny/DenyPersistent).
  // AllowFor(Duration) would serialize to an object — guard against that so a
  // non-string choice never crashes here. Treat anything containing "deny" as
  // a denial, everything else as approval.
  const choiceStr = typeof choice === 'string' ? choice : '';
  const approved = !choiceStr.toLowerCase().includes('deny');
  for (const entry of state.entries) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    for (const b of entry.blocks) {
      if (b.type === 'approval' && b.prompt_id === promptId) {
        b.status = approved ? 'approved' : 'denied';
        return;
      }
    }
  }
  for (const sa of Object.values(state.subagents)) {
    for (const b of sa.blocks) {
      if (b.type === 'approval' && b.prompt_id === promptId) {
        b.status = approved ? 'approved' : 'denied';
        return;
      }
    }
  }
}

/**
 * Resync a Run's event stream after a gap (B2 self-heal, Stage 3).
 *
 * When the frontend detects a missing seq (broadcast lag), it dispatches this
 * thunk with the last seen seq. The backend replays all envelopes with
 * seq > fromSeq from its persisted JSONL log, and we re-dispatch each one
 * through `agentEventReceived`.
 */
export const resyncRun = createAsyncThunk<
  void,
  { runId: string; fromSeq: number },
  { state: { chat: ChatState } }
>('chat/resyncRun', async ({ runId, fromSeq }, { dispatch, getState }) => {
  if (getState().chat.resyncing) return;
  dispatch(setResyncing(true));
  try {
    const lines = await invoke<string[]>('replay_since', { runId, fromSeq });
    for (const line of lines) {
      let payload: Record<string, unknown>;
      try {
        payload = JSON.parse(line);
      } catch {
        continue;
      }
      dispatch(agentEventReceived(payload));
    }
  } catch (e) {
    console.error('[resyncRun] failed to replay events:', e);
  } finally {
    dispatch(setResyncing(false));
  }
});

// ── Slice ────────────────────────────────────────────────────────────

export const chatSlice = createSlice({
  name: 'chat',
  initialState,
  reducers: {
    cacheCurrentSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      state.entriesBySession[sessionId] = state.entries;
      state.processingBySession[sessionId] = state.isProcessing;
      state.subagentsBySession[sessionId] = state.subagents;
    },
    restoreOrClearSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      const cached = state.entriesBySession[sessionId];
      if (cached) {
        state.entries = cached;
        state.isProcessing = state.processingBySession[sessionId] ?? false;
        state.subagents = state.subagentsBySession[sessionId] ?? {};
      } else {
        state.entries = [];
        state.isProcessing = false;
        state.subagents = {};
      }
      state.viewingSubagentPath = [];
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
      let raw: Record<string, unknown>;
      if (typeof action.payload === 'string') {
        try {
          raw = JSON.parse(action.payload);
        } catch {
          return;
        }
      } else {
        raw = action.payload as Record<string, unknown>;
      }

      // Every event now arrives as a stamped RunEvent envelope
      // ({ event, seq, event_id, run_id, ... }). The legacy AgentEvent
      // format is no longer produced by the backend (Stage 1, R4).
      if (!raw || typeof raw.event !== 'string') return;
      const ev = raw as unknown as RunEventPayload;

      // Gap detection (Stage 0/3): every event carries a per-Run monotonic seq.
      // A gap means events were lost in transit (e.g. broadcast lag). We warn
      // and signal the listener to trigger a resync from the persisted log.
      if (typeof ev.seq === 'number' && typeof ev.run_id === 'string') {
        const prev = state.lastSeqByRun[ev.run_id];
        if (prev !== undefined && ev.seq > prev + 1) {
          console.warn(
            `[agent-event] gap detected for run ${ev.run_id}: expected ${prev + 1}, got ${ev.seq} (${ev.seq - prev - 1} missing); triggering resync`
          );
          // Self-heal (B2): dispatch a window event the listener picks up to
          // replay missing envelopes from the backend's persisted log. We
          // can't dispatch a thunk from inside a reducer, so we defer it.
          Promise.resolve().then(() => {
            window.dispatchEvent(new CustomEvent('agent-event-gap', {
              detail: { runId: ev.run_id, fromSeq: prev },
            }));
          });
        }
        state.lastSeqByRun[ev.run_id] = ev.seq;
      }

      // Lifecycle (handled directly; not routed through block handlers)
      if (ev.event === 'state_changed' && ev.to) {
        state.runState = ev.to as RunState;
        if (ev.to === 'completed' || ev.to === 'cancelled' || ev.to === 'failed') {
          state.isProcessing = false;
        }
      } else if (ev.event === 'run_started') {
        state.runState = 'running';
      } else if (ev.event === 'run_paused') {
        state.runState = 'paused';
      } else if (ev.event === 'run_resumed') {
        state.runState = 'running';
      } else if (ev.event === 'run_completed' || ev.event === 'run_cancelled') {
        state.isProcessing = false;
        state.runId = null;
        // Close the active turn + stop any dangling subagents.
        handleAgentEnd(state);
      } else if (ev.event === 'run_failed') {
        state.isProcessing = false;
        state.runId = null;
        // Display the failure reason (pushes an error block).
        handleError(state, ev.error ?? 'run failed');
        // B9: close ALL open turns (not just the last one) so none stays stuck
        // in "Processed..." forever. handleError already stopped dangling
        // subagents on the active turn; close the rest too.
        for (const entry of state.entries) {
          if (entry.type === 'turn' && !entry.endTime) {
            entry.endTime = Date.now();
            stopDanglingSubagents(state, entry);
          }
        }
      }

      // Set the pending turn_id so main-agent handlers (which use
      // getActiveTurn) route to the correct turn by id (R7).
      state._pendingTurnId = ev.turn_id;

      // Block-routing: direct RunEvent -> handler (no legacy shim, R4)
      switch (ev.event) {
        case 'turn_started':
          handleTurnStart(state, ev.index ?? 0, ev.turn_id);
          break;
        case 'turn_ended':
          handleTurnEnded(state);
          break;
        case 'message_update':
        case 'model_streaming':
          if (ev.subagent_id) handleSubagentMessageUpdate(state, ev.subagent_id, ev.delta ?? {});
          else handleMessageUpdate(state, ev.delta ?? {});
          break;
        case 'message_end':
          if (!ev.subagent_id) handleMessageEnd(state);
          break;
        case 'tool_started':
          if (ev.subagent_id) handleSubagentToolStart(state, ev.subagent_id, ev.call_id ?? '', ev.name ?? '', ev.args);
          else handleToolStart(state, ev.call_id ?? '', ev.name ?? '', ev.args);
          break;
        case 'tool_update':
          if (ev.subagent_id) handleSubagentToolUpdate(state, ev.subagent_id, ev.call_id ?? '', ev.partial ?? '');
          else handleToolUpdate(state, ev.call_id ?? '', ev.partial ?? '');
          break;
        case 'tool_ended':
          if (ev.subagent_id) handleSubagentToolEnd(state, ev.subagent_id, ev.call_id ?? '', ev.result ?? '', ev.is_error ?? false);
          else handleToolEnd(state, ev.call_id ?? '', ev.result ?? '', ev.is_error ?? false);
          break;
        case 'approval_required':
          if (ev.subagent_id) handleSubagentApprovalRequired(state, ev.subagent_id, ev.prompt_id ?? '', ev.tool_name ?? '', ev.tool_input, ev.danger_level ?? '', ev.explanation ?? '');
          else handleApprovalRequired(state, ev.prompt_id ?? '', ev.tool_name ?? '', ev.tool_input, ev.danger_level ?? '', ev.explanation ?? '');
          break;
        case 'approval_resolved':
          resolveApprovalBlock(state, ev.prompt_id ?? '', ev.choice);
          break;
        case 'error':
          handleError(state, ((ev as unknown as Record<string, unknown>).message as string | undefined) ?? 'unknown error');
          break;
        case 'subagent_started':
          handleSubagentStart(state, ev.subagent_id ?? '', ev.role_name, ev.task ?? '');
          break;
        case 'subagent_ended':
          handleSubagentEnd(state, ev.subagent_id ?? '', ev.success ?? false, ev.iterations_used);
          break;
        // run_created / message_start / model_call_* / input_requested /
        // context_compacted / process_spawned / process_killed: not routed yet.
        default:
          break;
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
        // Subagent approvals live in the global dict now (R8)
        for (const sa of Object.values(state.subagents)) {
          if (sa.blocks) {
            const saBlock = sa.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
            if (saBlock && saBlock.type === 'approval') {
              saBlock.status = action.payload.approved ? 'approved' : 'denied';
              return;
            }
          }
        }
      }
    },
    clearChat: (state) => {
      state.entries = [];
      state.subagents = {};
      state.viewingSubagentPath = [];
      state.isProcessing = false;
    },
    agentAborted: (state) => {
      state.isProcessing = false;
      const last = state.entries[state.entries.length - 1];
      if (last && last.type === 'turn' && !last.endTime) {
        last.endTime = Date.now();
        stopDanglingSubagents(state, last);
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
    viewSubagent: (state, action: PayloadAction<{ id: string; name: string }>) => {
      state.viewingSubagentPath.push(action.payload);
    },
    popSubagentView: (state) => {
      state.viewingSubagentPath.pop();
    },
    clearSubagentView: (state) => {
      state.viewingSubagentPath = [];
    },
    setResyncing: (state, action: PayloadAction<boolean>) => {
      state.resyncing = action.payload;
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

          let subagentIds: string[] | undefined = undefined;
          if (event_log && Array.isArray(event_log)) {
            const subEvents = event_log.filter((e: EventLogEntry) => e.turn_index === turnIdx && e.event_type === 'subagent');
            if (subEvents.length > 0) {
              subagentIds = [];
              for (const ev of subEvents) {
                const payload: Record<string, unknown> =
                  ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)
                    ? (ev.payload as Record<string, unknown>)
                    : {};
                const subId = payload.id as string | undefined;
                if (subId) {
                  subagentIds.push(subId);
                  state.subagents[subId] = payload as unknown as SubagentEntry;
                }
              }
            }
          }

          state.entries.push({
            id: `turn-${turnIdx}-${Date.now()}`,
            type: 'turn',
            turnIndex: turnIdx,
            blocks,
            subagentIds,
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
  viewSubagent,
  popSubagentView,
  clearSubagentView,
  setResyncing,
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

export function selectSubagentById(state: { chat: ChatState }, subagentId: string): SubagentEntry | undefined {
  return state.chat.subagents[subagentId];
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
  }
  // Subagent approvals live in the global dict (R8)
  for (const sa of Object.values(state.chat.subagents)) {
    for (const b of sa.blocks) {
      if (b.type === 'approval' && b.status === 'pending') count++;
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

export function entriesToEventLog(
  entries: ChatEntry[],
  subagents: Record<string, SubagentEntry>
): {
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
          const sa = subagents[b.subagent_id];
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
