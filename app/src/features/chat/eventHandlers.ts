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

export function handleMessageStart(state: ChatState, messageId: string | undefined): void {
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

export function handleMessageUpdate(state: ChatState, messageId: string | undefined, delta: DeltaPayload): void {
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    const processed = processThinkBuffer(state._thinkBuffers, messageId ?? '_nomsg', delta);
    appendDeltaToBlocks(turn.blocks, processed, messageId);
  }
}

export function handleMessageEnd(state: ChatState, messageId?: string): void {
  if (messageId) delete state._thinkBuffers[messageId];
  const turn = getActiveTurn(state);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks, messageId);
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

export function handleToolUpdate(state: ChatState, toolCallId: string, partialResult: unknown): void {
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

export function handleToolEnd(
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

export function handleAgentEnd(state: ChatState): void {
  state.isProcessing = false;
  state.todo = [];
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
    sa.blocks.push({
      type: 'thinking',
      text: '',
      isStreaming: true,
      message_id: messageId,
      startTime: Date.now()
    });
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
    const processed = processThinkBuffer(state._thinkBuffers, messageId ?? `_sa_${subagentId}`, delta);
    appendDeltaToBlocks(sa.blocks, processed, messageId);
  }
}

export function handleSubagentMessageEnd(state: ChatState, subagentId: string, messageId?: string): void {
  if (messageId) delete state._thinkBuffers[messageId];
  const sa = state.subagents[subagentId];
  if (sa) {
    closeStreamingBlock(sa.blocks, messageId);
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

export function handleSubagentToolUpdate(state: ChatState, subagentId: string, toolCallId: string, partialResult: unknown): void {
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

export function handleSubagentToolEnd(
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
    sa.blocks.forEach((b) => {
      if (b.isStreaming) {
        b.isStreaming = false;
        if (b.type === 'thinking') b.endTime = Date.now();
      }
    });
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
      sa.blocks.forEach((b) => {
        if (b.isStreaming) {
          b.isStreaming = false;
          if (b.type === 'thinking') b.endTime = Date.now();
        }
      });
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

// ── Core event processing (extracted for batch dispatch, PERF-1) ──────

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

  if (ev.event === 'run_created') {
    if (ev.session_id === state.activeSessionId) {
      state.runId = ev.run_id ?? null;
    }
  }

  if (ev.run_id !== state.runId) {
    return;
  }

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
    case 'todo_updated':
      state.todo = (ev.items as TodoItem[]) ?? [];
      break;
    default:
      break;
  }
}
