import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession, deleteSession, saveSessionMessages, setActiveSession, createSession } from '../project/projectSlice';

import type {
  TurnBlock, SubagentEntry, ChatState, RunState, ChatEntry, FrontendPrompt,
} from './types';
import { processSingleEvent, stopDanglingSubagents } from './eventHandlers';

// ── Re-export types, selectors, and utils for backward compatibility ─
export type {
  TodoItem, TurnBlock, SubagentBlock, SubagentEntry, ChatEntry,
  ChatState, RunState, RunEventPayload, RunEventType, SteerMessage,
} from './types';
export {
  selectEntryIds, selectEntryById, selectSubagentById,
  selectPendingApprovalCount, selectActivePendingApproval,
} from './selectors';
export { entriesToMessages, stringifyResult, getFullMessages, getFullMessagesForSession, getTimingMetrics } from './utils';

// ── Helper ──────────────────────────────────────────────────────────

function ensureSession(state: ChatState, sessionId: string) {
  if (state.entries[sessionId] === undefined) state.entries[sessionId] = [];
  if (state.subagents[sessionId] === undefined) state.subagents[sessionId] = {};
  if (state._thinkBuffers[sessionId] === undefined) state._thinkBuffers[sessionId] = {};
  if (state.processing[sessionId] === undefined) state.processing[sessionId] = false;
  if (state.runId[sessionId] === undefined) state.runId[sessionId] = null;
  if (state.runState[sessionId] === undefined) state.runState[sessionId] = null;
  if (state.todo[sessionId] === undefined) state.todo[sessionId] = [];
  if (state.steerQueue[sessionId] === undefined) state.steerQueue[sessionId] = [];
  if (state.allPrompts[sessionId] === undefined) state.allPrompts[sessionId] = [];
  if (state.visiblePromptsCount[sessionId] === undefined) state.visiblePromptsCount[sessionId] = 1;
  if (state.isDirty[sessionId] === undefined) state.isDirty[sessionId] = false;
  if (state._resumedFromBackend[sessionId] === undefined) state._resumedFromBackend[sessionId] = false;
  if (state.goal[sessionId] === undefined) state.goal[sessionId] = null;
  if (state.goalCompleted[sessionId] === undefined) state.goalCompleted[sessionId] = false;
}

// ── Initial state ────────────────────────────────────────────────────

const initialState: ChatState = {
  entries: {},
  processing: {},
  subagents: {},
  runId: {},
  runState: {},
  todo: {},
  steerQueue: {},
  allPrompts: {},
  visiblePromptsCount: {},
  isDirty: {},
  _resumedFromBackend: {},
  _thinkBuffers: {},
  goal: {},
  goalCompleted: {},
  activeSessionId: null,
  runIdToSessionId: {},
  lastSeqByRun: {},
  viewingSubagentPath: [],
  btwEntries: [],
  learnEntries: [],
  skillsCache: null,
  resyncing: false,
  isResuming: false,
  _pendingGap: null,
  cacheMetrics: null,
};

// ── Resync thunk ─────────────────────────────────────────────────────

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

// ── Skills thunk ───────────────────────────────────────────────────────

export const fetchSkills = createAsyncThunk(
  'chat/fetchSkills',
  async (_, { getState, dispatch }) => {
    const state = getState() as { chat: ChatState };
    const cached = state.chat.skillsCache;
    if (cached && Date.now() - cached.loadedAt < 25000) {
      return cached.skills;
    }
    const skills = await invoke<import('./types').SkillManifest[]>('get_skills');
    dispatch(cacheSkills(skills));
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

    // Fallback: If metadata/blocks is missing (e.g. legacy session), reconstruct blocks from messages
    if (blocks.length === 0) {
      // 1. Thinking block from <think> tags in assistant message content
      if (assistantMsg && assistantMsg.content) {
        const hasThinkTag = assistantMsg.content.match(/<think>([\s\S]*?)<\/think>/);
        if (hasThinkTag) {
          const thinkContent = hasThinkTag[1];
          const restContent = assistantMsg.content.replace(/<think>[\s\S]*?<\/think>/, '').trim();
          blocks.push({ type: 'thinking', text: thinkContent, isStreaming: false });
          if (restContent) {
            blocks.push({ type: 'assistant', text: restContent, isStreaming: false });
          }
        } else {
          blocks.push({ type: 'assistant', text: assistantMsg.content, isStreaming: false });
        }
      }

      // 2. Tool blocks from tool messages or tool_calls
      if (assistantMsg && assistantMsg.tool_calls) {
        for (const tc of assistantMsg.tool_calls) {
          const toolMsg = prompt.messages.find(m => m.role === 'tool' && m.tool_call_id === tc.id);
          blocks.push({
            type: 'tool',
            call_id: tc.id,
            name: tc.function.name,
            args: tc.function.arguments,
            result: toolMsg?.content || '',
            active: false,
            is_error: false,
          });
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
  const mergedEntries = newEntries.map((entry) => {
    if (!entry.promptId) return entry;
    const existing = existingEntries.find((e) => e.type === entry.type && e.promptId === entry.promptId);
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
    loadMorePrompts: (state) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);
      state.visiblePromptsCount[sid] += 2;
      rebuildEntries(state, sid);
    },
    userMessageSent: (state, action: PayloadAction<{ text: string; model?: string; sessionId?: string }>) => {
      const sid = action.payload.sessionId ?? state.activeSessionId;
      if (!sid) return;
      if (action.payload.sessionId) {
        state.activeSessionId = action.payload.sessionId;
      }
      ensureSession(state, sid);

      state.entries[sid].push({
        id: `user-${Date.now()}`,
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
      state.visiblePromptsCount[sid] = Math.max(state.visiblePromptsCount[sid], state.allPrompts[sid].length);

      state.processing[sid] = true;
      state._resumedFromBackend[sid] = false;
      state.isDirty[sid] = true;
      state.todo[sid] = [];
    },
    runIdSet: (state, action: PayloadAction<string | { runId: string; sessionId?: string }>) => {
      const runId = typeof action.payload === 'string' ? action.payload : action.payload.runId;
      const sid = typeof action.payload === 'string'
        ? state.activeSessionId
        : action.payload.sessionId ?? state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      state.activeSessionId = sid;
      state.runId[sid] = runId;
      state.runIdToSessionId[runId] = sid;
      state.runState[sid] = 'running';

      // Update the ID of the latest user entry to use the new runId
      const entries = state.entries[sid];
      let lastUserIndex = -1;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].type === 'user') {
          lastUserIndex = i;
          break;
        }
      }
      if (lastUserIndex !== -1) {
        entries[lastUserIndex].id = `user-${runId}`;
        entries[lastUserIndex].promptId = runId;
      }

      // Update the ID of the latest turn entry if it exists
      let lastTurnIndex = -1;
      for (let i = entries.length - 1; i >= 0; i--) {
        if (entries[i].type === 'turn') {
          lastTurnIndex = i;
          break;
        }
      }
      if (lastTurnIndex !== -1 && lastTurnIndex > lastUserIndex) {
        entries[lastTurnIndex].id = `turn-${runId}`;
        entries[lastTurnIndex].promptId = runId;
      }

      // Update the ID of the last prompt in allPrompts
      const prompts = state.allPrompts[sid];
      if (prompts.length > 0) {
        const lastPrompt = prompts[prompts.length - 1];
        if (lastPrompt.status === 'running' || lastPrompt.id.startsWith('user-prompt-')) {
          lastPrompt.id = runId;
        }
      }
    },
    runStateChanged: (state, action: PayloadAction<RunState>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      state.runState[sid] = action.payload;
      if (action.payload === 'completed' || action.payload === 'cancelled' || action.payload === 'failed') {
        state.processing[sid] = false;
      }
    },
    agentEventReceived: (state, action: PayloadAction<string | Record<string, unknown>>) => {
      const sid = processSingleEvent(state, action.payload);
      if (sid) state.isDirty[sid] = true;
    },
    agentEventsBatch: (state, action: PayloadAction<Array<string | Record<string, unknown>>>) => {
      const dirtySessionIds = new Set<string>();
      for (const payload of action.payload) {
        const sid = processSingleEvent(state, payload);
        if (sid) dirtySessionIds.add(sid);
      }
      for (const sid of dirtySessionIds) state.isDirty[sid] = true;
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      state.isDirty[sid] = true;
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
    clearChat: (state, action: PayloadAction<string | undefined>) => {
      const sid = action.payload ?? state.activeSessionId;
      if (!sid) return;
      state.entries[sid] = [];
      state.subagents[sid] = {};
      state.processing[sid] = false;
      state.goal[sid] = null;
      state.goalCompleted[sid] = false;
      state.isDirty[sid] = false;
      state._resumedFromBackend[sid] = false;
      state.todo[sid] = [];
      state.steerQueue[sid] = [];
      state.allPrompts[sid] = [];
      state.visiblePromptsCount[sid] = 1;
      state._thinkBuffers[sid] = {};
      state.viewingSubagentPath = [];
      state.btwEntries = [];
      state.learnEntries = [];
    },
    agentAborted: (state) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      state.processing[sid] = false;
      state.isDirty[sid] = true;
      const entries = state.entries[sid];
      const last = entries[entries.length - 1];
      if (last && last.type === 'turn' && !last.endTime) {
        last.endTime = Date.now();
        stopDanglingSubagents(state.subagents[sid] ?? {}, last);
        if (last.blocks) {
          last.blocks.push({ type: 'error', text: '— Interrupted —' });
        }
      }
    },
    retryFromEntry: (state, action: PayloadAction<{ id: string; text?: string }>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      const entries = state.entries[sid];
      const entryId = action.payload.id;
      const idx = entries.findIndex((e) => e.id === entryId);
      if (idx === -1) return;
      const original = entries[idx];
      const userText = action.payload.text ?? original.text ?? '';
      const originalModel = original.model;
      const originalPromptId = original.promptId;
      entries.splice(idx);
      if (originalPromptId) {
        const promptIdx = state.allPrompts[sid].findIndex((p) => p.id === originalPromptId);
        if (promptIdx !== -1) {
          state.allPrompts[sid].splice(promptIdx);
        }
      }
      const promptId = `retry-prompt-${Date.now()}-${Math.random()}`;
      entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        promptId,
        text: userText,
        model: originalModel,
      });
      state.allPrompts[sid].push({
        id: promptId,
        session_id: sid,
        turn_index: state.allPrompts[sid].length,
        model: originalModel ?? '',
        status: 'running',
        token_usage: {},
        started_at: new Date().toISOString(),
        ended_at: null,
        created_at: new Date().toISOString(),
        messages: [{
          role: 'user',
          content: userText,
          model: originalModel,
        }],
      });
      state.visiblePromptsCount[sid] = Math.max(state.visiblePromptsCount[sid], state.allPrompts[sid].length);
      state.processing[sid] = true;
      state._resumedFromBackend[sid] = false;
      state.isDirty[sid] = true;
    },
    sendFailed: (state, action: PayloadAction<{ sessionId?: string; error: string }>) => {
      const sid = action.payload.sessionId ?? state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);
      state.processing[sid] = false;
      state.runState[sid] = 'failed';
      state.isDirty[sid] = true;
      state.entries[sid].push({
        id: `error-${Date.now()}`,
        type: 'turn',
        blocks: [{ type: 'error', text: action.payload.error }],
        startTime: Date.now(),
        endTime: Date.now(),
      });
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
    cacheSkills: (state, action: PayloadAction<import('./types').SkillManifest[]>) => {
      state.skillsCache = {
        skills: action.payload,
        loadedAt: Date.now(),
      };
    },
    clearSkillsCache: (state) => {
      state.skillsCache = null;
    },
    steerMessageQueued: (state, action: PayloadAction<{ steerId: string; text: string }>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
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
      state.isDirty[sid] = true;
    },
    steerMessageInjected: (state, action: PayloadAction<string>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      const steerId = action.payload;
      const sq = state.steerQueue[sid].find((s) => s.steerId === steerId);
      if (sq) sq.status = 'injected';
      for (const entry of state.entries[sid]) {
        if (entry.type === 'user' && entry.isSteer && entry.steerId === steerId) {
          entry.steerStatus = 'injected';
        }
      }
      state.isDirty[sid] = true;
    },
    steerMessageCancelled: (state, action: PayloadAction<string>) => {
      const sid = state.activeSessionId;
      if (!sid) return;
      ensureSession(state, sid);

      const steerId = action.payload;
      state.steerQueue[sid] = state.steerQueue[sid].filter((s) => s.steerId !== steerId);
      state.entries[sid] = state.entries[sid].filter(
        (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
      );
      state.isDirty[sid] = true;
    },
    // ── /btw side-channel ──────────────────────────────────────────
    btwAsked: (state, action: PayloadAction<{ id: string; question: string }>) => {
      state.btwEntries.push({ ...action.payload, answer: '', isStreaming: true, startTime: Date.now() });
    },
    btwDelta: (state, action: PayloadAction<{ id: string; text: string }>) => {
      const e = state.btwEntries.find((x) => x.id === action.payload.id);
      if (e) e.answer += action.payload.text;
    },
    btwDone: (state, action: PayloadAction<{ id: string }>) => {
      const e = state.btwEntries.find((x) => x.id === action.payload.id);
      if (e) { e.isStreaming = false; e.endTime = Date.now(); }
    },
    btwError: (state, action: PayloadAction<{ id: string; text: string }>) => {
      const e = state.btwEntries.find((x) => x.id === action.payload.id);
      if (e) { e.isStreaming = false; if (!e.answer) e.answer = `⚠ ${action.payload.text}`; e.endTime = Date.now(); }
    },
    // ── /learn memory ──────────────────────────────────────────────
    learnRequested: (state, action: PayloadAction<{ id: string; input: string }>) => {
      state.learnEntries.push({ ...action.payload, status: 'pending', timestamp: Date.now() });
    },
    learnSaved: (state, action: PayloadAction<{ id: string; title: string; rule: string }>) => {
      const e = state.learnEntries.find((x) => x.id === action.payload.id);
      if (e) { e.status = 'saved'; e.title = action.payload.title; e.rule = action.payload.rule; }
    },
    learnError: (state, action: PayloadAction<{ id: string; error: string }>) => {
      const e = state.learnEntries.find((x) => x.id === action.payload.id);
      if (e) { e.status = 'error'; e.error = action.payload.error; }
    },
  },
  extraReducers: (builder) => {
    builder.addCase(setActiveSession, (state, action) => {
      state.activeSessionId = action.payload;
    });
    builder.addCase(createSession.fulfilled, (state, action) => {
      state.activeSessionId = action.payload.session.id;
    });
    builder.addCase(resumeSession.pending, (state) => {
      state.isResuming = true;
    });
    builder.addCase(resumeSession.rejected, (state) => {
      state.isResuming = false;
    });
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      state.isResuming = false;
      const sessionId = action.payload.meta.id;
      state.activeSessionId = sessionId;
      if (state.entries[sessionId]?.length > 0) return;
      const { prompts } = action.payload;
      state.entries[sessionId] = [];
      state.allPrompts[sessionId] = prompts ?? [];
      state.processing[sessionId] = state.allPrompts[sessionId].some(p => p.status === 'running');
      // Initially render only the last 2 prompts (or 1 if only 1 exists)
      state.visiblePromptsCount[sessionId] = Math.min(2, state.allPrompts[sessionId].length);
      rebuildEntries(state, sessionId);
      state._resumedFromBackend[sessionId] = true;
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
      delete state.steerQueue[sessionId];
      delete state.allPrompts[sessionId];
      delete state.visiblePromptsCount[sessionId];
      delete state.isDirty[sessionId];
      delete state._resumedFromBackend[sessionId];
      delete state._thinkBuffers[sessionId];
      delete state.goal[sessionId];
      delete state.goalCompleted[sessionId];
    });
    builder.addCase(saveSessionMessages.fulfilled, (state, action) => {
      const sessionId = action.payload.sessionId;
      state.isDirty[sessionId] = false;
    });
  },
});

// ── Exports ──────────────────────────────────────────────────────────

export const {
  userMessageSent,
  agentEventReceived,
  agentEventsBatch,
  toolApprovalResponded,
  clearChat,
  agentAborted,
  loadMorePrompts,
  retryFromEntry,
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
  learnRequested,
  learnSaved,
  learnError,
} = chatSlice.actions;
export default chatSlice.reducer;
