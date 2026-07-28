import type {
  ChatState,
  ChatEntry,
  SubagentEntry,
  DeltaPayload,
  RunEventPayload,
  RunState,
  TodoItem,
  ParkedPlan,
  PlanDetail,
  ClarificationQuestion,
  ClarificationAnswers,
  NoticeBlock,
} from './types';
import {
  closeStreamingBlock,
  closeStreamingThinking,
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

/** Drop in-flight recovery banners once the model stream resumes (or hard-fails). */
export function clearRecoverableNotices(blocks: AnyBlock[]): void {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const b = blocks[i];
    if (b.type !== 'notice') continue;
    if (
      b.recoverable ||
      b.code === 'model_retry' ||
      b.code === 'model_stream_retry'
    ) {
      blocks.splice(i, 1);
    }
  }
}

function pushMessageStart(blocks: AnyBlock[], messageId: string | undefined): void {
  // Retry succeeded — stream is back; don't leave the retry banner hanging.
  clearRecoverableNotices(blocks);
  blocks.push({
    type: 'thinking',
    text: '',
    isStreaming: true,
    message_id: messageId,
    startTime: Date.now(),
  });
}

function applyMessageUpdate(
  thinkBuffers: Record<string, string>,
  blocks: AnyBlock[],
  thinkKey: string,
  messageId: string | undefined,
  delta: DeltaPayload
): void {
  // Runtime stream retries resume via model_streaming (no message_start),
  // so clear the hanging retry banner as soon as tokens arrive again.
  clearRecoverableNotices(blocks);
  const processed = processThinkBuffer(thinkBuffers, thinkKey, delta);
  appendDeltaToBlocks(blocks, processed, messageId);
}

function applyMessageEnd(
  thinkBuffers: Record<string, string>,
  blocks: AnyBlock[],
  messageId?: string
): void {
  if (messageId) delete thinkBuffers[messageId];
  closeStreamingBlock(blocks, messageId);
}

function pushToolStart(blocks: AnyBlock[], callId: string, name: string, args?: unknown): void {
  // Stream may resume with a tool call instead of text after a retry.
  clearRecoverableNotices(blocks);
  // Upgrade an existing preparing placeholder when possible.
  const preparingIdx = blocks.findIndex((b) => {
    if (b.type !== 'tool' || b.phase !== 'preparing') return false;
    if (b.call_id && b.call_id === callId) return true;
    return false;
  });
  const byNameIdx =
    preparingIdx >= 0
      ? preparingIdx
      : blocks.findIndex((b) => b.type === 'tool' && b.phase === 'preparing' && b.name === name);

  if (byNameIdx >= 0) {
    const block = blocks[byNameIdx];
    if (block.type === 'tool') {
      block.call_id = callId;
      block.name = name;
      block.args = args;
      block.result = '';
      block.active = true;
      block.is_error = false;
      block.phase = 'running';
      block.startTime = Date.now();
      delete block.hint_path;
      return;
    }
  }

  closeStreamingThinking(blocks);
  blocks.push({
    type: 'tool',
    call_id: callId,
    name,
    args,
    result: '',
    active: true,
    is_error: false,
    startTime: Date.now(),
    phase: 'running',
  });
}

function upsertToolPreparing(
  blocks: AnyBlock[],
  streamIndex: number,
  callId?: string,
  name?: string,
  hintPath?: string
): void {
  const existingIdx = blocks.findIndex((b) => {
    if (b.type !== 'tool' || b.phase !== 'preparing') return false;
    if (b.stream_index === streamIndex) return true;
    if (callId && b.call_id === callId) return true;
    return false;
  });

  if (existingIdx >= 0) {
    const block = blocks[existingIdx];
    if (block.type === 'tool') {
      if (callId) block.call_id = callId;
      if (name) block.name = name;
      if (hintPath) block.hint_path = hintPath;
      block.stream_index = streamIndex;
    }
    return;
  }

  closeStreamingThinking(blocks);
  blocks.push({
    type: 'tool',
    call_id: callId || `preparing_${streamIndex}`,
    name: name || 'tool',
    args: hintPath ? { path: hintPath } : undefined,
    result: '',
    active: true,
    is_error: false,
    startTime: Date.now(),
    phase: 'preparing',
    stream_index: streamIndex,
    hint_path: hintPath,
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

function pushClarification(
  blocks: AnyBlock[],
  promptId: string,
  title: string | undefined,
  questions: ClarificationQuestion[]
): void {
  closeStreamingBlock(blocks);
  blocks.push({
    type: 'clarification',
    prompt_id: promptId,
    title,
    questions,
    status: 'pending',
  });
}

// ── Turn lookup ──────────────────────────────────────────────────────

/** Canonical prompts-table id for the current live turn (never runId). */
function currentPromptId(state: ChatState, sessionId: string): string | undefined {
  const entries = state.entries[sessionId];
  if (!entries) return undefined;
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if ((entry.type === 'user' || entry.type === 'turn') && entry.promptId) {
      return entry.promptId;
    }
  }
  return undefined;
}

export function getActiveTurn(state: ChatState, sessionId: string): ChatEntry | undefined {
  const entries = state.entries[sessionId];
  if (!entries) return undefined;
  const pendingTurnId = state._pendingTurnId?.[sessionId];
  if (pendingTurnId) {
    const byId = entries.find(
      (e) => e.type === 'turn' && (e.turnId === pendingTurnId || e.turnIds?.includes(pendingTurnId))
    );
    if (byId && byId.type === 'turn') return byId;
  }
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry.type === 'turn' && !entry.endTime) {
      return entry;
    }
  }
  return undefined;
}

function getOrCreateSubagent(
  subagents: Record<string, SubagentEntry>,
  subagentId: string,
  roleName: string,
  task: string
): SubagentEntry {
  if (!subagents[subagentId]) {
    subagents[subagentId] = {
      id: subagentId,
      role_name: roleName,
      task,
      status: 'working',
      blocks: [],
      startTime: Date.now(),
    };
  }
  return subagents[subagentId];
}

// ── Main agent event handlers ────────────────────────────────────────

/** Find the open turn even when a pending steer card was pushed after it. */
function findOpenTurn(entries: ChatEntry[]): ChatEntry | undefined {
  for (let i = entries.length - 1; i >= 0; i--) {
    const entry = entries[i];
    if (entry.type === 'turn' && !entry.endTime) return entry;
    // Skip trailing steer cards — they must not split the pre-inject Worked block.
    if (entry.type === 'user' && entry.isSteer) continue;
    break;
  }
  return undefined;
}

export function handleTurnStart(state: ChatState, sessionId: string, turnIndex: number, turnId?: string): void {
  const entries = state.entries[sessionId];
  if (turnId) {
    const existing = entries.find(
      (e) => e.type === 'turn' && (e.turnId === turnId || e.turnIds?.includes(turnId))
    );
    if (existing && existing.type === 'turn') {
      existing.turnIndex = turnIndex;
      return;
    }
  }
  const openTurn = findOpenTurn(entries);
  if (openTurn && openTurn.type === 'turn') {
    openTurn.turnIndex = turnIndex;
    if (turnId) {
      openTurn.turnId = turnId;
      if (!openTurn.turnIds) openTurn.turnIds = [];
      if (!openTurn.turnIds.includes(turnId)) openTurn.turnIds.push(turnId);
    }
  } else {
    // Close any previous open turns since we are starting a new turn block
    for (const entry of entries) {
      if (entry.type === 'turn' && !entry.endTime) {
        entry.endTime = Date.now();
        stopDanglingSubagents(state.subagents[sessionId], entry);
      }
    }
    entries.push({
      id: turnId ? `turn-${turnId}` : `turn-${turnIndex}-${Date.now()}`,
      type: 'turn',
      promptId: currentPromptId(state, sessionId),
      turnId,
      turnIds: turnId ? [turnId] : [],
      turnIndex,
      blocks: [],
      startTime: Date.now(),
    });
  }
}

export function handleCacheInfo(state: ChatState, sessionId: string, hitRate: number): void {
  let turn = getActiveTurn(state, sessionId);
  if (!turn) {
    for (let i = state.entries[sessionId].length - 1; i >= 0; i--) {
      if (state.entries[sessionId][i].type === 'turn') {
        turn = state.entries[sessionId][i];
        break;
      }
    }
  }
  if (turn && turn.type === 'turn') {
    // Any negative hit_rate is a sentinel (not a real rate):
    //   -1.0 → prefix drifted   -2.0 → cache expired from idle
    // Clear the display so the frontend doesn't show garbage.
    if (hitRate < 0) {
      turn.cacheHitRate = undefined;
    } else {
      turn.cacheHitRate = hitRate;
    }
  }
}

export function handleCacheSummary(
  state: ChatState,
  runId: string,
  metrics: { total_turns: number; total_hit_tokens: number; total_miss_tokens: number; turns_with_hits: number; cumulative_hit_rate: number }
): void {
  state.cacheMetricsByRun[runId] = {
    total_turns: metrics.total_turns,
    total_hit_tokens: metrics.total_hit_tokens,
    total_miss_tokens: metrics.total_miss_tokens,
    turns_with_hits: metrics.turns_with_hits,
    cumulative_hit_rate: metrics.cumulative_hit_rate,
  };
}

export function handleMessageStart(state: ChatState, sessionId: string, messageId: string | undefined): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushMessageStart(turn.blocks, messageId);
  }
}

/** Stream HTTP/SSE is up again — clear retry banners before any tokens arrive. */
export function handleModelCallStarted(state: ChatState, sessionId: string): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    clearRecoverableNotices(turn.blocks);
  }
}

export function handleMessageUpdate(state: ChatState, sessionId: string, messageId: string | undefined, delta: DeltaPayload): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyMessageUpdate(state._thinkBuffers[sessionId], turn.blocks, messageId ?? '_nomsg', messageId, delta);
  }
}

export function handleMessageEnd(state: ChatState, sessionId: string, messageId?: string): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyMessageEnd(state._thinkBuffers[sessionId], turn.blocks, messageId);
  }
}

export function handleToolPreparing(
  state: ChatState,
  sessionId: string,
  streamIndex: number,
  callId?: string,
  name?: string,
  hintPath?: string
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    upsertToolPreparing(turn.blocks, streamIndex, callId, name, hintPath);
  }
}

export function handleToolStart(
  state: ChatState,
  sessionId: string,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushToolStart(turn.blocks, toolCallId, toolName, args);
  }
}

export function handleToolUpdate(state: ChatState, sessionId: string, toolCallId: string, partialResult: unknown): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyToolUpdate(turn.blocks, toolCallId, partialResult);
  }
}

export function handleToolEnd(
  state: ChatState,
  sessionId: string,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    applyToolEnd(turn.blocks, toolCallId, result, isError);
  }
}

export function handleApprovalRequired(
  state: ChatState,
  sessionId: string,
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushApproval(turn.blocks, promptId, toolName, toolInput, dangerLevel, explanation);
  }
}

export function handleInputRequested(
  state: ChatState,
  sessionId: string,
  promptId: string,
  title: string | undefined,
  questions: ClarificationQuestion[]
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    pushClarification(turn.blocks, promptId, title, questions);
  }
}

export function resolveClarificationBlock(
  state: ChatState,
  sessionId: string,
  promptId: string,
  answers?: ClarificationAnswers
): void {
  if (!promptId) return;
  for (const entry of state.entries[sessionId]) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    for (const b of entry.blocks) {
      if (b.type === 'clarification' && b.prompt_id === promptId) {
        b.status = 'answered';
        if (answers) b.answers = answers;
        return;
      }
    }
  }
}

export function handleAgentEnd(state: ChatState, sessionId: string): void {
  state.processing[sessionId] = false;
  state.steerQueue[sessionId] = [];
  state.entries[sessionId] = state.entries[sessionId].filter(
    (e) => !(e.type === 'user' && e.isSteer && e.steerStatus === 'pending')
  );
  for (const entry of state.entries[sessionId]) {
    if (entry.type !== 'turn') continue;
    if (!entry.endTime) {
      entry.endTime = Date.now();
      stopDanglingSubagents(state.subagents[sessionId], entry);
    }
    // Always clear retry banners on run end — abort may have already stamped
    // endTime before run_cancelled arrives.
    if (entry.blocks) clearRecoverableNotices(entry.blocks);
  }
}

export function handleError(state: ChatState, sessionId: string, errorText: string): void {
  state.processing[sessionId] = false;
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    clearRecoverableNotices(turn.blocks);
    stopDanglingSubagents(state.subagents[sessionId], turn);
    const lastBlock = turn.blocks[turn.blocks.length - 1];
    if (lastBlock && lastBlock.type === 'error') {
      lastBlock.text = errorText;
      return;
    }
    turn.blocks.push({ type: 'error', text: errorText });
  } else {
    state.entries[sessionId].push({
      id: `error-${Date.now()}`,
      type: 'turn',
      promptId: currentPromptId(state, sessionId),
      turnIndex: 0,
      blocks: [{ type: 'error', text: errorText }],
      startTime: Date.now(),
      endTime: Date.now(),
    });
  }
}

export function handleNotice(
  state: ChatState,
  sessionId: string,
  message: string,
  code?: string,
  severity?: string,
  recoverable?: boolean,
  details?: Pick<NoticeBlock, 'strategy' | 'tokens_before' | 'tokens_after'>,
): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);

    // Nested retry layers (SSE restart vs recovery engine) emit different
    // codes for the same user-facing situation — keep one connection banner.
    const isConnectionRetry =
      code === 'model_retry' ||
      code === 'model_stream_retry' ||
      /Failed to connect to remote model/i.test(message);

    if (isConnectionRetry) {
      for (let i = turn.blocks.length - 1; i >= 0; i--) {
        const block = turn.blocks[i];
        if (block.type !== 'notice') continue;
        if (
          block.code === 'model_retry' ||
          block.code === 'model_stream_retry' ||
          /Failed to connect to remote model/i.test(block.text)
        ) {
          turn.blocks[i] = {
            type: 'notice',
            text: message,
            code: 'model_retry',
            severity,
            recoverable: true,
          };
          return;
        }
      }
      turn.blocks.push({
        type: 'notice',
        text: message,
        code: 'model_retry',
        severity,
        recoverable: true,
      });
      return;
    }

    // Other notices (compact / fallback / …): replace same code only.
    if (code) {
      for (let i = turn.blocks.length - 1; i >= 0; i--) {
        const block = turn.blocks[i];
        if (block.type === 'notice' && block.code === code) {
          turn.blocks[i] = { type: 'notice', text: message, code, severity, recoverable, ...details };
          return;
        }
      }
    }
    turn.blocks.push({ type: 'notice', text: message, code, severity, recoverable, ...details });
  }
}

export function handleTurnEnded(state: ChatState, sessionId: string): void {
  const turn = getActiveTurn(state, sessionId);
  if (turn && turn.type === 'turn' && turn.blocks) {
    closeStreamingBlock(turn.blocks);
    clearRecoverableNotices(turn.blocks);
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
  sessionId: string,
  subagentId: string,
  parentCallId: string | undefined,
  roleName: string | undefined,
  task: string | unknown
): void {
  const safeTask = typeof task === 'string' ? task : JSON.stringify(task);
  const safeRoleName = typeof roleName === 'string' ? roleName : String(subagentId);
  const turn = getActiveTurn(state, sessionId);
  if (turn) {
    getOrCreateSubagent(state.subagents[sessionId], subagentId, safeRoleName, safeTask);
    if (!turn.subagentIds) turn.subagentIds = [];
    if (!turn.subagentIds.includes(subagentId)) turn.subagentIds.push(subagentId);
    if (turn.blocks) {
      turn.blocks.push({ type: 'subagent_ref', subagent_id: subagentId, parent_call_id: parentCallId });
    }
  }
}

export function handleSubagentMessageStart(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  messageId: string | undefined
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    if (!sa.blocks) sa.blocks = [];
    pushMessageStart(sa.blocks, messageId);
  }
}

export function handleSubagentMessageUpdate(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  messageId: string | undefined,
  delta: DeltaPayload
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    applyMessageUpdate(state._thinkBuffers[sessionId], sa.blocks, messageId ?? `_sa_${subagentId}`, messageId, delta);
  }
}

export function handleSubagentMessageEnd(state: ChatState, sessionId: string, subagentId: string, messageId?: string): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    applyMessageEnd(state._thinkBuffers[sessionId], sa.blocks, messageId);
  }
}

export function handleSubagentToolStart(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  toolCallId: string,
  toolName: string,
  args?: unknown
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    pushToolStart(sa.blocks, toolCallId, toolName, args);
  }
}

export function handleSubagentToolUpdate(state: ChatState, sessionId: string, subagentId: string, toolCallId: string, partialResult: unknown): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    applyToolUpdate(sa.blocks, toolCallId, partialResult);
  }
}

export function handleSubagentToolEnd(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  toolCallId: string,
  result: unknown,
  isError: boolean
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    applyToolEnd(sa.blocks, toolCallId, result, isError);
  }
}

export function handleSubagentApprovalRequired(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  promptId: string,
  toolName: string,
  toolInput: unknown,
  dangerLevel: string,
  explanation: string
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    pushApproval(sa.blocks, promptId, toolName, toolInput, dangerLevel, explanation);
  }
}

export function handleSubagentEnd(
  state: ChatState,
  sessionId: string,
  subagentId: string,
  success: boolean,
  iterationsUsed?: number
): void {
  const sa = state.subagents[sessionId][subagentId];
  if (sa) {
    sa.status = success ? 'done' : 'error';
    sa.iterations_used = iterationsUsed;
    sa.endTime = Date.now();
    finalizeAllBlocks(sa.blocks);
  }
}

// ── Shared utilities ─────────────────────────────────────────────────

export function stopDanglingSubagents(subagents: Record<string, SubagentEntry>, turn: ChatEntry): void {
  const ids = turn.subagentIds;
  if (!ids) return;
  for (const id of ids) {
    const sa = subagents[id];
    if (sa && sa.status === 'working') {
      sa.status = 'error';
      sa.endTime = Date.now();
      finalizeAllBlocks(sa.blocks);
    }
  }
}

export function resolveApprovalBlock(state: ChatState, sessionId: string, promptId: string, choice?: string): void {
  if (!promptId) return;
  const choiceStr = typeof choice === 'string' ? choice : '';
  const approved = !choiceStr.toLowerCase().includes('deny');
  for (const entry of state.entries[sessionId]) {
    if (entry.type !== 'turn' || !entry.blocks) continue;
    for (const b of entry.blocks) {
      if (b.type === 'approval' && b.prompt_id === promptId) {
        b.status = approved ? 'approved' : 'denied';
        return;
      }
    }
  }
  for (const sa of Object.values(state.subagents[sessionId] ?? {})) {
    for (const b of sa.blocks) {
      if (b.type === 'approval' && b.prompt_id === promptId) {
        b.status = approved ? 'approved' : 'denied';
        return;
      }
    }
  }
}

// ── Core event processing ────────────────────────────────────────────

export function processSingleEvent(state: ChatState, payload: string | Record<string, unknown>): string | null {
  let raw: Record<string, unknown>;
  if (typeof payload === 'string') {
    try {
      raw = JSON.parse(payload);
    } catch {
      return null;
    }
  } else {
    raw = payload as Record<string, unknown>;
  }

  if (!raw || typeof raw.event !== 'string') return null;
  const ev = raw as unknown as RunEventPayload;
  if (!ev.run_id) return null;

  // Track runId to sessionId mapping
  if (ev.event === 'run_created') {
    if (ev.session_id) {
      (state.runIdToSessionId ??= {})[ev.run_id] = ev.session_id;
      state.runId[ev.session_id] = ev.run_id;
      (state.lastRunId ??= {})[ev.session_id] = ev.run_id;
    }
  }

  // Resolve sessionId. Most runtime events are Envelope-scoped by run_id only;
  // the run_created bootstrap event can be missed because the frontend subscribes
  // after create_run returns, so keep a client-side runId -> sessionId map too.
  const sessionId = ev.session_id ?? state.runIdToSessionId?.[ev.run_id] ?? null;
  if (!sessionId) {
    return null;
  }

  // Ensure per-session maps have defaults for this sessionId
  (state.entries ??= {})[sessionId] ??= [];
  (state.subagents ??= {})[sessionId] ??= {};
  (state._thinkBuffers ??= {})[sessionId] ??= {};
  (state.todo ??= {})[sessionId] ??= [];
  (state.parkedPlans ??= {})[sessionId] ??= [];
  (state.plans ??= {})[sessionId] ??= [];
  (state.activePlanId ??= {})[sessionId] ??= null;
  (state.activePlanTitle ??= {})[sessionId] ??= null;
  (state.steerQueue ??= {})[sessionId] ??= [];
  (state.processing ??= {})[sessionId] ??= false;
  (state.runId ??= {})[sessionId] ??= null;
  (state.lastRunId ??= {})[sessionId] ??= ev.run_id;
  (state.runState ??= {})[sessionId] ??= null;
  (state.contextUsageRevision ??= {})[sessionId] ??= 0;
  (state.goal ??= {})[sessionId] ??= null;
  (state.goalCompleted ??= {})[sessionId] ??= false;
  (state.viewingSubagentPath ??= {})[sessionId] ??= [];
  (state.btwEntries ??= {})[sessionId] ??= [];
  (state.isResuming ??= {})[sessionId] ??= false;
  (state._pendingTurnId ??= {})[sessionId] ??= undefined;

  (state.appliedEventIdsByRun ??= {})[ev.run_id] ??= {};
  (state.pendingEventsByRun ??= {})[ev.run_id] ??= {};
  const trackedRuns = Object.keys(state.appliedEventIdsByRun);
  if (trackedRuns.length > 128) {
    for (const oldRunId of trackedRuns.slice(0, trackedRuns.length - 128)) {
      delete state.appliedEventIdsByRun[oldRunId];
      delete state.pendingEventsByRun[oldRunId];
      delete state.pendingGapByRun[oldRunId];
      delete state.resyncingByRun[oldRunId];
      delete state.cacheMetricsByRun[oldRunId];
      delete state.lastSeqByRun[oldRunId];
      delete state.runIdToSessionId[oldRunId];
    }
  }

  // Events are folded exactly once and only in contiguous sequence order.
  // Live events beyond a gap wait in the per-run reorder buffer until replay
  // supplies the missing sequence.
  if (ev.event_id && state.appliedEventIdsByRun[ev.run_id][ev.event_id]) {
    return null;
  }
  if (typeof ev.seq === 'number') {
    const prev = state.lastSeqByRun[ev.run_id];
    if (prev !== undefined && ev.seq <= prev) {
      return null;
    }
    if (prev !== undefined && ev.seq > prev + 1) {
      state.pendingEventsByRun[ev.run_id][ev.seq] = ev;
      const pendingSeqs = Object.keys(state.pendingEventsByRun[ev.run_id])
        .map(Number)
        .sort((a, b) => a - b);
      while (pendingSeqs.length > 512) {
        const seq = pendingSeqs.pop();
        if (seq !== undefined) delete state.pendingEventsByRun[ev.run_id][seq];
      }
      state.pendingGapByRun[ev.run_id] = { fromSeq: prev, toSeq: ev.seq };
      return null;
    }
    state.lastSeqByRun[ev.run_id] = ev.seq;
  }
  if (ev.event_id) {
    state.appliedEventIdsByRun[ev.run_id][ev.event_id] = true;
    const appliedIds = Object.keys(state.appliedEventIdsByRun[ev.run_id]);
    if (appliedIds.length > 4096) {
      for (const oldId of appliedIds.slice(0, appliedIds.length - 4096)) {
        delete state.appliedEventIdsByRun[ev.run_id][oldId];
      }
    }
  }

  // Set _pendingTurnId BEFORE lifecycle handlers that may call handleError/getActiveTurn
  state._pendingTurnId[sessionId] = ev.turn_id;

  // Run lifecycle events
  if (ev.event === 'state_changed' && ev.to) {
    state.runState[sessionId] = ev.to as RunState;
    if (ev.to === 'completed' || ev.to === 'cancelled' || ev.to === 'failed') {
      handleAgentEnd(state, sessionId);
    }
  } else if (ev.event === 'run_started') {
    state.runState[sessionId] = 'running';
    // Do not clear durable plans — they survive across runs / restarts.
  } else if (ev.event === 'run_paused') {
    state.runState[sessionId] = 'paused';
  } else if (ev.event === 'run_resumed') {
    state.runState[sessionId] = 'running';
  } else if (ev.event === 'run_completed' || ev.event === 'run_cancelled') {
    state.processing[sessionId] = false;
    state.runId[sessionId] = null;
    handleAgentEnd(state, sessionId);
  } else if (ev.event === 'run_failed') {
    state.processing[sessionId] = false;
    state.runId[sessionId] = null;
    handleError(state, sessionId, ev.error ?? 'run failed');
    for (const entry of state.entries[sessionId]) {
      if (entry.type === 'turn' && !entry.endTime) {
        entry.endTime = Date.now();
        stopDanglingSubagents(state.subagents[sessionId], entry);
      }
    }
  }

  // Event-specific dispatch
  switch (ev.event) {
    case 'turn_started':
      handleTurnStart(state, sessionId, ev.index ?? 0, ev.turn_id);
      break;
    case 'cache_info':
      handleCacheInfo(state, sessionId, ev.hit_rate ?? -1.0);
      break;
    case 'cache_summary':
      handleCacheSummary(state, ev.run_id, {
        total_turns: ev.total_turns ?? 0,
        total_hit_tokens: ev.total_hit_tokens ?? 0,
        total_miss_tokens: ev.total_miss_tokens ?? 0,
        turns_with_hits: ev.turns_with_hits ?? 0,
        cumulative_hit_rate: ev.cumulative_hit_rate ?? 0,
      });
      break;
    case 'turn_ended':
      handleTurnEnded(state, sessionId);
      break;
    case 'model_call_started':
      handleModelCallStarted(state, sessionId);
      break;
    case 'message_start':
      if (ev.subagent_id) handleSubagentMessageStart(state, sessionId, ev.subagent_id, ev.message_id);
      else handleMessageStart(state, sessionId, ev.message_id);
      break;
    case 'message_update':
    case 'model_streaming':
      if (ev.subagent_id) handleSubagentMessageUpdate(state, sessionId, ev.subagent_id, ev.message_id, ev.delta ?? {});
      else handleMessageUpdate(state, sessionId, ev.message_id, ev.delta ?? {});
      break;
    case 'message_end':
      if (ev.subagent_id) handleSubagentMessageEnd(state, sessionId, ev.subagent_id, ev.message_id);
      else handleMessageEnd(state, sessionId, ev.message_id);
      break;
    case 'message_interrupted': {
      const turn = findOpenTurn(state.entries[sessionId]);
      if (turn?.type === 'turn') {
        const content =
          ev.partial_message && typeof ev.partial_message === 'object'
            ? ev.partial_message.content ?? ''
            : '';
        const thinkMatch = content.match(/<think>([\s\S]*?)<\/think>/);
        const thinking = thinkMatch?.[1] ?? '';
        const visible = content.replace(/<think>[\s\S]*?<\/think>/, '').trim();
        const existingThinking = turn.blocks?.find(
          (block) => block.type === 'thinking' && block.message_id === ev.message_id,
        );
        if (
          existingThinking?.type === 'thinking' &&
          thinking.length > existingThinking.text.length
        ) {
          existingThinking.text = thinking;
        } else if (!existingThinking && thinking) {
          turn.blocks?.push({
            type: 'thinking',
            text: thinking,
            isStreaming: false,
            message_id: ev.message_id,
            startTime: Date.now(),
            endTime: Date.now(),
          });
        }
        const existing = turn.blocks?.find(
          (block) => block.type === 'assistant' && block.message_id === ev.message_id,
        );
        if (existing?.type === 'assistant' && visible.length > existing.text.length) {
          existing.text = visible;
        } else if (!existing && visible) {
          turn.blocks?.push({
            type: 'assistant',
            text: visible,
            isStreaming: false,
            message_id: ev.message_id,
          });
        }
        turn.interrupted = true;
      }
      handleMessageEnd(state, sessionId, ev.message_id);
      break;
    }
    case 'tool_preparing':
      handleToolPreparing(
        state,
        sessionId,
        typeof ev.index === 'number' ? ev.index : 0,
        ev.call_id,
        ev.name,
        ev.hint_path
      );
      break;
    case 'tool_started':
      if (ev.subagent_id) handleSubagentToolStart(state, sessionId, ev.subagent_id, ev.call_id ?? '', ev.name ?? '', ev.args);
      else handleToolStart(state, sessionId, ev.call_id ?? '', ev.name ?? '', ev.args);
      break;
    case 'tool_update':
      if (ev.subagent_id) handleSubagentToolUpdate(state, sessionId, ev.subagent_id, ev.call_id ?? '', ev.partial ?? '');
      else handleToolUpdate(state, sessionId, ev.call_id ?? '', ev.partial ?? '');
      break;
    case 'tool_ended':
      if (ev.subagent_id) handleSubagentToolEnd(state, sessionId, ev.subagent_id, ev.call_id ?? '', ev.result ?? '', ev.is_error ?? false);
      else handleToolEnd(state, sessionId, ev.call_id ?? '', ev.result ?? '', ev.is_error ?? false);
      // skill_reload rescans Brain — drop Redux TTL so the selector refreshes.
      if (ev.name === 'skill_reload' && !ev.is_error) {
        state.skillsCache = null;
      }
      break;
    case 'approval_required':
      if (ev.subagent_id) handleSubagentApprovalRequired(state, sessionId, ev.subagent_id, ev.prompt_id ?? '', ev.tool_name ?? '', ev.tool_input, ev.danger_level ?? '', ev.explanation ?? '');
      else handleApprovalRequired(state, sessionId, ev.prompt_id ?? '', ev.tool_name ?? '', ev.tool_input, ev.danger_level ?? '', ev.explanation ?? '');
      break;
    case 'approval_resolved':
      resolveApprovalBlock(state, sessionId, ev.prompt_id ?? '', ev.choice);
      break;
    case 'input_requested': {
      const questions = (ev.questions as ClarificationQuestion[] | undefined) ?? [];
      handleInputRequested(state, sessionId, ev.prompt_id ?? '', ev.title, questions);
      break;
    }
    case 'input_resolved': {
      // Rust emits ClarificationAnswers { answers: { qId: [optIds] } }
      const raw = ev.answers as { answers?: ClarificationAnswers } | ClarificationAnswers | undefined;
      let normalized: ClarificationAnswers | undefined;
      if (raw && typeof raw === 'object') {
        if ('answers' in raw && raw.answers && typeof raw.answers === 'object' && !Array.isArray(raw.answers)) {
          normalized = raw.answers;
        } else {
          normalized = raw as ClarificationAnswers;
        }
      }
      resolveClarificationBlock(state, sessionId, ev.prompt_id ?? '', normalized);
      break;
    }
    case 'context_compacted':
      handleNotice(
        state,
        sessionId,
        ev.summary ?? '',
        'context_compacted',
        'info',
        false,
        {
          strategy: ev.strategy,
          tokens_before: ev.tokens_before,
          tokens_after: ev.tokens_after,
        },
      );
      state.contextUsageRevision[sessionId] += 1;
      state.lastRunId[sessionId] = ev.run_id;
      break;
    case 'error':
      handleError(state, sessionId, ((ev as unknown as Record<string, unknown>).message as string | undefined) ?? 'unknown error');
      break;
    case 'notice':
      handleNotice(
        state,
        sessionId,
        typeof ev.message === 'string' ? ev.message : ev.code ?? 'runtime notice',
        ev.code,
        ev.severity,
        ev.recoverable,
      );
      break;
    case 'subagent_started':
      handleSubagentStart(state, sessionId, ev.subagent_id ?? '', ev.parent_call_id, ev.role_name, ev.task ?? '');
      break;
    case 'subagent_ended':
      handleSubagentEnd(state, sessionId, ev.subagent_id ?? '', ev.success ?? false, ev.iterations_used);
      break;
    case 'todo_updated':
      state.todo[sessionId] = (ev.items as TodoItem[]) ?? [];
      state.parkedPlans[sessionId] = (ev.parked as ParkedPlan[]) ?? [];
      state.plans[sessionId] = (ev.plans as PlanDetail[]) ?? [];
      state.activePlanId[sessionId] = ev.active_plan_id ?? null;
      state.activePlanTitle[sessionId] = ev.active_plan_title ?? null;
      break;
    case 'goal_set':
      state.goal[sessionId] = ev.goal ?? null;
      state.goalCompleted[sessionId] = false;
      break;
    case 'goal_completed':
      state.goalCompleted[sessionId] = true;
      break;
    case 'goal_cleared':
      state.goal[sessionId] = null;
      state.goalCompleted[sessionId] = false;
      state.todo[sessionId] = [];
      state.parkedPlans[sessionId] = [];
      state.plans[sessionId] = [];
      state.activePlanId[sessionId] = null;
      state.activePlanTitle[sessionId] = null;
      break;
    case 'steer_queued': {
      const steerId = ev.steer_id ?? '';
      const msg = typeof ev.message === 'string' ? ev.message : '';
      if (!state.steerQueue[sessionId].some((s) => s.steerId === steerId)) {
        state.steerQueue[sessionId].push({
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
      const sq = state.steerQueue[sessionId].find((s) => s.steerId === steerId);
      if (sq) sq.status = 'injected';
      for (const entry of state.entries[sessionId]) {
        if (entry.type === 'user' && entry.isSteer && entry.steerId === steerId) {
          entry.steerStatus = 'injected';
        }
      }
      for (const entry of state.entries[sessionId]) {
        if (entry.type === 'turn' && !entry.endTime) {
          entry.endTime = Date.now();
          stopDanglingSubagents(state.subagents[sessionId], entry);
        }
      }
      break;
    }
    case 'steer_cancelled':
    case 'steer_failed': {
      const steerId = ev.steer_id ?? '';
      state.steerQueue[sessionId] = state.steerQueue[sessionId].filter((s) => s.steerId !== steerId);
      state.entries[sessionId] = state.entries[sessionId].filter(
        (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
      );
      break;
    }
    default:
      break;
  }

  const lastApplied = state.lastSeqByRun[ev.run_id];
  if (lastApplied !== undefined) {
    const next = state.pendingEventsByRun[ev.run_id]?.[lastApplied + 1];
    if (next) {
      delete state.pendingEventsByRun[ev.run_id][lastApplied + 1];
      processSingleEvent(state, next as unknown as Record<string, unknown>);
    }
    const gap = state.pendingGapByRun[ev.run_id];
    if (gap && (state.lastSeqByRun[ev.run_id] ?? -1) >= gap.toSeq) {
      delete state.pendingGapByRun[ev.run_id];
    }
  }
  return sessionId;
}
