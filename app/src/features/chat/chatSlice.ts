import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resolveSkillScope, type SkillScopeProjectState } from './skillScope';
import { resumeSession, deleteSession } from '../project/projectSlice';

import type {
  TurnBlock, SubagentEntry, ChatState, RunState, ChatEntry, FrontendPrompt,
  TodoItem, ParkedPlan,
} from './types';
import { processSingleEvent, stopDanglingSubagents, clearRecoverableNotices } from './eventHandlers';

// ── Re-export public chat types and selectors ──────────────────────
export {
  selectEntryIds, selectEntryById, selectSubagentById,
  selectPendingApprovalCount, selectHasActivePendingApproval,
  selectActivePendingApproval, pendingApprovalEqual,
  selectHasActivePendingClarification, selectActivePendingClarification,
  pendingClarificationEqual,
  selectViewingSubagentPath, selectActiveBtwEntries,
  selectIsResumingActive,
} from './selectors';
export type {
  TodoItem, ParkedPlan, TurnBlock, SubagentBlock, SubagentEntry, ChatEntry,
  ChatState, RunState, RunEventPayload, RunEventType, SteerMessage,
  ClarificationQuestion, ClarificationOption, ClarificationAnswers,
} from './types';

// ── Helper ──────────────────────────────────────────────────────────

function ensureSession(state: ChatState, sessionId: string) {
  if (state.entries[sessionId] === undefined) state.entries[sessionId] = [];
  if (state.subagents[sessionId] === undefined) state.subagents[sessionId] = {};
  if (state._thinkBuffers[sessionId] === undefined) state._thinkBuffers[sessionId] = {};
  if (state.processing[sessionId] === undefined) state.processing[sessionId] = false;
  if (state.runId[sessionId] === undefined) state.runId[sessionId] = null;
  if (state.runState[sessionId] === undefined) state.runState[sessionId] = null;
  if (state.todo[sessionId] === undefined) state.todo[sessionId] = [];
  if (state.parkedPlans[sessionId] === undefined) state.parkedPlans[sessionId] = [];
  if (state.activePlanId[sessionId] === undefined) state.activePlanId[sessionId] = null;
  if (state.activePlanTitle[sessionId] === undefined) state.activePlanTitle[sessionId] = null;
  if (state.steerQueue[sessionId] === undefined) state.steerQueue[sessionId] = [];
  if (state.allPrompts[sessionId] === undefined) state.allPrompts[sessionId] = [];
  if (state.visiblePromptsCount[sessionId] === undefined) state.visiblePromptsCount[sessionId] = 1;
  if (state.isDirty[sessionId] === undefined) state.isDirty[sessionId] = false;
  if (state.contentRevision[sessionId] === undefined) state.contentRevision[sessionId] = 0;
  if (state.persistedRevision[sessionId] === undefined) state.persistedRevision[sessionId] = 0;
  if (state._resumedFromBackend[sessionId] === undefined) state._resumedFromBackend[sessionId] = false;
  if (state.goal[sessionId] === undefined) state.goal[sessionId] = null;
  if (state.goalCompleted[sessionId] === undefined) state.goalCompleted[sessionId] = false;
  if (state.viewingSubagentPath[sessionId] === undefined) state.viewingSubagentPath[sessionId] = [];
  if (state.btwEntries[sessionId] === undefined) state.btwEntries[sessionId] = [];
  if (state.isResuming[sessionId] === undefined) state.isResuming[sessionId] = false;
  if (state._pendingTurnId[sessionId] === undefined) state._pendingTurnId[sessionId] = undefined;
}

function markDirty(state: ChatState, sessionId: string) {
  ensureSession(state, sessionId);
  state.contentRevision[sessionId] += 1;
  state.isDirty[sessionId] = true;
}

function markPersisted(state: ChatState, sessionId: string) {
  ensureSession(state, sessionId);
  state.persistedRevision[sessionId] = state.contentRevision[sessionId];
  state.isDirty[sessionId] = false;
}

function payloadEventName(payload: string | Record<string, unknown>): string | undefined {
  if (typeof payload === 'string') {
    try {
      return JSON.parse(payload)?.event;
    } catch {
      return undefined;
    }
  }
  return typeof payload.event === 'string' ? payload.event : undefined;
}

function isTerminalPayload(payload: string | Record<string, unknown>): boolean {
  const event = payloadEventName(payload);
  return event === 'run_completed' || event === 'run_cancelled' || event === 'run_failed';
}

// ── Initial state ────────────────────────────────────────────────────

const initialState: ChatState = {
  entries: {},
  processing: {},
  subagents: {},
  runId: {},
  runState: {},
  todo: {},
  parkedPlans: {},
  activePlanId: {},
  activePlanTitle: {},
  steerQueue: {},
  allPrompts: {},
  visiblePromptsCount: {},
  isDirty: {},
  contentRevision: {},
  persistedRevision: {},
  _resumedFromBackend: {},
  _thinkBuffers: {},
  goal: {},
  goalCompleted: {},
  viewingSubagentPath: {},
  btwEntries: {},
  isResuming: {},
  _pendingTurnId: {},
  runIdToSessionId: {},
  lastSeqByRun: {},
  skillsCache: null,
  resyncingByRun: {},
  pendingGapByRun: {},
  cacheMetricsByRun: {},
  appliedEventIdsByRun: {},
  pendingEventsByRun: {},
};

// ── Resync thunk ─────────────────────────────────────────────────────

export const resyncRun = createAsyncThunk<
  void,
  { runId: string; fromSeq: number }
>('chat/resyncRun', async ({ runId, fromSeq }, { dispatch, getState }) => {
  const state = getState() as { chat: ChatState };
  if (state.chat.resyncingByRun[runId]) return;
  dispatch(setResyncing({ runId, value: true }));
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
    dispatch(setResyncing({ runId, value: false }));
  }
});

// ── Skills thunk ───────────────────────────────────────────────────────

export const fetchSkills = createAsyncThunk(
  'chat/fetchSkills',
  async (_, { getState, dispatch }) => {
    const state = getState() as { chat: ChatState; project?: SkillScopeProjectState };
    const { sessionId, workspace, scopeKey } = resolveSkillScope(state.project);
    const cached = state.chat.skillsCache;
    if (cached && cached.scopeKey === scopeKey && Date.now() - cached.loadedAt < 25000) {
      return cached.skills;
    }
    const skills = await invoke<import('./types').SkillManifest[]>('get_skills', { sessionId, workspace });
    dispatch(cacheSkills({ skills, scopeKey }));
    return skills;
  }
);

export const invalidateSkillsCache = createAsyncThunk(
  'chat/invalidateSkillsCache',
  async () => {
    await invoke('invalidate_skills_cache');
  }
);

// ── rebuildEntries ────────────────────────────────────────────────────

function rebuildEntries(state: ChatState, sessionId: string) {
  const prompts = state.allPrompts[sessionId];
  if (!prompts || prompts.length === 0) {
    state.entries[sessionId] = state.entries[sessionId] ?? [];
    return;
  }

  // 1. Rebuild prompt status map by turn_index for zombie detection.
  const promptStatusByTurn = new Map<number, string>();
  for (const p of prompts) {
    promptStatusByTurn.set(p.turn_index, p.status);
  }

  // 2. Clear and rebuild subagents from all prompts
  state.subagents[sessionId] = {};
  for (const prompt of prompts) {
    for (const msg of prompt.messages) {
      if (msg.role === 'assistant' && msg.metadata && msg.metadata.subagents) {
        const subMap = msg.metadata.subagents as Record<string, SubagentEntry>;
        for (const [subId, subEntry] of Object.entries(subMap)) {
          state.subagents[sessionId][subId] = subEntry;
        }
      }
    }
  }

  // 3. Get the visible prompts slice (last visiblePromptsCount prompts)
  const count = state.visiblePromptsCount[sessionId];
  const startIdx = Math.max(0, prompts.length - count);
  const visiblePrompts = prompts.slice(startIdx);

  const newEntries: ChatEntry[] = [];

  for (const prompt of visiblePrompts) {
    // Find user message text from prompt messages
    const userMsg = prompt.messages.find((m) => m.role === 'user')?.content || '';

    // A. Push the user entry
    newEntries.push({
      id: `user-${prompt.id}`,
      type: 'user',
      promptId: prompt.id,
      text: userMsg,
      model: prompt.model,
    });

    // B. Reconstruct the turn entry
    const turnIdx = prompt.turn_index;
    let blocks: TurnBlock[] = [];
    let startTime: number | undefined = undefined;
    let endTime: number | undefined = undefined;
    let cacheHitRate: number | undefined = undefined;
    let turnIds: string[] | undefined = undefined;

    // Find the assistant message in the prompt
    const assistantMsg = prompt.messages.find((m) => m.role === 'assistant');
    if (assistantMsg && assistantMsg.metadata) {
      const meta = assistantMsg.metadata;
      if (Array.isArray(meta.blocks)) {
        blocks = [...meta.blocks];
      }
      startTime = meta.startTime;
      endTime = meta.endTime;
      cacheHitRate = meta.cacheHitRate;
      turnIds = meta.turnIds;
    }

    // Canonical fallback: fold every provider message in order. A prompt may
    // contain several assistant(tool_calls) -> tool -> assistant iterations;
    // restoring only the first assistant silently hid most of the conversation.
    if (blocks.length === 0) {
      const toolBlocks = new Map<string, Extract<TurnBlock, { type: 'tool' }>>();
      for (const message of prompt.messages) {
        if (message.role === 'assistant') {
          if (message.content) {
            const hasThinkTag = message.content.match(/<think>([\s\S]*?)<\/think>/);
            if (hasThinkTag) {
              blocks.push({ type: 'thinking', text: hasThinkTag[1], isStreaming: false });
              const visible = message.content.replace(/<think>[\s\S]*?<\/think>/, '').trim();
              if (visible) blocks.push({ type: 'assistant', text: visible, isStreaming: false });
            } else {
              blocks.push({ type: 'assistant', text: message.content, isStreaming: false });
            }
          }
          for (const tc of message.tool_calls ?? []) {
            let args: unknown = tc.function.arguments;
            try { args = JSON.parse(tc.function.arguments); } catch { /* retain raw args */ }
            const toolBlock: Extract<TurnBlock, { type: 'tool' }> = {
              type: 'tool',
              call_id: tc.id,
              name: tc.function.name,
              args,
              result: '',
              active: false,
              is_error: false,
            };
            toolBlocks.set(tc.id, toolBlock);
            blocks.push(toolBlock);
          }
        } else if (message.role === 'tool' && message.tool_call_id) {
          const toolBlock = toolBlocks.get(message.tool_call_id);
          if (toolBlock) {
            toolBlock.result = message.content ?? '';
            if (message.name) toolBlock.name = message.name;
          } else {
            blocks.push({
              type: 'tool',
              call_id: message.tool_call_id,
              name: message.name ?? 'tool',
              result: message.content ?? '',
              active: false,
              is_error: false,
            });
          }
        }
      }
    }

    // Fallback timing
    if (!startTime && prompt.started_at) {
      const parsed = new Date(prompt.started_at).getTime();
      if (!isNaN(parsed)) startTime = parsed;
    }
    const isCompletedStatus =
      prompt.status === 'completed' ||
      prompt.status === 'cancelled' ||
      prompt.status === 'failed' ||
      prompt.status === 'interrupted';
    if (isCompletedStatus && !endTime) {
      if (prompt.ended_at) {
        const parsed = new Date(prompt.ended_at).getTime();
        if (!isNaN(parsed)) endTime = parsed;
      }
      if (!endTime && startTime) {
        endTime = startTime + 5000;
      }
      if (!endTime) {
        endTime = Date.now();
      }
    }

    const subagentIds = blocks
      .filter((b) => b.type === 'subagent_ref')
      .map((b) => (b as Extract<TurnBlock, { type: 'subagent_ref' }>).subagent_id);

    const pStatus = promptStatusByTurn.get(turnIdx);
    newEntries.push({
      id: `turn-${prompt.id}`,
      type: 'turn',
      promptId: prompt.id,
      turnIndex: turnIdx,
      blocks,
      subagentIds: subagentIds.length > 0 ? subagentIds : undefined,
      startTime,
      endTime,
      cacheHitRate,
      turnIds,
      interrupted: pStatus === 'interrupted' || pStatus === 'cancelled',
    });
  }

  const existingEntries = state.entries[sessionId] ?? [];
  const existingByKey = new Map<string, ChatEntry>();
  for (const e of existingEntries) {
    existingByKey.set(`${e.type}:${e.promptId ?? e.id}`, e);
  }

  const mergedEntries = newEntries.map((entry) => {
    if (!entry.promptId) return entry;
    const existing = existingByKey.get(`${entry.type}:${entry.promptId}`);
    if (!existing) return entry;
    if (entry.type === 'turn' && existing.type === 'turn') {
      const existingBlocks = existing.blocks?.length ?? 0;
      const rebuiltBlocks = entry.blocks?.length ?? 0;
      return existingBlocks >= rebuiltBlocks ? existing : entry;
    }
    if (entry.type === 'user' && existing.type === 'user') {
      return existing.text ? existing : entry;
    }
    return entry;
  });

  const rebuiltKeys = new Set(mergedEntries.map((e) => `${e.type}:${e.promptId ?? e.id}`));
  for (const existing of existingEntries) {
    const key = `${existing.type}:${existing.promptId ?? existing.id}`;
    if (!rebuiltKeys.has(key)) {
      mergedEntries.push(existing);
    }
  }

  state.entries[sessionId] = mergedEntries;
}

// ── Slice ────────────────────────────────────────────────────────────

export const chatSlice = createSlice({
  name: 'chat',
  initialState,
  reducers: {
    loadMorePrompts: (state, action: PayloadAction<{ sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.visiblePromptsCount[sid] += 2;
      rebuildEntries(state, sid);
    },
    userMessageSent: (state, action: PayloadAction<{ text: string; model?: string; sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      state.entries[sid].push({
        id: `user-${crypto.randomUUID()}`,
        type: 'user',
        promptId: Date.now().toString(),
        text: action.payload.text,
        model: action.payload.model,
      });
      const newPrompt: FrontendPrompt = {
        id: `user-prompt-${Date.now()}-${Math.random()}`,
        session_id: sid,
        turn_index: state.allPrompts[sid].length,
        model: action.payload.model ?? '',
        status: 'running',
        token_usage: {},
        started_at: new Date().toISOString(),
        ended_at: null,
        created_at: new Date().toISOString(),
        messages: [{
          role: 'user',
          content: action.payload.text,
          model: action.payload.model,
        }],
      };
      state.allPrompts[sid].push(newPrompt);
      // Materialize only the newly appended prompt. Raising this to the full
      // prompt count would claim that older, hidden prompts exist in `entries`
      // and the next full save would silently omit them.
      state.visiblePromptsCount[sid] = Math.min(
        state.allPrompts[sid].length,
        state.visiblePromptsCount[sid] + 1,
      );

      state.processing[sid] = true;
      state._resumedFromBackend[sid] = false;
      markDirty(state, sid);
      // Keep durable plans across sends — do not wipe todo/parked on user message.

      state.entries[sid].push({
        id: `turn-pending-${Date.now()}`,
        type: 'turn',
        promptId: newPrompt.id,
        turnIndex: state.allPrompts[sid].length - 1,
        blocks: [],
        startTime: Date.now(),
      });
    },
    runIdSet: (state, action: PayloadAction<{ runId: string; sessionId: string; promptId?: string }>) => {
      const { runId, sessionId: sid, promptId } = action.payload;
      ensureSession(state, sid);

      state.runId[sid] = runId;
      state.runIdToSessionId[runId] = sid;
      state.runState[sid] = 'running';

      // Bind entry/prompt identity ONLY from the backend prompts-table id.
      // Never use runId as promptId — they are different UUIDs.
      if (!promptId) return;

      const entries = state.entries[sid];
      let lastUserIndex = -1;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].type === 'user') {
          lastUserIndex = i;
          break;
        }
      }
      if (lastUserIndex !== -1) {
        entries[lastUserIndex].id = `user-${promptId}`;
        entries[lastUserIndex].promptId = promptId;
      }

      let lastTurnIndex = -1;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].type === 'turn') {
          lastTurnIndex = i;
          break;
        }
      }
      if (lastTurnIndex !== -1 && lastTurnIndex > lastUserIndex) {
        entries[lastTurnIndex].id = `turn-${promptId}`;
        entries[lastTurnIndex].promptId = promptId;
      }

      const prompts = state.allPrompts[sid];
      if (prompts.length > 0) {
        const lastPrompt = prompts[prompts.length - 1];
        if (lastPrompt.status === 'running' || lastPrompt.id.startsWith('user-prompt-') || lastPrompt.id.startsWith('retry-prompt-')) {
          lastPrompt.id = promptId;
        }
      }
    },
    runStateChanged: (state, action: PayloadAction<{ sessionId: string; runState: RunState }>) => {
      const { sessionId: sid, runState } = action.payload;
      ensureSession(state, sid);

      state.runState[sid] = runState;
      if (runState === 'completed' || runState === 'cancelled' || runState === 'failed') {
        state.processing[sid] = false;
      }
    },
    agentEventReceived: (state, action: PayloadAction<string | Record<string, unknown>>) => {
      const sid = processSingleEvent(state, action.payload);
      if (sid) {
        if (isTerminalPayload(action.payload)) markPersisted(state, sid);
        else markDirty(state, sid);
      }
    },
    agentEventsBatch: (state, action: PayloadAction<Array<string | Record<string, unknown>>>) => {
      const dirtySessionIds = new Set<string>();
      const persistedSessionIds = new Set<string>();
      for (const payload of action.payload) {
        const sid = processSingleEvent(state, payload);
        if (sid) {
          if (isTerminalPayload(payload)) persistedSessionIds.add(sid);
          else dirtySessionIds.add(sid);
        }
      }
      for (const sid of dirtySessionIds) markDirty(state, sid);
      for (const sid of persistedSessionIds) markPersisted(state, sid);
    },
    toolApprovalResponded: (state, action: PayloadAction<{ sessionId: string; promptId: string; approved: boolean }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      markDirty(state, sid);
      for (const entry of state.entries[sid]) {
        if (entry.type !== 'turn' || !entry.blocks) continue;
        const block = entry.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
        if (block && block.type === 'approval') {
          block.status = action.payload.approved ? 'approved' : 'denied';
          return;
        }
      }
      for (const sa of Object.values(state.subagents[sid])) {
        if (!sa.blocks) continue;
        const saBlock = sa.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
        if (saBlock && saBlock.type === 'approval') {
          saBlock.status = action.payload.approved ? 'approved' : 'denied';
          return;
        }
      }
    },
    clarificationAnswered: (state, action: PayloadAction<{
      sessionId: string;
      promptId: string;
      answers: Record<string, string[]>;
    }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      markDirty(state, sid);
      for (const entry of state.entries[sid]) {
        if (entry.type !== 'turn' || !entry.blocks) continue;
        const block = entry.blocks.find(
          (b) => b.type === 'clarification' && b.prompt_id === action.payload.promptId
        );
        if (block && block.type === 'clarification') {
          block.status = 'answered';
          block.answers = action.payload.answers;
          return;
        }
      }
    },
    goalCleared: (state, action: PayloadAction<{ sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.goal[sid] = null;
      state.goalCompleted[sid] = false;
      state.todo[sid] = [];
      state.parkedPlans[sid] = [];
      state.activePlanId[sid] = null;
      state.activePlanTitle[sid] = null;
      markDirty(state, sid);
    },
    clearChat: (state, action: PayloadAction<string>) => {
      const sid = action.payload;
      state.entries[sid] = [];
      state.subagents[sid] = {};
      state.processing[sid] = false;
      state.goal[sid] = null;
      state.goalCompleted[sid] = false;
      markPersisted(state, sid);
      state._resumedFromBackend[sid] = false;
      state.todo[sid] = [];
      state.parkedPlans[sid] = [];
      state.activePlanId[sid] = null;
      state.activePlanTitle[sid] = null;
      state.steerQueue[sid] = [];
      state.allPrompts[sid] = [];
      state.visiblePromptsCount[sid] = 1;
      state._thinkBuffers[sid] = {};
      state.viewingSubagentPath[sid] = [];
      state.btwEntries[sid] = [];
      state.isResuming[sid] = false;
      state._pendingTurnId[sid] = undefined;
    },
    plansHydrated: (
      state,
      action: PayloadAction<{
        sessionId: string;
        items: TodoItem[];
        parked: ParkedPlan[];
        activePlanId?: string | null;
        activePlanTitle?: string | null;
      }>,
    ) => {
      const { sessionId: sid, items, parked, activePlanId, activePlanTitle } = action.payload;
      ensureSession(state, sid);
      state.todo[sid] = items ?? [];
      state.parkedPlans[sid] = parked ?? [];
      state.activePlanId[sid] = activePlanId ?? null;
      state.activePlanTitle[sid] = activePlanTitle ?? null;
    },
    agentAborted: (state, action: PayloadAction<{ sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      state.processing[sid] = false;
      markDirty(state, sid);
      const entries = state.entries[sid];
      const last = entries[entries.length - 1];
      if (last && last.type === 'turn' && !last.endTime) {
        last.endTime = Date.now();
        stopDanglingSubagents(state.subagents[sid] ?? {}, last);
        if (last.blocks) {
          clearRecoverableNotices(last.blocks);
          last.blocks.push({ type: 'error', text: '— Interrupted —' });
        }
      }
    },
    sendFailed: (state, action: PayloadAction<{ sessionId: string; error: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.processing[sid] = false;
      state.runState[sid] = 'failed';
      markDirty(state, sid);
      const entries = state.entries[sid];
      const lastEntry = entries[entries.length - 1];
      if (lastEntry && lastEntry.type === 'turn' && !lastEntry.endTime) {
        lastEntry.endTime = Date.now();
        lastEntry.blocks = [{ type: 'error', text: action.payload.error }];
      } else {
        entries.push({
          id: `error-${Date.now()}`,
          type: 'turn',
          blocks: [{ type: 'error', text: action.payload.error }],
          startTime: Date.now(),
          endTime: Date.now(),
        });
      }
    },
    viewSubagent: (state, action: PayloadAction<{ sessionId: string; id: string; name: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.viewingSubagentPath[sid].push({ id: action.payload.id, name: action.payload.name });
    },
    popSubagentView: (state, action: PayloadAction<{ sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.viewingSubagentPath[sid].pop();
    },
    clearSubagentView: (state, action: PayloadAction<{ sessionId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.viewingSubagentPath[sid] = [];
    },
    setResyncing: (state, action: PayloadAction<{ runId: string; value: boolean }>) => {
      state.resyncingByRun[action.payload.runId] = action.payload.value;
    },
    clearPendingGap: (state, action: PayloadAction<string>) => {
      delete state.pendingGapByRun[action.payload];
    },
    cacheSkills: (state, action: PayloadAction<{
      skills: import('./types').SkillManifest[];
      scopeKey: string;
    }>) => {
      state.skillsCache = {
        skills: action.payload.skills,
        loadedAt: Date.now(),
        scopeKey: action.payload.scopeKey,
      };
    },
    clearSkillsCache: (state) => {
      state.skillsCache = null;
    },
    steerMessageQueued: (state, action: PayloadAction<{ sessionId: string; steerId: string; text: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      const { steerId, text } = action.payload;
      state.steerQueue[sid].push({
        steerId,
        text,
        status: 'pending',
        timestamp: Date.now(),
      });
      state.entries[sid].push({
        id: `steer-${steerId}`,
        type: 'user',
        text,
        isSteer: true,
        steerId,
        steerStatus: 'pending',
      });
      markDirty(state, sid);
    },
    steerMessageInjected: (state, action: PayloadAction<{ sessionId: string; steerId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      const { steerId } = action.payload;
      const sq = state.steerQueue[sid].find((s) => s.steerId === steerId);
      if (sq) sq.status = 'injected';
      for (const entry of state.entries[sid]) {
        if (entry.type === 'user' && entry.isSteer && entry.steerId === steerId) {
          entry.steerStatus = 'injected';
        }
      }
      markDirty(state, sid);
    },
    steerMessageCancelled: (state, action: PayloadAction<{ sessionId: string; steerId: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);

      const { steerId } = action.payload;
      state.steerQueue[sid] = state.steerQueue[sid].filter((s) => s.steerId !== steerId);
      state.entries[sid] = state.entries[sid].filter(
        (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
      );
      markDirty(state, sid);
    },
    // ── /btw side-channel ──────────────────────────────────────────
    btwAsked: (state, action: PayloadAction<{ sessionId: string; id: string; question: string }>) => {
      const sid = action.payload.sessionId;
      ensureSession(state, sid);
      state.btwEntries[sid].push({
        id: action.payload.id,
        question: action.payload.question,
        answer: '',
        isStreaming: true,
        startTime: Date.now(),
      });
    },
    btwDelta: (state, action: PayloadAction<{ sessionId: string; id: string; text: string }>) => {
      const list = state.btwEntries[action.payload.sessionId];
      if (!list) return;
      const e = list.find((x) => x.id === action.payload.id);
      if (e) e.answer += action.payload.text;
    },
    btwDone: (state, action: PayloadAction<{ sessionId: string; id: string }>) => {
      const list = state.btwEntries[action.payload.sessionId];
      if (!list) return;
      const e = list.find((x) => x.id === action.payload.id);
      if (e) { e.isStreaming = false; e.endTime = Date.now(); }
    },
    btwError: (state, action: PayloadAction<{ sessionId: string; id: string; text: string }>) => {
      const list = state.btwEntries[action.payload.sessionId];
      if (!list) return;
      const e = list.find((x) => x.id === action.payload.id);
      if (e) { e.isStreaming = false; if (!e.answer) e.answer = `⚠ ${action.payload.text}`; e.endTime = Date.now(); }
    },

  },
  extraReducers: (builder) => {
    builder.addCase(invalidateSkillsCache.fulfilled, (state) => {
      state.skillsCache = null;
    });
    builder.addCase(resumeSession.pending, (state, action) => {
      const sessionId = action.meta.arg;
      state.isResuming[sessionId] = true;
    });
    builder.addCase(resumeSession.rejected, (state, action) => {
      const sessionId = action.meta.arg;
      state.isResuming[sessionId] = false;
    });
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      const sessionId = action.payload.meta.id;
      state.isResuming[sessionId] = false;
      const meta = action.payload.meta as {
        pinned_goal?: string | null;
        goal_completed?: boolean;
      };
      // Always hydrate session-level goal (even if entries already cached).
      ensureSession(state, sessionId);
      state.goal[sessionId] = meta.pinned_goal?.trim() ? meta.pinned_goal : null;
      state.goalCompleted[sessionId] = !!meta.goal_completed;
      if (state.entries[sessionId]?.length > 0) return;
      const { prompts } = action.payload;
      state.entries[sessionId] = [];
      state.allPrompts[sessionId] = prompts ?? [];
      state.processing[sessionId] = state.allPrompts[sessionId].some(p => p.status === 'running');
      // Initially render only the last 2 prompts (or 1 if only 1 exists)
      state.visiblePromptsCount[sessionId] = Math.min(2, state.allPrompts[sessionId].length);
      rebuildEntries(state, sessionId);
      state._resumedFromBackend[sessionId] = true;
      state.contentRevision[sessionId] = 0;
      state.persistedRevision[sessionId] = 0;
      state.isDirty[sessionId] = false;
    });
    builder.addCase(deleteSession.fulfilled, (state, action) => {
      const { sessionId } = action.payload;
      delete state.entries[sessionId];
      delete state.processing[sessionId];
      delete state.subagents[sessionId];
      delete state.runId[sessionId];
      delete state.runState[sessionId];
      delete state.todo[sessionId];
      delete state.parkedPlans[sessionId];
      delete state.activePlanId[sessionId];
      delete state.activePlanTitle[sessionId];
      delete state.steerQueue[sessionId];
      delete state.allPrompts[sessionId];
      delete state.visiblePromptsCount[sessionId];
      delete state.isDirty[sessionId];
      delete state.contentRevision[sessionId];
      delete state.persistedRevision[sessionId];
      delete state._resumedFromBackend[sessionId];
      delete state._thinkBuffers[sessionId];
      delete state.goal[sessionId];
      delete state.goalCompleted[sessionId];
      delete state.viewingSubagentPath[sessionId];
      delete state.btwEntries[sessionId];

      delete state.isResuming[sessionId];
      delete state._pendingTurnId[sessionId];
    });
  },
});

// ── Exports ──────────────────────────────────────────────────────────

export const {
  userMessageSent,
  agentEventReceived,
  agentEventsBatch,
  toolApprovalResponded,
  clarificationAnswered,
  goalCleared,
  clearChat,
  plansHydrated,
  agentAborted,
  loadMorePrompts,
  sendFailed,
  runIdSet,
  runStateChanged,
  viewSubagent,
  popSubagentView,
  clearSubagentView,
  setResyncing,
  clearPendingGap,
  cacheSkills,
  clearSkillsCache,
  steerMessageQueued,
  steerMessageInjected,
  steerMessageCancelled,
  btwAsked,
  btwDelta,
  btwDone,
  btwError,
} = chatSlice.actions;
export default chatSlice.reducer;
