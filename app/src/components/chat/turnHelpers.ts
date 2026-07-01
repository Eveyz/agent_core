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

export function generateSmartToolsLabel(toolBlocks: TurnBlock[], isStreaming: boolean): string {
  const regularTools = toolBlocks.filter(b => b.type === 'tool' && !isSubagentTool(b));
  if (regularTools.length === 0) return isStreaming ? 'Calling tool...' : 'Called tool';

  const isAnyToolActive = regularTools.some(b => b.type === 'tool' && b.active);
  const isExecuting = isStreaming || isAnyToolActive;

  const counts: Record<string, number> = {};
  for (const t of regularTools) {
    if (t.type === 'tool') {
      counts[t.name] = (counts[t.name] || 0) + 1;
    }
  }

  const parts: string[] = [];
  const editCount = counts['edit'] || 0;
  if (editCount > 0) {
    parts.push(isExecuting ? `Editing ${editCount} file${editCount > 1 ? 's' : ''}` : `Edited ${editCount} file${editCount > 1 ? 's' : ''}`);
  }

  const createCount = (counts['write_file'] || 0) + (counts['write_to_file'] || 0);
  if (createCount > 0) {
    parts.push(isExecuting ? `Creating ${createCount} file${createCount > 1 ? 's' : ''}` : `Created ${createCount} file${createCount > 1 ? 's' : ''}`);
  }

  const readCount = counts['read_file'] || 0;
  if (readCount > 0) {
    parts.push(isExecuting ? `Reading ${readCount} file${readCount > 1 ? 's' : ''}` : `Read ${readCount} file${readCount > 1 ? 's' : ''}`);
  }

  const searchCount = (counts['tavily_search'] || 0) + (counts['webfetch'] || 0);
  if (searchCount > 0) {
    parts.push(isExecuting ? `Searching ${searchCount} quer${searchCount > 1 ? 'ies' : 'y'}` : `Searched ${searchCount} quer${searchCount > 1 ? 'ies' : 'y'}`);
  }

  const bashCount = counts['bash'] || 0;
  if (bashCount > 0) {
    parts.push(isExecuting ? `Running ${bashCount} command${bashCount > 1 ? 's' : ''}` : `Ran ${bashCount} command${bashCount > 1 ? 's' : ''}`);
  }

  const grepCount = (counts['grep_search'] || 0) + (counts['grep'] || 0) + (counts['glob_search'] || 0) + (counts['glob'] || 0);
  if (grepCount > 0) {
    parts.push(isExecuting ? `Searching ${grepCount} pattern${grepCount > 1 ? 's' : ''}` : `Searched ${grepCount} pattern${grepCount > 1 ? 's' : ''}`);
  }

  const memorySearchCount = (counts['archival_memory_search'] || 0) + (counts['conversation_search'] || 0) + (counts['conversation_search_date'] || 0);
  if (memorySearchCount > 0) {
    parts.push(isExecuting ? `Searching ${memorySearchCount} memor${memorySearchCount > 1 ? 'ies' : 'y'}` : `Searched ${memorySearchCount} memor${memorySearchCount > 1 ? 'ies' : 'y'}`);
  }

  let taskCount = 0;
  let otherCount = 0;
  for (const [name, count] of Object.entries(counts)) {
    if (name.startsWith('todo_')) {
      taskCount += count;
    } else if (!['edit', 'write_file', 'write_to_file', 'read_file', 'tavily_search', 'webfetch', 'bash', 'grep_search', 'grep', 'glob_search', 'glob', 'archival_memory_search', 'conversation_search', 'conversation_search_date'].includes(name)) {
      otherCount += count;
    }
  }

  if (taskCount > 0) {
    parts.push(isExecuting ? `Updating ${taskCount} task${taskCount > 1 ? 's' : ''}` : `Updated ${taskCount} task${taskCount > 1 ? 's' : ''}`);
  }

  if (otherCount > 0 || parts.length === 0) {
    if (parts.length === 0) {
      return isExecuting ? `Calling ${otherCount} tool${otherCount > 1 ? 's' : ''}...` : `Called ${otherCount} tool${otherCount > 1 ? 's' : ''}`;
    } else {
      parts.push(isExecuting ? `calling ${otherCount} other tool${otherCount > 1 ? 's' : ''}` : `called ${otherCount} other tool${otherCount > 1 ? 's' : ''}`);
    }
  }

  let label = parts.join(', ');
  // Capitalize first letter
  label = label.charAt(0).toUpperCase() + label.slice(1);
  if (isExecuting) label += '...';
  
  return label;
}
