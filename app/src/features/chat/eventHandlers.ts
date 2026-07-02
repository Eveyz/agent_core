import type {
  ChatState,
  ChatEntry,
  SubagentEntry,
  DeltaPayload,
  RunEventPayload,
  RunState,
  TodoItem,
} from './types';
import {
  closeStreamingBlock,
  processThinkBuffer,
  appendDeltaToBlocks,
  truncateResult,
  stringifyResult,
} from './utils';
import type { AnyBlock } from './utils';

// ── Block-level helpers (shared between main agent + subagent) ───────

/** Find a tool block by call_id (or the first active tool block if callId is empty). */
function findToolBlock(blocks: AnyBlock[] | undefined, callId: string): AnyBlock | undefined {
  if (!blocks) return undefined;
  for (let i = blocks.length - 1; i >= 0; i--) {
    const b = blocks[i];
    if (b.type === 'tool' && (callId ? b.call_id === callId : b.active)) {
      return b;
    }
  }
  return undefined;
}

/** Close ALL streaming blocks and active tool blocks. Used on turn/subagent end. */
function finalizeAllBlocks(blocks: AnyBlock[] | undefined): void {
  if (!blocks) return;
  for (const b of blocks) {
    if ('isStreaming' in b && b.isStreaming) {
      b.isStreaming = false;
      if (b.type === 'thinking') b.endTime = Date.now();
    }
    if (b.type === 'tool' && b.active) {
      b.active = false;
      if (!b.endTime) b.endTime = Date.now();
      if (b.result === undefined) b.result = '';
    }
  }
}

// ── Unified block operations (work on any block array) ──────────────

function pushMessageStart(blocks: AnyBlock[], messageId: string | undefined): void {
  blocks.push({
    type: 'thinking',
    text: '',
    isStreaming: true,
    message_id: messageId,
    startTime: Date.now(),
  });
}

function applyMessageUpdate(
  state: ChatState,
  blocks: AnyBlock[],
  thinkKey: string,
  messageId: string | undefined,
  delta: DeltaPayload
): void {
  const processed = processThinkBuffer(state._thinkBuffers, thinkKey, delta);
  appendDeltaToBlocks(blocks, processed, messageId);
}

function applyMessageEnd(
  state: ChatState,
  blocks: AnyBlock[],
  messageId?: string
): void {
  if (messageId) delete state._thinkBuffers[messageId];
  closeStreamingBlock(blocks, messageId);
}

function pushToolStart(blocks: AnyBlock[], callId: string, name: string, args?: unknown): void {
  closeStreamingBlock(blocks);
  blocks.push({
    type: 'tool',
    call_id: callId,
    name,
    args,
    result: '',
    active: true,
    is_error: false,
    startTime: Date.now(),
  });
}

function applyToolUpdate(blocks: AnyBlock[], callId: string, partialResult: unknown): void {
  const block = findToolBlock(blocks, callId);
  if (block && block.type === 'tool') {
    block.result += typeof partialResult === 'string' ? partialResult : JSON.stringify(partialResult);
  }
}

function applyToolEnd(blocks: AnyBlock[], callId: string, result: unknown, isError: boolean): void {
  const block = findToolBlock(blocks, callId);
  if (block && block.type === 'tool') {
    block.active = false;
    block.is_error = isError;
    block.endTime = Date.now();
    block.result = truncateResult(stringifyResult(result));
  }
}

function pushApproval(
  blocks: AnyBlock[],
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  closeStreamingBlock(blocks);
  blocks.push({
    type: 'approval',
    prompt_id: promptId,
    tool_name: toolName,
    tool_input: toolInput,
    danger_level: dangerLevel,
    explanation,
    status: 'pending',
  });
}

// ── Turn lookup ──────────────────────────────────────────────────────

export function getActiveTurn(state: ChatState): ChatEntry | undefined {
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

// ── Main agent event handlers ────────────────────────────────────────

export function handleTurnStart(state: ChatState, turnIndex: number, turnId?: string): void {
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
    last.turnIndex = turnIndex;
    if (turnId) {
      last.turnId = turnId;
      if (!last.turnIds) last.turnIds = [];
      if (!last.turnIds.includes(turnId)) last.turnIds.push(turnId);
    }
  } else {
    // Close any previous open turns since we are starting a new turn block
    for (const entry of state.entries) {
      if (entry.type === 'turn' && !entry.endTime) {
        entry.endTime = Date.now();
        stopDanglingSubagents(state, entry);
      }
    }
    state.entries.push({
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

export function handleCacheInfo(state: ChatState, hitRate: number): void {
  let turn = getActiveTurn(state);
  if (!turn) {
    for (let i = state.entries.length - 1; i >= 0; i--) {
      if (state.entries[i].type === 'turn') {
        turn = state.entries[i];
        break;
      }
    }
  }
  if (turn && turn.type === 'turn') {
    if (hitRate === -1.0) {
      turn.cacheHitRate = undefined;
    } else {
      turn.cacheHitRate = hitRate;
    }
  }
}

export function handleMessageStart(state: ChatState, messageId: string | undefined): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushMessageStart(turn.blocks, messageId);
  }
}

export function handleMessageUpdate(state: ChatState, messageId: string | undefined, delta: DeltaPayload): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyMessageUpdate(state, turn.blocks, messageId ?? '_nomsg', messageId, delta);
  }
}

export function handleMessageEnd(state: ChatState, messageId?: string): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyMessageEnd(state, turn.blocks, messageId);
  }
}

export function handleToolStart(
  state: ChatState,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushToolStart(turn.blocks, toolCallId, toolName, args);
  }
}

export function handleToolUpdate(state: ChatState, toolCallId: string, partialResult: unknown): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyToolUpdate(turn.blocks, toolCallId, partialResult);
  }
}

export function handleToolEnd(
  state: ChatState,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyToolEnd(turn.blocks, toolCallId, result, isError);
  }
}

export function handleApprovalRequired(
  state: ChatState,
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushApproval(turn.blocks, promptId, toolName, toolInput, dangerLevel, explanation);
  }
}

export function handleAgentEnd(state: ChatState): void {
  state.isProcessing = false;
  for (const entry of state.entries) {
    if (entry.type === 'turn' && !entry.endTime) {
      entry.endTime = Date.now();
      stopDanglingSubagents(state, entry);
    }
  }
}

export function handleError(state: ChatState, errorText: string): void {
  state.isProcessing = false;
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    stopDanglingSubagents(state, turn);
    const lastBlock = turn.blocks[turn.blocks.length - 1];
    if (lastBlock && lastBlock.type === 'error') {
      lastBlock.text = errorText;
      return;
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

export function handleTurnEnded(state: ChatState): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    for (const b of turn.blocks) {
      if (b.type === 'tool' && b.active) {
        b.active = false;
        if (!b.endTime) b.endTime = Date.now();
        if (b.result === undefined) b.result = '';
      }
    }
  }
}

// ── Subagent event handlers ──────────────────────────────────────────

export function handleSubagentStart(
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

export function handleSubagentMessageStart(
  state: ChatState,
  subagentId: string,
  messageId: string | undefined
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    if (!sa.blocks) sa.blocks = [];
    pushMessageStart(sa.blocks, messageId);
  }
}

export function handleSubagentMessageUpdate(
  state: ChatState,
  subagentId: string,
  messageId: string | undefined,
  delta: DeltaPayload
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    applyMessageUpdate(state, sa.blocks, messageId ?? `_sa_${subagentId}`, messageId, delta);
  }
}

export function handleSubagentMessageEnd(state: ChatState, subagentId: string, messageId?: string): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    applyMessageEnd(state, sa.blocks, messageId);
  }
}

export function handleSubagentToolStart(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    pushToolStart(sa.blocks, toolCallId, toolName, args);
  }
}

export function handleSubagentToolUpdate(state: ChatState, subagentId: string, toolCallId: string, partialResult: unknown): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    applyToolUpdate(sa.blocks, toolCallId, partialResult);
  }
}

export function handleSubagentToolEnd(
  state: ChatState,
  subagentId: string,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const sa = state.subagents[subagentId];
  if (sa) {
    applyToolEnd(sa.blocks, toolCallId, result, isError);
  }
}

export function handleSubagentApprovalRequired(
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
    pushApproval(sa.blocks, promptId, toolName, toolInput, dangerLevel, explanation);
  }
}

export function handleSubagentEnd(
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
    finalizeAllBlocks(sa.blocks);
  }
}

// ── Shared utilities ─────────────────────────────────────────────────

export function stopDanglingSubagents(state: ChatState, turn: ChatEntry): void {
  const ids = turn.subagentIds;
  if (!ids) return;
  for (const id of ids) {
    const sa = state.subagents[id];
    if (sa && sa.status === 'working') {
      sa.status = 'error';
      sa.endTime = Date.now();
      finalizeAllBlocks(sa.blocks);
    }
  }
}

export function resolveApprovalBlock(state: ChatState, promptId: string, choice?: string): void {
  if (!promptId) return;
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

// ── Core event processing ────────────────────────────────────────────

export function processSingleEvent(state: ChatState, payload: string | Record<string, unknown>): void {
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
  if (!ev.run_id) return;

  // Track runId to sessionId mapping
  if (ev.event === 'run_created') {
    if (ev.session_id) {
      if (!state.runIdToSessionId) {
        state.runIdToSessionId = {};
      }
      state.runIdToSessionId[ev.run_id] = ev.session_id;
      state.runIdBySession[ev.session_id] = ev.run_id;
      if (ev.session_id === state.activeSessionId) {
        state.runId = ev.run_id;
      }
    }
  }

  const targetSessionId = ev.session_id || (state.runIdToSessionId && state.runIdToSessionId[ev.run_id]);
  if (!targetSessionId) {
    if (ev.run_id !== state.runId) {
      return;
    }
  }

  const isBackground = targetSessionId && targetSessionId !== state.activeSessionId;

  // Temporarily swap references if it's a background session event
  const originalEntries = state.entries;
  const originalIsProcessing = state.isProcessing;
  const originalSubagents = state.subagents;
  const originalRunId = state.runId;
  const originalTodo = state.todo;
  const originalSteerQueue = state.steerQueue;

  if (isBackground) {
    state.entries = state.entriesBySession[targetSessionId] || [];
    state.isProcessing = state.processingBySession[targetSessionId] ?? false;
    state.subagents = state.subagentsBySession[targetSessionId] ?? {};
    state.runId = state.runIdBySession[targetSessionId] ?? null;
    state.todo = state.todoBySession?.[targetSessionId] || [];
    state.steerQueue = state.steerQueueBySession?.[targetSessionId] || [];
  }

  try {
    // Set _pendingTurnId BEFORE lifecycle handlers that may call handleError/getActiveTurn
    state._pendingTurnId = ev.turn_id;

    // Seq gap detection
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

    // Run lifecycle events
    if (ev.event === 'state_changed' && ev.to) {
      state.runState = ev.to as RunState;
      if (ev.to === 'completed' || ev.to === 'cancelled' || ev.to === 'failed') {
        handleAgentEnd(state);
      }
    } else if (ev.event === 'run_started') {
      state.runState = 'running';
      if (!isBackground) {
        state.todo = [];
      }
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

    // Event-specific dispatch
    switch (ev.event) {
      case 'turn_started':
        handleTurnStart(state, ev.index ?? 0, ev.turn_id);
        break;
      case 'cache_info':
        handleCacheInfo(state, ev.hit_rate ?? -1.0);
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
      case 'todo_updated':
        state.todo = (ev.items as TodoItem[]) ?? [];
        break;
      case 'steer_queued': {
        const steerId = ev.steer_id ?? '';
        const msg = typeof ev.message === 'string' ? ev.message : '';
        // Only add to steerQueue if not already present (dedup — the frontend
        // may have already added it optimistically via steerMessageQueued).
        if (!state.steerQueue.some((s) => s.steerId === steerId)) {
          state.steerQueue.push({
            steerId,
            text: msg,
            status: 'pending',
            timestamp: Date.now(),
          });
        }
        break;
      }
      case 'steer_injected': {
        const steerId = ev.steer_id ?? '';
        const sq = state.steerQueue.find((s) => s.steerId === steerId);
        if (sq) sq.status = 'injected';
        for (const entry of state.entries) {
          if (entry.type === 'user' && entry.isSteer && entry.steerId === steerId) {
            entry.steerStatus = 'injected';
          }
        }
        // Close any previous open turns since steer message is now injected and a new turn is starting
        for (const entry of state.entries) {
          if (entry.type === 'turn' && !entry.endTime) {
            entry.endTime = Date.now();
            stopDanglingSubagents(state, entry);
          }
        }
        break;
      }
      case 'steer_cancelled':
      case 'steer_failed': {
        const steerId = ev.steer_id ?? '';
        state.steerQueue = state.steerQueue.filter((s) => s.steerId !== steerId);
        // Remove the steer entry from chat history (it was never injected)
        state.entries = state.entries.filter(
          (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
        );
        break;
      }
      default:
        break;
    }
  } finally {
    if (isBackground) {
      state.entriesBySession[targetSessionId] = state.entries;
      state.processingBySession[targetSessionId] = state.isProcessing;
      state.subagentsBySession[targetSessionId] = state.subagents;
      state.runIdBySession[targetSessionId] = state.runId;
      if (!state.todoBySession) {
        state.todoBySession = {};
      }
      state.todoBySession[targetSessionId] = state.todo;
      if (!state.steerQueueBySession) {
        state.steerQueueBySession = {};
      }
      state.steerQueueBySession[targetSessionId] = state.steerQueue;

      state.entries = originalEntries;
      state.isProcessing = originalIsProcessing;
      state.subagents = originalSubagents;
      state.runId = originalRunId;
      state.todo = originalTodo;
      state.steerQueue = originalSteerQueue;
    }
  }

  if (!isBackground && state.activeSessionId) {
    state.entriesBySession[state.activeSessionId] = state.entries;
    state.processingBySession[state.activeSessionId] = state.isProcessing;
    state.subagentsBySession[state.activeSessionId] = state.subagents;
    state.runIdBySession[state.activeSessionId] = state.runId;
    if (!state.todoBySession) {
      state.todoBySession = {};
    }
    state.todoBySession[state.activeSessionId] = state.todo;
    if (!state.steerQueueBySession) {
      state.steerQueueBySession = {};
    }
    state.steerQueueBySession[state.activeSessionId] = state.steerQueue;
  }
}
