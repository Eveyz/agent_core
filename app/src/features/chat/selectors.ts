import { createSelector } from '@reduxjs/toolkit';
import type { ChatState, ChatEntry, SubagentEntry, TodoItem } from './types';
import type { ApprovalBlock } from '../../components/chat/turnHelpers';

const EMPTY_ENTRIES: ChatEntry[] = [];
const EMPTY_IDS: string[] = [];
const EMPTY_TODOS: TodoItem[] = [];
const EMPTY_SUBAGENTS: Record<string, SubagentEntry> = {};

function sid(state: { chat: ChatState }): string | null {
  return state.chat.activeSessionId;
}

export const selectActiveSessionEntries = createSelector(
  [(state: { chat: ChatState }) => state.chat.entries, sid],
  (entries, sessionId) => (sessionId ? entries[sessionId] ?? EMPTY_ENTRIES : EMPTY_ENTRIES),
);

export const selectActiveSessionTodos = createSelector(
  [(state: { chat: ChatState }) => state.chat.todo, sid],
  (todo, sessionId) => (sessionId ? todo[sessionId] ?? EMPTY_TODOS : EMPTY_TODOS),
);

export const selectEntryIds = createSelector(
  [selectActiveSessionEntries],
  (entries) => (entries.length > 0 ? entries.map((e) => e.id) : EMPTY_IDS),
);

export function selectEntryById(state: { chat: ChatState }, entryId: string): ChatEntry | undefined {
  const sessionId = state.chat.activeSessionId;
  if (!sessionId) return undefined;
  const list = state.chat.entries[sessionId];
  if (!list) return undefined;
  return list.find((e) => e.id === entryId);
}

export function selectSubagentById(state: { chat: ChatState }, subagentId: string): SubagentEntry | undefined {
  const sessionId = state.chat.activeSessionId;
  if (!sessionId) return undefined;
  const subs = state.chat.subagents[sessionId];
  return subs?.[subagentId];
}

export const selectPendingApprovalCount = createSelector(
  [selectActiveSessionEntries, (state: { chat: ChatState }) => state.chat.subagents, sid],
  (entries, subagentsMap, sessionId) => {
    if (!sessionId) return 0;
    const subagents = subagentsMap[sessionId] ?? EMPTY_SUBAGENTS;
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

export const selectActivePendingApproval = createSelector(
  [selectActiveSessionEntries, (state: { chat: ChatState }) => state.chat.subagents, sid],
  (entries, subagentsMap, sessionId) => {
    if (!sessionId) return null;
    const subagents = subagentsMap[sessionId] ?? EMPTY_SUBAGENTS;
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const b of entry.blocks) {
        if (b.type === 'approval' && b.status === 'pending') {
          return b;
        }
      }
    }
    for (const sa of Object.values(subagents)) {
      if (!sa.blocks) continue;
      for (const b of sa.blocks) {
        if (b.type === 'approval' && b.status === 'pending') {
          return b as ApprovalBlock;
        }
      }
    }
    return null;
  }
);
