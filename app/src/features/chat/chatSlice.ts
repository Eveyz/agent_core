import { createSlice, createAsyncThunk, createSelector, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession } from '../project/projectSlice';

// ── Types ────────────────────────────────────────────────────────────

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
  runIdBySession: Record<string, string | null>;
  activeSessionId: string | null;
  _resumedFromBackend: boolean;
  /** Per-message buffer for cross-chunk <think> tag reassembly (P0-4). */
  _thinkBuffers: Record<string, string>;
  /** Transient gap info set by the reducer, consumed by listenerMiddleware (P2-1). */
  _pendingGap: { runId: string; fromSeq: number } | null;
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
  runIdBySession: {},
  activeSessionId: null,
  _resumedFromBackend: false,
  _thinkBuffers: {},
  _pendingGap: null,
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
  message_id?: string;
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

function blockMessageId(b: AnyBlock): string | undefined {
  return 'message_id' in b ? (b as { message_id?: string }).message_id : undefined;
}

function closeStreamingBlock(blocks: AnyBlock[] | undefined, messageId?: string): void {
  if (!blocks || blocks.length === 0) return;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if ('isStreaming' in block && block.isStreaming) {
      // When scoped to a message_id, only finalize blocks that belong to it;
      // leave other messages' streaming blocks untouched.
      if (messageId !== undefined && blockMessageId(block) !== messageId) {
        continue;
      }
      block.isStreaming = false;
      if (block.type === 'thinking') {
        block.endTime = Date.now();
      }
    } else if (messageId === undefined) {
      // Unscoped close: stop at the first finalized block (preserves the
      // tail-close behaviour tool/approval handlers rely on).
      break;
    }
  }
}

// ── Cross-chunk <think> tag reassembly (P0-4) ────────────────────────
// DeepSeek streams <think>...</think> tags inline in Text deltas. If a tag
// is split across two deltas, naive includes() matching fails. We buffer the
// trailing partial-tag suffix and prepend it to the next chunk.

const THINK_OPEN = '<think>';
const THINK_CLOSE = '</think>';

function getPartialThinkTagSuffix(text: string): string {
  let longest = '';
  for (const tag of [THINK_OPEN, THINK_CLOSE]) {
    for (let i = 1; i < tag.length; i++) {
      const prefix = tag.substring(0, i);
      if (text.endsWith(prefix) && prefix.length > longest.length) {
        longest = prefix;
      }
    }
  }
  return longest;
}

function processThinkBuffer(
  buffers: Record<string, string>,
  key: string,
  delta: DeltaPayload
): DeltaPayload {
  if (typeof delta.Text !== 'string' || !delta.Text) return delta;
  const prev = buffers[key] ?? '';
  let text = prev + delta.Text;
  buffers[key] = '';
  const partial = getPartialThinkTagSuffix(text);
  if (partial) {
    buffers[key] = text.slice(text.length - partial.length);
    text = text.slice(0, text.length - partial.length);
  }
  return { ...delta, Text: text };
}

function appendDeltaToBlocks(
  blocks: AnyBlock[],
  delta: DeltaPayload,
  messageId?: string
): void {
  const appendToType = (text: string, targetType: 'assistant' | 'thinking') => {
    // Route by identity: find the most recent streaming block that belongs to
    // this message and matches the target type. When no message_id is present
    // (e.g. restored entries), fall back to the legacy "any streaming block"
    // match so undefined === undefined.
    let targetBlock: AnyBlock | null = null;
    for (let i = blocks.length - 1; i >= 0; i--) {
      const b = blocks[i];
      if ('isStreaming' in b && b.isStreaming && b.type === targetType && blockMessageId(b) === messageId) {
        targetBlock = b;
        break;
      }
    }

    if (!targetBlock) {
      if (targetType === 'thinking') {
        const lastBlock = blocks[blocks.length - 1];
        if (lastBlock && lastBlock.type === 'assistant' && 'isStreaming' in lastBlock && lastBlock.isStreaming && blockMessageId(lastBlock) === messageId) {
          blocks.splice(blocks.length - 1, 0, { type: 'thinking', text: '', isStreaming: true, message_id: messageId, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 2];
        } else {
          blocks.push({ type: 'thinking', text: '', isStreaming: true, message_id: messageId, startTime: Date.now() });
          targetBlock = blocks[blocks.length - 1];
        }
      } else {
        blocks.push({ type: 'assistant', text: '', isStreaming: true, message_id: messageId });
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

export function stringifyResult(result: unknown): string {
  if (result === undefined || result === null) return '';
  if (typeof result === 'string') return result;
  try {
    const s = JSON.stringify(result, null, 2);
    return typeof s === 'string' ? s : String(result);
  } catch {
    return String(result);
  }
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
      // Use the backend-assigned turn_id as the stable identity when present
      // (R7/Tier-3) so blocks/events can be located by id instead of by
      // position. Falls back to a positional id only when no turn_id is known.
      id: turnId ? `turn-${turnId}` : `turn-${turnIndex}-${Date.now()}`,
      type: 'turn',
      turnId,
      turnIds: turnId ? [turnId] : [],
      turnIndex,
      blocks: [],
      startTime: Date.now(),
    });
  }
}

function handleMessageStart(state: ChatState, messageId: string | undefined): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    turn.blocks.push({
      type: 'thinking',
      text: '',
      isStreaming: true,
      message_id: messageId,
      startTime: Date.now()
    });
  }
}

function handleMessageUpdate(state: ChatState, messageId: string | undefined, delta: DeltaPayload): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    const processed = processThinkBuffer(state._thinkBuffers, messageId ?? '_nomsg', delta);
    appendDeltaToBlocks(turn.blocks, processed, messageId);
  }
}


function handleMessageEnd(state: ChatState, messageId?: string): void {
  if (messageId) delete state._thinkBuffers[messageId];
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks, messageId);
  }
}

function handleToolStart(
  state: ChatState,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    turn.blocks.push({
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
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    let block = undefined;
    for (let i = turn.blocks.length - 1; i >= 0; i--) {
      const b = turn.blocks[i];
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
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    let block = undefined;
    for (let i = turn.blocks.length - 1; i >= 0; i--) {
      const b = turn.blocks[i];
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
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    turn.blocks.push({
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
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    
    // Stop any dangling subagents owned by this turn
    stopDanglingSubagents(state, turn);
    
    const lastBlock = turn.blocks[turn.blocks.length - 1];
    const isRetryError = errorText.toLowerCase().includes('retrying model call');
    
    if (lastBlock && lastBlock.type === 'error') {
      const lastText = typeof lastBlock.text === 'string' ? lastBlock.text : '';
      if (isRetryError && lastText.toLowerCase().includes('retrying model call')) {
        lastBlock.text = errorText;
        return;
      }
    }
    
    turn.blocks.push({ type: 'error', text: errorText });
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
  parentCallId: string | undefined,
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
      turn.blocks.push({ type: 'subagent_ref', subagent_id: subagentId, parent_call_id: parentCallId });
    }
  }
}

function handleSubagentMessageStart(
  state: ChatState,
  subagentId: string,
  messageId: string | undefined
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    if (!sa.blocks) sa.blocks = [];
    sa.blocks.push({
      type: 'thinking',
      text: '',
      isStreaming: true,
      message_id: messageId,
      startTime: Date.now()
    });
  }
}

function handleSubagentMessageUpdate(
  state: ChatState,
  subagentId: string,
  messageId: string | undefined,
  delta: DeltaPayload
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    const processed = processThinkBuffer(state._thinkBuffers, messageId ?? `_sa_${subagentId}`, delta);
    appendDeltaToBlocks(sa.blocks, processed, messageId);
  }
}

function handleSubagentMessageEnd(state: ChatState, subagentId: string, messageId?: string): void {
  if (messageId) delete state._thinkBuffers[messageId];
  const sa = state.subagents[subagentId];
  if (sa) {
    closeStreamingBlock(sa.blocks, messageId);
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
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    // Force close any tools that might have missed their end events
    for (const b of turn.blocks) {
      if (b.type === 'tool' && b.active) {
        b.active = false;
        if (!b.endTime) b.endTime = Date.now();
        if (b.result === undefined) b.result = '';
      }
    }
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

// ── Core event processing (extracted for batch dispatch, PERF-1) ──────

function processSingleEvent(state: ChatState, payload: string | Record<string, unknown>): void {
  let raw: Record<string, unknown>;
  if (typeof payload === 'string') {
    try {
      raw = JSON.parse(payload);
    } catch {
      return;
    }
  } else {
    raw = payload as Record<string, unknown>;
  }

  if (!raw || typeof raw.event !== 'string') return;
  const ev = raw as unknown as RunEventPayload;

  // When a new run is created for the active session, claim it
  if (ev.event === 'run_created') {
    if (ev.session_id === state.activeSessionId) {
      state.runId = ev.run_id ?? null;
    }
  }

  // Ignore all events that don't belong to the active run
  if (ev.run_id !== state.runId) {
    return;
  }

  // Gap detection (Stage 0/3)
  if (typeof ev.seq === 'number' && typeof ev.run_id === 'string') {
    const prev = state.lastSeqByRun[ev.run_id];
    if (prev !== undefined && ev.seq > prev + 1) {
      console.warn(
        `[agent-event] gap detected for run ${ev.run_id}: expected ${prev + 1}, got ${ev.seq} (${ev.seq - prev - 1} missing); triggering resync`
      );
      state._pendingGap = { runId: ev.run_id, fromSeq: prev };
    }
    state.lastSeqByRun[ev.run_id] = ev.seq;
  }

  // Lifecycle
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
    handleAgentEnd(state);
  } else if (ev.event === 'run_failed') {
    state.isProcessing = false;
    state.runId = null;
    handleError(state, ev.error ?? 'run failed');
    for (const entry of state.entries) {
      if (entry.type === 'turn' && !entry.endTime) {
        entry.endTime = Date.now();
        stopDanglingSubagents(state, entry);
      }
    }
  }

  state._pendingTurnId = ev.turn_id;

  switch (ev.event) {
    case 'turn_started':
      handleTurnStart(state, ev.index ?? 0, ev.turn_id);
      break;
    case 'turn_ended':
      handleTurnEnded(state);
      break;
    case 'message_start':
      if (ev.subagent_id) handleSubagentMessageStart(state, ev.subagent_id, ev.message_id);
      else handleMessageStart(state, ev.message_id);
      break;
    case 'message_update':
    case 'model_streaming':
      if (ev.subagent_id) handleSubagentMessageUpdate(state, ev.subagent_id, ev.message_id, ev.delta ?? {});
      else handleMessageUpdate(state, ev.message_id, ev.delta ?? {});
      break;
    case 'message_end':
      if (ev.subagent_id) handleSubagentMessageEnd(state, ev.subagent_id, ev.message_id);
      else handleMessageEnd(state, ev.message_id);
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
      handleSubagentStart(state, ev.subagent_id ?? '', ev.parent_call_id, ev.role_name, ev.task ?? '');
      break;
    case 'subagent_ended':
      handleSubagentEnd(state, ev.subagent_id ?? '', ev.success ?? false, ev.iterations_used);
      break;
    default:
      break;
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
  { runId: string; fromSeq: number }
>('chat/resyncRun', async ({ runId, fromSeq }, { dispatch, getState }) => {
  const state = getState() as { chat: ChatState };
  if (state.chat.resyncing) return;
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
      state.runIdBySession[sessionId] = state.runId;
    },
    restoreOrClearSession: (state, action: PayloadAction<string>) => {
      const sessionId = action.payload;
      state.activeSessionId = sessionId;
      const cached = state.entriesBySession[sessionId];
      if (cached) {
        state.entries = cached;
        state.isProcessing = state.processingBySession[sessionId] ?? false;
        state.subagents = state.subagentsBySession[sessionId] ?? {};
        state.runId = state.runIdBySession[sessionId] ?? null;
      } else {
        state.entries = [];
        state.isProcessing = false;
        state.subagents = {};
        state.runId = null;
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
      processSingleEvent(state, action.payload);
    },
    agentEventsBatch: (state, action: PayloadAction<Array<string | Record<string, unknown>>>) => {
      // PERF-1: Process all buffered events in a single reducer pass.
      // This avoids N selector recomputations per frame during streaming.
      for (const payload of action.payload) {
        processSingleEvent(state, payload);
      }
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      // Resolve by prompt_id across every turn + subagent (prompt_id is the
      // stable identity), not just the last entry.
      for (const entry of state.entries) {
        if (entry.type !== 'turn' || !entry.blocks) continue;
        const block = entry.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
        if (block && block.type === 'approval') {
          block.status = action.payload.approved ? 'approved' : 'denied';
          return;
        }
      }
      // Subagent approvals live in the global dict (R8)
      for (const sa of Object.values(state.subagents)) {
        if (!sa.blocks) continue;
        const saBlock = sa.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
        if (saBlock && saBlock.type === 'approval') {
          saBlock.status = action.payload.approved ? 'approved' : 'denied';
          return;
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
    retryFromEntry: (state, action: PayloadAction<{ id: string; text?: string }>) => {
      const entryId = action.payload.id;
      const idx = state.entries.findIndex((e) => e.id === entryId);
      if (idx === -1) return;
      const userText = action.payload.text ?? state.entries[idx].text ?? '';
      // Truncate everything from the retried entry onward, then push the new message.
      state.entries.splice(idx);
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
    clearPendingGap: (state) => {
      state._pendingGap = null;
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
  agentEventsBatch,
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
  clearPendingGap,
} = chatSlice.actions;
export default chatSlice.reducer;

// ── Memoized selectors ───────────────────────────────────────────────

// PERF-6: Memoized entry ID array. Only recomputes when entries are added or
// removed, not when text content changes during streaming.
export const selectEntryIds = createSelector(
  [(state: { chat: ChatState }) => state.chat.entries],
  (entries) => entries.map((e) => e.id)
);

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

export const selectPendingApprovalCount = createSelector(
  [
    (state: { chat: ChatState }) => state.chat.entries,
    (state: { chat: ChatState }) => state.chat.subagents,
  ],
  (entries, subagents) => {
    let count = 0;
    for (const entry of entries) {
      if (entry.type !== 'turn') continue;
      if (entry.blocks) {
        for (const b of entry.blocks) {
          if (b.type === 'approval' && b.status === 'pending') count++;
        }
      }
    }
    for (const sa of Object.values(subagents)) {
      for (const b of sa.blocks) {
        if (b.type === 'approval' && b.status === 'pending') count++;
      }
    }
    return count;
  }
);

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
