import type { RootState } from '../store';
import type { SubagentBlock, TurnBlock } from '../features/chat/chatSlice';

export function getActiveSessionTitle(projectState: RootState['project']): string {
  if (!projectState.activeSessionId || !projectState.activeProjectId) return '';
  const list = projectState.sessions[projectState.activeProjectId] ?? [];
  const s = list.find((s) => s.id === projectState.activeSessionId);
  return s?.title ?? '';
}

/**
 * Strip machine-readable handoff chrome from subagent text before showing it
 * in Overview / detail UI. Keeps the human answer; drops context_status tags
 * and trailing Evidence / Missing / Unresolved / Transcript sections (often
 * empty or only useful to the parent agent).
 */
export function stripContextStatus(text: string): string {
  let out = text.replace(/<context_status>[\s\S]*?<\/context_status>/gi, '');
  // Models sometimes emit a malformed self-closing attribute form instead of a tag pair.
  out = out.replace(/<context_status\s*=\s*\{[\s\S]*?\}\s*>/gi, '');
  out = out.replace(/(?:^|\n)[ \t]*(?:上下文状态|Context status)\s*[:：][ \t]*/gi, '\n');
  // Cut from the first handoff trailer heading through the end.
  out = out.replace(
    /\n{1,2}(?:Evidence|Missing context|Unresolved|Transcript)\s*:[\s\S]*$/i,
    '',
  );
  // Bare trailing label with no body (model echo / empty tool summary).
  out = out.replace(/\n{1,2}Evidence\s*:\s*$/i, '');
  return out.replace(/\n{3,}/g, '\n\n').trim();
}

/** Subagent blocks are a subset of TurnBlock (no nested subagent_ref). */
export function convertSubagentBlocks(blocks: SubagentBlock[]): TurnBlock[] {
  return blocks.map((b): TurnBlock => {
    if (b.type === 'assistant' && b.text) {
      return { ...b, text: stripContextStatus(b.text) };
    }
    return b;
  });
}
