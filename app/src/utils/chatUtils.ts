import type { RootState } from '../store';
import type { SubagentBlock, TurnBlock } from '../features/chat/chatSlice';

export function getActiveSessionTitle(projectState: RootState['project']): string {
  if (!projectState.activeSessionId || !projectState.activeProjectId) return '';
  const list = projectState.sessions[projectState.activeProjectId] ?? [];
  const s = list.find((s) => s.id === projectState.activeSessionId);
  return s?.title ?? '';
}

/** Subagent blocks are a subset of TurnBlock (no nested subagent_ref). */
export function convertSubagentBlocks(blocks: SubagentBlock[]): TurnBlock[] {
  return blocks.map((b): TurnBlock => b);
}
