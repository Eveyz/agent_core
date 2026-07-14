import { createSelector } from '@reduxjs/toolkit';
import type { ChatState, ChatEntry, SubagentEntry, TodoItem, BtwEntry } from './types';
import type { ApprovalBlock } from '../../components/chat/turnHelpers';

const EMPTY_ENTRIES: ChatEntry[] = [];
const EMPTY_IDS: string[] = [];
const EMPTY_TODOS: TodoItem[] = [];
const EMPTY_SUBAGENTS: Record<string, SubagentEntry> = {};
const EMPTY_PATH: { id: string; name: string }[] = [];
const EMPTY_BTW: BtwEntry[] = [];
const entryIndexes = new WeakMap<ChatEntry[], Map<string, ChatEntry>>();


type Rootish = {
  chat: ChatState;
  project: { activeSessionId: string | null };
};

function sid(state: Rootish): string | null {
  return state.project.activeSessionId;
}

export const selectActiveSessionEntries = createSelector(
  [(state: Rootish) => state.chat.entries, sid],
  (entries, sessionId) => (sessionId ? entries[sessionId] ?? EMPTY_ENTRIES : EMPTY_ENTRIES),
);

export const selectActiveSessionTodos = createSelector(
  [(state: Rootish) => state.chat.todo, sid],
  (todo, sessionId) => (sessionId ? todo[sessionId] ?? EMPTY_TODOS : EMPTY_TODOS),
);

export const selectEntryIds = createSelector(
  [selectActiveSessionEntries],
  (entries) => (entries.length > 0 ? entries.map((e) => e.id) : EMPTY_IDS),
);

export function selectEntryById(state: Rootish, entryId: string): ChatEntry | undefined {
  const sessionId = state.project.activeSessionId;
  if (!sessionId) return undefined;
  const list = state.chat.entries[sessionId];
  if (!list) return undefined;
  let index = entryIndexes.get(list);
  if (!index) {
    index = new Map(list.map((entry) => [entry.id, entry]));
    entryIndexes.set(list, index);
  }
  return index.get(entryId);
}

export function selectSubagentById(state: Rootish, subagentId: string): SubagentEntry | undefined {
  const sessionId = state.project.activeSessionId;
  if (!sessionId) return undefined;
  const subs = state.chat.subagents[sessionId];
  return subs?.[subagentId];
}

export const selectViewingSubagentPath = createSelector(
  [(state: Rootish) => state.chat.viewingSubagentPath, sid],
  (paths, sessionId) => (sessionId ? paths[sessionId] ?? EMPTY_PATH : EMPTY_PATH),
);

export const selectActiveBtwEntries = createSelector(
  [(state: Rootish) => state.chat.btwEntries, sid],
  (btw, sessionId) => (sessionId ? btw[sessionId] ?? EMPTY_BTW : EMPTY_BTW),
);



export const selectIsResumingActive = createSelector(
  [(state: Rootish) => state.chat.isResuming, sid],
  (resuming, sessionId) => (sessionId ? !!resuming[sessionId] : false),
);

export const selectPendingApprovalCount = createSelector(
  [selectActiveSessionEntries, (state: Rootish) => state.chat.subagents, sid],
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

export const selectHasActivePendingApproval = createSelector(
  [selectPendingApprovalCount],
  (count) => count > 0,
);

/** Equality for approval overlay — ignore Immer identity churn when content is unchanged. */
export function pendingApprovalEqual(
  a: ApprovalBlock | null,
  b: ApprovalBlock | null,
): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.prompt_id === b.prompt_id &&
    a.status === b.status &&
    a.tool_name === b.tool_name &&
    a.danger_level === b.danger_level &&
    a.explanation === b.explanation
  );
}

export const selectActivePendingApproval = createSelector(
  [selectActiveSessionEntries, (state: Rootish) => state.chat.subagents, sid],
  (entries, subagentsMap, sessionId): ApprovalBlock | null => {
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
          return b;
        }
      }
    }
    return null;
  }
);

export type ClarificationBlock = Extract<import('./types').TurnBlock, { type: 'clarification' }>;

export const selectHasActivePendingClarification = createSelector(
  [selectActiveSessionEntries, sid],
  (entries, sessionId) => {
    if (!sessionId) return false;
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const b of entry.blocks) {
        if (b.type === 'clarification' && b.status === 'pending') return true;
      }
    }
    return false;
  }
);

export function pendingClarificationEqual(
  a: ClarificationBlock | null,
  b: ClarificationBlock | null,
): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return (
    a.prompt_id === b.prompt_id &&
    a.status === b.status &&
    a.title === b.title &&
    a.questions.length === b.questions.length
  );
}

export const selectActivePendingClarification = createSelector(
  [selectActiveSessionEntries, sid],
  (entries, sessionId): ClarificationBlock | null => {
    if (!sessionId) return null;
    for (const entry of entries) {
      if (entry.type !== 'turn' || !entry.blocks) continue;
      for (const b of entry.blocks) {
        if (b.type === 'clarification' && b.status === 'pending') {
          return b;
        }
      }
    }
    return null;
  }
);
