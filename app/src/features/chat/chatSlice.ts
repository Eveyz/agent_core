import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession, deleteSession, saveSessionMessages } from '../project/projectSlice';

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
export { entriesToMessages, stringifyResult, getFullMessages, getTimingMetrics } from './utils';

// ── Initial state ────────────────────────────────────────────────────

const initialState: ChatState = {
  isDirty: false,
  isDirtyBySession: {},
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
  runIdToSessionId: {},
  activeSessionId: null,
  isResuming: false,
  _resumedFromBackend: false,
  _thinkBuffers: {},
  _pendingGap: null,
  todo: [],
  todoBySession: {},
  steerQueue: [],
  steerQueueBySession: {},
  skillsCache: null,
  btwEntries: [],
  learnEntries: [],
  goal: null,
  goalCompleted: false,
  allPrompts: [],
  visiblePromptsCount: 1,
  allPromptsBySession: {},
  visiblePromptsCountBySession: {},
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
      return cached.skills; // skip fetch if cache fresh
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

function rebuildEntries(state: ChatState) {
  if (!state.allPrompts || state.allPrompts.length === 0) {
    state.entries = [];
    return;
  }

  // 1. Rebuild prompt status map by turn_index for zombie detection.
  const promptStatusByTurn = new Map<number, string>();
  for (const p of state.allPrompts) {
    promptStatusByTurn.set(p.turn_index, p.status);
  }

  // 2. Clear and rebuild subagents from all prompts
  state.subagents = {};
  for (const prompt of state.allPrompts) {
    for (const msg of prompt.messages) {
      if (msg.role === 'assistant' && msg.metadata && msg.metadata.subagents) {
        const subMap = msg.metadata.subagents as Record<string, SubagentEntry>;
        for (const [subId, subEntry] of Object.entries(subMap)) {
          state.subagents[subId] = subEntry;
        }
      }
    }
  }

  // 3. Get the visible prompts slice (last visiblePromptsCount prompts)
  const count = state.visiblePromptsCount;
  const startIdx = Math.max(0, state.allPrompts.length - count);
  const visiblePrompts = state.allPrompts.slice(startIdx);

  const newEntries: ChatEntry[] = [];

  for (const prompt of visiblePrompts) {
    // Find user message text from prompt messages
    const userMsg = prompt.messages.find((m) => m.role === 'user')?.content || '';

    // A. Push the user entry
    newEntries.push({
      id: `user-${prompt.id}`,
      type: 'user',
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
      id: `turn-${turnIdx}-${prompt.id}`,
      type: 'turn',
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

  state.entries = newEntries;
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
      state.subagentsBySession[sessionId] = state.subagents;
      state.runIdBySession[sessionId] = state.runId;
      if (!state.todoBySession) {
        state.todoBySession = {};
      }
      state.todoBySession[sessionId] = state.todo;
      if (!state.steerQueueBySession) {
        state.steerQueueBySession = {};
      }
      state.steerQueueBySession[sessionId] = state.steerQueue;
      state.allPromptsBySession[sessionId] = state.allPrompts;
      state.visiblePromptsCountBySession[sessionId] = state.visiblePromptsCount;
      state.isDirtyBySession[sessionId] = state.isDirty;
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
        state.todo = state.todoBySession?.[sessionId] ?? [];
        state.steerQueue = state.steerQueueBySession?.[sessionId] ?? [];
        state.allPrompts = state.allPromptsBySession[sessionId] ?? [];
        state.visiblePromptsCount = state.visiblePromptsCountBySession[sessionId] ?? 1;
        state.isDirty = state.isDirtyBySession[sessionId] ?? false;
      } else {
        state.entries = [];
        state.isProcessing = false;
        state.subagents = {};
        state.runId = null;
        state.todo = [];
        state.steerQueue = [];
        state.allPrompts = [];
        state.visiblePromptsCount = 1;
        state.isDirty = false;
      }
      state.viewingSubagentPath = [];
      state._resumedFromBackend = false;
      state.btwEntries = [];
      state.learnEntries = [];
      state.goal = null;
      state.goalCompleted = false;
    },
    loadMorePrompts: (state) => {
      state.visiblePromptsCount += 2;
      rebuildEntries(state);
    },
    userMessageSent: (state, action: PayloadAction<{ text: string; model?: string }>) => {
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: action.payload.text,
        model: action.payload.model,
      });
      const newPrompt: FrontendPrompt = {
        id: `user-prompt-${Date.now()}-${Math.random()}`,
        session_id: state.activeSessionId ?? '',
        turn_index: state.allPrompts.length,
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
      state.allPrompts.push(newPrompt);
      state.visiblePromptsCount = Math.max(state.visiblePromptsCount, state.allPrompts.length);

      state.isProcessing = true;
      state._resumedFromBackend = false;
      state.isDirty = true;
      state.todo = [];
      if (state.activeSessionId) {
        if (!state.todoBySession) {
          state.todoBySession = {};
        }
        state.todoBySession[state.activeSessionId] = [];
      }
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
      state.isDirty = true;
    },
    agentEventsBatch: (state, action: PayloadAction<Array<string | Record<string, unknown>>>) => {
      for (const payload of action.payload) {
        processSingleEvent(state, payload);
      }
      state.isDirty = true;
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
      state.isDirty = true;
      for (const entry of state.entries) {
        if (entry.type !== 'turn' || !entry.blocks) continue;
        const block = entry.blocks.find((b) => b.type === 'approval' && b.prompt_id === action.payload.promptId);
        if (block && block.type === 'approval') {
          block.status = action.payload.approved ? 'approved' : 'denied';
          return;
        }
      }
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
      state.btwEntries = [];
      state.learnEntries = [];
      state.goal = null;
      state.goalCompleted = false;
      state.isDirty = false;
    },
    agentAborted: (state) => {
      state.isProcessing = false;
      state.isDirty = true;
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
      state.entries.splice(idx);
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: userText,
        model: state.entries[idx]?.model,
      });
      state.isProcessing = true;
      state._resumedFromBackend = false;
      state.isDirty = true;
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
      const { steerId, text } = action.payload;
      state.steerQueue.push({
        steerId,
        text,
        status: 'pending',
        timestamp: Date.now(),
      });
      state.entries.push({
        id: `steer-${steerId}`,
        type: 'user',
        text,
        isSteer: true,
        steerId,
        steerStatus: 'pending',
      });
      state.isDirty = true;
    },
    steerMessageInjected: (state, action: PayloadAction<string>) => {
      const steerId = action.payload;
      const sq = state.steerQueue.find((s) => s.steerId === steerId);
      if (sq) sq.status = 'injected';
      for (const entry of state.entries) {
        if (entry.type === 'user' && entry.isSteer && entry.steerId === steerId) {
          entry.steerStatus = 'injected';
        }
      }
      state.isDirty = true;
    },
    steerMessageCancelled: (state, action: PayloadAction<string>) => {
      const steerId = action.payload;
      state.steerQueue = state.steerQueue.filter((s) => s.steerId !== steerId);
      state.entries = state.entries.filter(
        (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
      );
      state.isDirty = true;
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
    builder.addCase(resumeSession.pending, (state) => {
      state.isResuming = true;
    });
    builder.addCase(resumeSession.rejected, (state) => {
      state.isResuming = false;
    });
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      state.isResuming = false;
      if (state.entries.length > 0) return;
      const { prompts } = action.payload;
      state.entries = [];

      state.allPrompts = prompts ?? [];
      state.isProcessing = state.allPrompts.some(p => p.status === 'running');
      // Initially render only the last 2 prompts (or 1 if only 1 exists)
      state.visiblePromptsCount = Math.min(2, state.allPrompts.length);

      rebuildEntries(state);
      state._resumedFromBackend = true;
      state.isDirty = false;
      state.isDirtyBySession[action.payload.meta.id] = false;
    });
    builder.addCase(deleteSession.fulfilled, (state, action) => {
      const { sessionId } = action.payload;
      delete state.entriesBySession[sessionId];
      delete state.processingBySession[sessionId];
      delete state.subagentsBySession[sessionId];
      delete state.runIdBySession[sessionId];
      delete state.allPromptsBySession[sessionId];
      delete state.visiblePromptsCountBySession[sessionId];
      delete state.isDirtyBySession[sessionId];
      if (state.todoBySession) {
        delete state.todoBySession[sessionId];
      }
      if (state.steerQueueBySession) {
        delete state.steerQueueBySession[sessionId];
      }
    });
    builder.addCase(saveSessionMessages.fulfilled, (state, action) => {
      const sessionId = action.payload.sessionId;
      state.isDirtyBySession[sessionId] = false;
      if (state.activeSessionId === sessionId) {
        state.isDirty = false;
      }
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
  cacheCurrentSession,
  restoreOrClearSession,
  loadMorePrompts,
  retryFromEntry,
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
