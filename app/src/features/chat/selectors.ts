import { createSelector } from '@reduxjs/toolkit';
import type { ChatState, ChatEntry, SubagentEntry } from './types';

export const selectEntryIds = createSelector(
  [(state: { chat: ChatState }) => state.chat.entries],
  (entries: ChatEntry[]) => entries.map((e) => e.id)
);

export function selectEntryById(state: { chat: ChatState }, entryId: string): ChatEntry | undefined {
  return state.chat.entries.find((e) => e.id === entryId);
}

export function selectSubagentById(state: { chat: ChatState }, subagentId: string): SubagentEntry | undefined {
  return state.chat.subagents[subagentId];
}

export const selectPendingApprovalCount = createSelector(
  [
    (state: { chat: ChatState }) => state.chat.entries,
    (state: { chat: ChatState }) => state.chat.subagents,
  ],
  (entries: ChatEntry[], subagents: Record<string, SubagentEntry>) => {
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
  [
    (state: { chat: ChatState }) => state.chat.entries,
    (state: { chat: ChatState }) => state.chat.subagents,
  ],
  (entries: ChatEntry[], subagents: Record<string, SubagentEntry>) => {
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
          return {
            type: 'approval',
            prompt_id: b.prompt_id ?? '',
            tool_name: b.tool_name ?? '',
            tool_input: b.tool_input,
            danger_level: b.danger_level ?? '',
            explanation: b.explanation ?? '',
            status: b.status ?? 'pending',
          } as const;
        }
      }
    }
    return null;
  }
);

