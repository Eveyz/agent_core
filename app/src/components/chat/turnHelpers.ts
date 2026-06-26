import type { TurnBlock } from '../../features/chat/chatSlice';

export type ThinkingBlock = Extract<TurnBlock, { type: 'thinking' }>;
export type AssistantBlock = Extract<TurnBlock, { type: 'assistant' }>;
export type ApprovalBlock = Extract<TurnBlock, { type: 'approval' }>;
export type SubagentRefBlock = Extract<TurnBlock, { type: 'subagent_ref' }>;

const SUBAGENT_TOOL_NAMES = ['subagent', 'subagents', 'invoke_subagent'];

export function isSubagentTool(b: TurnBlock): boolean {
  return b.type === 'tool' && SUBAGENT_TOOL_NAMES.includes(b.name);
}

export function isSubagentRefBlock(b: TurnBlock): b is SubagentRefBlock {
  return b.type === 'subagent_ref';
}

export interface TurnIteration {
  id: string;
  thinkingBlock?: ThinkingBlock;
  toolBlocks: TurnBlock[];
  isLast: boolean;
}

export type TurnRenderItem =
  | { type: 'iteration'; data: TurnIteration }
  | { type: 'assistant'; data: AssistantBlock }
  | { type: 'error'; data: Extract<TurnBlock, { type: 'error' }> };

export function groupBlocksIntoItems(blocks: TurnBlock[]): TurnRenderItem[] {
  const items: TurnRenderItem[] = [];
  let currentIter: TurnIteration | null = null;

  const pushCurrentIter = () => {
    if (currentIter) {
      items.push({ type: 'iteration', data: currentIter });
      currentIter = null;
    }
  };

  blocks.forEach((b, idx) => {
    if (b.type === 'assistant') {
      pushCurrentIter();
      items.push({ type: 'assistant', data: b as AssistantBlock });
      return;
    }

    if (b.type === 'error') {
      pushCurrentIter();
      items.push({ type: 'error', data: b });
      return;
    }

    if (b.type === 'thinking') {
      pushCurrentIter();
      currentIter = { id: `iter-${idx}`, thinkingBlock: b as ThinkingBlock, toolBlocks: [], isLast: false };
    } else {
      if (!currentIter) currentIter = { id: `iter-init-${idx}`, toolBlocks: [], isLast: false };
      currentIter.toolBlocks.push(b);
    }
  });

  pushCurrentIter();

  const lastIter = items.slice().reverse().find(i => i.type === 'iteration');
  if (lastIter && lastIter.type === 'iteration') {
    lastIter.data.isLast = true;
  }

  return items;
}

/** Count subagents from the tool args: single = 1, batch = tasks.length. */
export function countSpawnedAgents(args?: unknown): number {
  if (!args || typeof args !== 'object') return 1;
  const obj = args as Record<string, unknown>;
  if (Array.isArray(obj.tasks)) return obj.tasks.length;
  return 1;
}

/** Extract a filename from a path string. */
export function basename(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

/** Parse a unified diff into side-by-side rows. */
export interface DiffRow {
  oldLineNo: number | null;
  newLineNo: number | null;
  oldText: string;
  newText: string;
  type: 'context' | 'add' | 'del' | 'empty';
}

export function parseUnifiedDiff(diffStr: string): DiffRow[] {
  const lines = diffStr.split('\n');
  const rows: DiffRow[] = [];
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;

  for (const line of lines) {
    if (line.startsWith('@@')) {
      const m = line.match(/@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/);
      if (m) {
        oldLine = parseInt(m[1], 10);
        newLine = parseInt(m[2], 10);
      }
      inHunk = true;
      continue;
    }
    if (!inHunk) continue;
    if (line.startsWith('---') || line.startsWith('+++')) continue;

    if (line.startsWith('+')) {
      rows.push({ oldLineNo: null, newLineNo: newLine++, oldText: '', newText: line.slice(1), type: 'add' });
    } else if (line.startsWith('-')) {
      rows.push({ oldLineNo: oldLine++, newLineNo: null, oldText: line.slice(1), newText: '', type: 'del' });
    } else if (line.startsWith(' ')) {
      rows.push({ oldLineNo: oldLine++, newLineNo: newLine++, oldText: line.slice(1), newText: line.slice(1), type: 'context' });
    } else if (line.startsWith('\\')) {
      // "\ No newline at end of file" — skip
      continue;
    }
  }
  return rows;
}

/** Extract the line-range summary line the backend emits:
 *  "Edited lines 12–18 (3 additions, 2 deletions)" → {start,end,adds,dels}. */
export interface EditSummary {
  start: number;
  end: number;
  additions: number;
  deletions: number;
}

export function parseEditSummary(result: string): EditSummary | null {
  const m = result.match(/Edited lines (\d+)–(\d+) \((\d+) additions?, (\d+) deletions?\)/);
  if (!m) return null;
  return { start: +m[1], end: +m[2], additions: +m[3], deletions: +m[4] };
}
