import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';
import { resumeSession } from '../project/projectSlice';

import type {
  TurnBlock, SubagentEntry, ChatState, RunState, EventLogEntry,
} from './types';
import { processSingleEvent, stopDanglingSubagents } from './eventHandlers';

// ── Re-export types, selectors, and utils for backward compatibility ─
export type {
  TodoItem, TurnBlock, SubagentBlock, SubagentEntry, ChatEntry,
  ChatState, RunState, RunEventPayload, RunEventType,
} from './types';
export {
  selectEntryIds, selectEntryById, selectSubagentById,
  selectPendingApprovalCount,
} from './selectors';
export { entriesToMessages, entriesToEventLog, stringifyResult } from './utils';

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
  activeSessionId: null,
  _resumedFromBackend: false,
  _thinkBuffers: {},
  _pendingGap: null,
  todo: [],
  skillsCache: null,
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
      state.todo = [];
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
  },
  extraReducers: (builder) => {
    builder.addCase(resumeSession.fulfilled, (state, action) => {
      if (state.entries.length > 0) return;
      const { messages, event_log } = action.payload;
      state.entries = [];
      state.isProcessing = false;

      // Group the event log by turn_index once, instead of scanning the
      // full array three times per assistant turn (O(turns x events)).
      const eventsByTurn = new Map<number, EventLogEntry[]>();
      if (event_log && Array.isArray(event_log)) {
        for (const ev of event_log) {
          const arr = eventsByTurn.get(ev.turn_index);
          if (arr) arr.push(ev);
          else eventsByTurn.set(ev.turn_index, [ev]);
        }
      }

      const toPayload = (ev: EventLogEntry): Record<string, unknown> =>
        ev.payload && typeof ev.payload === 'object' && !Array.isArray(ev.payload)
          ? (ev.payload as Record<string, unknown>)
          : {};

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

          // Prefer full message content over event_log (which may be truncated).
          // The event_log assistant block serves as a fallback — if msg.content
          // is available, use it for a complete restore.
          const assistantBlock = blocks.find((b) => b.type === 'assistant');
          if (assistantBlock && msg.content) {
            assistantBlock.text = msg.content;
          } else if (!assistantBlock) {
            blocks.push({ type: 'assistant', text: msg.content, isStreaming: false });
          }

          let startTime: number | undefined = undefined;
          let endTime: number | undefined = undefined;
          for (const ev of turnEvents) {
            if (ev.event_type === 'turn_meta' && ev.payload) {
              const meta = ev.payload as Record<string, unknown>;
              startTime = meta.startTime as number | undefined;
              endTime = meta.endTime as number | undefined;
              break;
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
              state.subagents[subId] = payload as unknown as SubagentEntry;
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
} = chatSlice.actions;
export default chatSlice.reducer;
