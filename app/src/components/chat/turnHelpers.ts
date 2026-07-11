import type { TurnBlock } from '../../features/chat/chatSlice';

export type ThinkingBlock = Extract<TurnBlock, { type: 'thinking' }>;
export type AssistantBlock = Extract<TurnBlock, { type: 'assistant' }>;
export type ApprovalBlock = Extract<TurnBlock, { type: 'approval' }>;
export type ClarificationBlock = Extract<TurnBlock, { type: 'clarification' }>;
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

  // Trailing model text after tool_calls inserts an assistant block between the
  // spawn tool and its subagent_ref, splitting them across iterations. Reattach
  // orphaned refs to the iteration that owns the parent spawn tool.
  const iterations = items.filter(
    (i): i is { type: 'iteration'; data: TurnIteration } => i.type === 'iteration'
  );
  for (const iter of iterations) {
    const orphanRefs = iter.data.toolBlocks.filter(isSubagentRefBlock);
    if (orphanRefs.length === 0) continue;
    if (iter.data.toolBlocks.some(isSubagentTool)) continue;

    for (const ref of orphanRefs) {
      const home =
        iterations.find((i) =>
          i.data.toolBlocks.some(
            (b) => isSubagentTool(b) && !!b.call_id && b.call_id === ref.parent_call_id
          )
        ) ?? iterations.find((i) => i.data.toolBlocks.some(isSubagentTool));
      if (home && home !== iter) {
        iter.data.toolBlocks = iter.data.toolBlocks.filter((b) => b !== ref);
        home.data.toolBlocks.push(ref);
      }
    }
  }

  const cleaned = items.filter((i) => {
    if (i.type !== 'iteration') return true;
    const hasThinking = !!i.data.thinkingBlock?.text?.trim();
    return hasThinking || i.data.toolBlocks.length > 0;
  });

  const lastIter = cleaned.slice().reverse().find((i) => i.type === 'iteration');
  if (lastIter && lastIter.type === 'iteration') {
    lastIter.data.isLast = true;
  }

  return cleaned;
}

/** Progress narrations that are punctuation-only (e.g. stray "." after tool_calls). */
export function isTrivialAssistantText(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return true;
  return /^[.\u00B7\u2026…\s]+$/.test(trimmed);
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

export function generateSmartToolsLabel(
  toolBlocks: TurnBlock[],
  isStreaming: boolean,
  t: (key: string, options?: any) => string
): string {
  const regularTools = toolBlocks.filter(b => b.type === 'tool' && !isSubagentTool(b));
  if (regularTools.length === 0) return isStreaming ? t('chat.tools.labels.calling') : t('chat.tools.labels.called');

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
    const key = isExecuting ? 'editing' : 'edited';
    const suffix = editCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: editCount }));
  }

  const createCount = (counts['write_file'] || 0) + (counts['write_to_file'] || 0);
  if (createCount > 0) {
    const key = isExecuting ? 'creating' : 'created';
    const suffix = createCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: createCount }));
  }

  const readCount = counts['read_file'] || 0;
  if (readCount > 0) {
    const key = isExecuting ? 'reading' : 'read';
    const suffix = readCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: readCount }));
  }

  const searchCount = (counts['tavily_search'] || 0) + (counts['webfetch'] || 0);
  if (searchCount > 0) {
    const key = isExecuting ? 'searchingQueries' : 'searchedQueries';
    const suffix = searchCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: searchCount }));
  }

  const bashCount = counts['bash'] || 0;
  if (bashCount > 0) {
    const key = isExecuting ? 'runningCommands' : 'ranCommands';
    const suffix = bashCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: bashCount }));
  }

  const grepCount = (counts['grep_search'] || 0) + (counts['grep'] || 0) + (counts['glob_search'] || 0) + (counts['glob'] || 0);
  if (grepCount > 0) {
    const key = isExecuting ? 'searchingPatterns' : 'searchedPatterns';
    const suffix = grepCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: grepCount }));
  }

  const memorySearchCount = (counts['archival_memory_search'] || 0) + (counts['conversation_search'] || 0) + (counts['conversation_search_date'] || 0);
  if (memorySearchCount > 0) {
    const key = isExecuting ? 'searchingMemories' : 'searchedMemories';
    const suffix = memorySearchCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: memorySearchCount }));
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
    const key = isExecuting ? 'updatingTasks' : 'updatedTasks';
    const suffix = taskCount > 1 ? '_plural' : '';
    parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: taskCount }));
  }

  if (otherCount > 0 || parts.length === 0) {
    if (parts.length === 0) {
      const suffix = otherCount > 1 ? '_plural' : '';
      return isExecuting 
        ? t(`chat.tools.labels.calling${suffix}`, { count: otherCount }) 
        : t(`chat.tools.labels.called${suffix}`, { count: otherCount });
    } else {
      const key = isExecuting ? 'callingOthers' : 'calledOthers';
      const suffix = otherCount > 1 ? '_plural' : '';
      parts.push(t(`chat.tools.labels.${key}${suffix}`, { count: otherCount }));
    }
  }

  let label = parts.join(', ');
  // Capitalize first letter
  label = label.charAt(0).toUpperCase() + label.slice(1);
  if (isExecuting && !label.endsWith('...')) label += '...';
  
  return label;
}
