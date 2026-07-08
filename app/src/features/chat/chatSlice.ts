import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession, deleteSession } from '../project/projectSlice';

import type {
  TurnBlock, SubagentEntry, ChatState, RunState, EventLogEntry, ChatEntry, FrontendPrompt,
} from './types';
import { processSingleEvent, stopDanglingSubagents } from './eventHandlers';
import { entriesToEventLog } from './utils';

// ── Re-export types, selectors, and utils for backward compatibility ─
export type {
  TodoItem, TurnBlock, SubagentBlock, SubagentEntry, ChatEntry,
  ChatState, RunState, RunEventPayload, RunEventType, SteerMessage,
} from './types';
export {
  selectEntryIds, selectEntryById, selectSubagentById,
  selectPendingApprovalCount, selectActivePendingApproval,
} from './selectors';
export { entriesToMessages, entriesToEventLog, stringifyResult, getFullMessages, getFullEventLog } from './utils';

// ── Initial state ────────────────────────────────────────────────────

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
  allEventLog: [],
  eventLogBySession: {},
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

function rebuildEntries(state: ChatState, eventLog: EventLogEntry[]) {
  if (!state.allPrompts || state.allPrompts.length === 0) {
    state.entries = [];
    return;
  }

  // 1. Rebuild prompt status map by turn_index for zombie detection.
  const promptStatusByTurn = new Map<number, string>();
  for (const p of state.allPrompts) {
    promptStatusByTurn.set(p.turn_index, p.status);
  }

  // 2. Group the event log by turn_index
  const eventsByTurn = new Map<number, EventLogEntry[]>();
  if (eventLog && Array.isArray(eventLog)) {
    for (const ev of eventLog) {
      const arr = eventsByTurn.get(ev.turn_index);
      if (arr) arr.push(ev);
      else eventsByTurn.set(ev.turn_index, [ev]);
    }
  }

  // 3. Make sure all subagents are populated in state.subagents
  if (eventLog && Array.isArray(eventLog)) {
    for (const ev of eventLog) {
      if (ev.event_type === 'subagent' && ev.payload) {
        const payload = ev.payload as Record<string, unknown>;
        const subId = payload.id as string | undefined;
        if (subId) {
          state.subagents[subId] = payload as unknown as SubagentEntry;
        }
      }
    }
  }

  const toPayload = (ev: EventLogEntry): Record<string, unknown> =>
    ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)
      ? (ev.payload as Record<string, unknown>)
      : {};

  // 4. Get the visible prompts slice (last visiblePromptsCount prompts)
  const count = state.visiblePromptsCount;
  const startIdx = Math.max(0, state.allPrompts.length - count);
  const visiblePrompts = state.allPrompts.slice(startIdx);

  const newEntries: ChatEntry[] = [];

  for (const prompt of visiblePrompts) {
    // A. Push the user entry
    newEntries.push({
      id: `user-${prompt.id}`,
      type: 'user',
      text: prompt.user_message,
      model: prompt.model,
    });

    // B. Reconstruct the turn entry if it has messages/events or is running
    const turnIdx = prompt.turn_index;
    const blocks: TurnBlock[] = [];
    const turnEvents = eventsByTurn.get(turnIdx) ?? [];

    for (const ev of turnEvents) {
      if (
        ev.event_type !== 'tool_call' &&
        ev.event_type !== 'subagent' &&
        ev.event_type !== 'thinking' &&
        ev.event_type !== 'assistant'
      ) {
        continue;
      }
      const payload = toPayload(ev);
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
        const payload = ev.payload as Record<string, unknown>;
        blocks.push({
          type: 'thinking',
          text: (payload.text as string) ?? '',
          isStreaming: false,
          startTime: payload.startTime as number | undefined,
          endTime: payload.endTime as number | undefined,
        });
      } else if (ev.event_type === 'assistant') {
        const payload = ev.payload as Record<string, unknown>;
        blocks.push({
          type: 'assistant',
          text: (payload.text as string) ?? '',
          isStreaming: false,
        });
      }
    }

    // Parse prompt assistant messages
    const asstMsg = prompt.messages.find((m) => m.role === 'assistant');
    if (asstMsg) {
      const hasThinkTag = asstMsg.content?.match(/<think>([\s\S]*?)<\/think>/);
      if (hasThinkTag) {
        const thinkContent = hasThinkTag[1];
        const restContent = asstMsg.content!.replace(/<think>[\s\S]*?<\/think>/, '').trim();
        const thinkBlock = blocks.find((b) => b.type === 'thinking');
        if (thinkBlock) {
          thinkBlock.text = thinkContent;
        } else {
          blocks.push({ type: 'thinking', text: thinkContent, isStreaming: false });
        }
        const asstBlock = blocks.find((b) => b.type === 'assistant');
        if (asstBlock) {
          asstBlock.text = restContent;
        } else if (restContent) {
          blocks.push({ type: 'assistant', text: restContent, isStreaming: false });
        }
      } else {
        const assistantBlock = blocks.find((b) => b.type === 'assistant');
        if (assistantBlock && asstMsg.content) {
          assistantBlock.text = asstMsg.content;
        } else if (!assistantBlock && asstMsg.content) {
          blocks.push({ type: 'assistant', text: asstMsg.content, isStreaming: false });
        }
      }
    }

    let startTime: number | undefined = undefined;
    let endTime: number | undefined = undefined;
    let cacheHitRate: number | undefined = undefined;
    let turnIds: string[] | undefined = undefined;
    for (const ev of turnEvents) {
      if (ev.event_type === 'turn_meta' && ev.payload) {
        const meta = ev.payload as Record<string, unknown>;
        startTime = meta.startTime as number | undefined;
        endTime = meta.endTime as number | undefined;
        cacheHitRate = meta.cacheHitRate as number | undefined;
        turnIds = meta.turnIds as string[] | undefined;
        break;
      }
    }

    // Fallback to database prompt timestamps if event_log's turn_meta is missing/incomplete
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
        endTime = startTime + 5000; // default to 5s execution
      }
      if (!endTime) {
        endTime = Date.now();
      }
    }

    let subagentIds: string[] | undefined = undefined;
    for (const ev of turnEvents) {
      if (ev.event_type !== 'subagent') continue;
      const payload = toPayload(ev);
      const subId = payload.id as string | undefined;
      if (subId) {
        if (!subagentIds) subagentIds = [];
        subagentIds.push(subId);
      }
    }

    const pStatus = promptStatusByTurn.get(turnIdx);
    newEntries.push({
      id: `turn-${turnIdx}-${prompt.id}`,
      type: 'turn',
      turnIndex: turnIdx,
      blocks,
      subagentIds,
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
      state.eventLogBySession[sessionId] = state.allEventLog;
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
        state.allEventLog = state.eventLogBySession[sessionId] ?? [];
      } else {
        state.entries = [];
        state.isProcessing = false;
        state.subagents = {};
        state.runId = null;
        state.todo = [];
        state.steerQueue = [];
        state.allPrompts = [];
        state.visiblePromptsCount = 1;
        state.allEventLog = [];
      }
      state.viewingSubagentPath = [];
      state._resumedFromBackend = false;
      state.btwEntries = [];
      state.learnEntries = [];
      state.goal = null;
      state.goalCompleted = false;
    },
    loadMorePrompts: (state) => {
      const { eventLog } = entriesToEventLog(state.entries, state.subagents);
      const visibleTurnIndexes = new Set(
        state.entries
          .filter((e) => e.type === 'turn')
          .map((e) => e.turnIndex)
          .filter((x) => x !== undefined) as number[]
      );
      const filteredOldEventLog = state.allEventLog.filter(
        (ev) => !visibleTurnIndexes.has(ev.turn_index)
      );
      state.allEventLog = [...filteredOldEventLog, ...(eventLog as EventLogEntry[])];

      state.visiblePromptsCount += 2;
      rebuildEntries(state, state.allEventLog);
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
        user_message: action.payload.text,
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
    },
    agentEventsBatch: (state, action: PayloadAction<Array<string | Record<string, unknown>>>) => {
      for (const payload of action.payload) {
        processSingleEvent(state, payload);
      }
    },
    toolApprovalResponded: (state, action: PayloadAction<{ promptId: string; approved: boolean }>) => {
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
      state.entries.splice(idx);
      state.entries.push({
        id: `user-${Date.now()}`,
        type: 'user',
        text: userText,
        model: state.entries[idx]?.model,
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
    },
    steerMessageCancelled: (state, action: PayloadAction<string>) => {
      const steerId = action.payload;
      state.steerQueue = state.steerQueue.filter((s) => s.steerId !== steerId);
      state.entries = state.entries.filter(
        (e) => !(e.type === 'user' && e.isSteer && e.steerId === steerId)
      );
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
      const { event_log, prompts } = action.payload;
      state.entries = [];
      state.isProcessing = false;

      state.allPrompts = prompts ?? [];
      state.allEventLog = event_log ?? [];
      // Initially render only the last 2 prompts (or 1 if only 1 exists)
      state.visiblePromptsCount = Math.min(2, state.allPrompts.length);

      rebuildEntries(state, state.allEventLog);
      state._resumedFromBackend = true;
    });
    builder.addCase(deleteSession.fulfilled, (state, action) => {
      const { sessionId } = action.payload;
      delete state.entriesBySession[sessionId];
      delete state.processingBySession[sessionId];
      delete state.subagentsBySession[sessionId];
      delete state.runIdBySession[sessionId];
      delete state.allPromptsBySession[sessionId];
      delete state.visiblePromptsCountBySession[sessionId];
      delete state.eventLogBySession[sessionId];
      if (state.todoBySession) {
        delete state.todoBySession[sessionId];
      }
      if (state.steerQueueBySession) {
        delete state.steerQueueBySession[sessionId];
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
