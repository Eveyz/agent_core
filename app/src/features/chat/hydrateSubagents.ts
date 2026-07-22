import type { SubagentEntry, TurnBlock } from './types';
import { stripContextStatus } from '../../utils/chatUtils';

const SUBAGENT_TOOL_NAMES = new Set(['subagent', 'subagents', 'invoke_subagent']);

type SpawnSpec = {
  roleName: string;
  task: string;
};

function isSpawnTool(
  block: TurnBlock,
): block is Extract<TurnBlock, { type: 'tool' }> & { name: string } {
  return block.type === 'tool' && SUBAGENT_TOOL_NAMES.has(block.name);
}

function parseArgs(raw: unknown): Record<string, unknown> {
  if (typeof raw === 'string') {
    try {
      const parsed = JSON.parse(raw);
      return parsed && typeof parsed === 'object' ? (parsed as Record<string, unknown>) : {};
    } catch {
      return {};
    }
  }
  if (raw && typeof raw === 'object') return raw as Record<string, unknown>;
  return {};
}

/** Extract spawn specs from a subagent / subagents tool call. */
export function spawnSpecsFromArgs(args: unknown): SpawnSpec[] {
  const obj = parseArgs(args);
  if (Array.isArray(obj.tasks)) {
    return obj.tasks
      .map((item) => {
        if (!item || typeof item !== 'object') return null;
        const taskObj = item as Record<string, unknown>;
        const roleName = typeof taskObj.id === 'string' ? taskObj.id : 'Subagent';
        const task = typeof taskObj.task === 'string' ? taskObj.task : '';
        return { roleName, task };
      })
      .filter((s): s is SpawnSpec => !!s);
  }
  const roleName = typeof obj.id === 'string' ? obj.id : 'Subagent';
  const task = typeof obj.task === 'string' ? obj.task : '';
  return [{ roleName, task }];
}

function extractRuntimeId(text: string): string | undefined {
  const match = text.match(/runtime_id:\s*([0-9a-f-]{36})/i);
  return match?.[1];
}

/** Pull the per-role section out of a batch subagents tool result. */
export function extractBatchSection(result: string, roleName: string): string {
  // [1] weather-shanghai — success\n...
  const escaped = roleName.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
  const re = new RegExp(
    `\\[\\d+\\]\\s+${escaped}\\s+[—\\-][\\s\\S]*?(?=\\n\\[\\d+\\]\\s+|\\n=== End batch results ===|$)`,
    'i',
  );
  const match = result.match(re);
  return match ? match[0].trim() : result;
}

function summaryFromHandoff(text: string): string {
  let body = text;
  // Drop handoff header block when present
  const schemaIdx = body.indexOf('[subagent-handoff/v1]');
  if (schemaIdx >= 0) {
    const afterHeader = body.slice(schemaIdx);
    const blank = afterHeader.search(/\n\n/);
    body = blank >= 0 ? afterHeader.slice(blank + 2) : afterHeader;
  }
  // Drop batch index line
  body = body.replace(/^\[\d+\]\s+\S+\s+[—\-]\s+\w+\s*/i, '');
  return stripContextStatus(body).trim();
}

/**
 * After resume rebuilds turn blocks from the canonical transcript, re-attach
 * `subagent_ref` blocks and stub `SubagentEntry` records from spawn tool
 * args + results. Live Redux entries are gone after refresh; child sessions
 * exist in SQLite but are not yet loaded into the UI map.
 */
export function hydrateSubagentsFromBlocks(
  blocks: TurnBlock[],
  subagentsMap: Record<string, SubagentEntry>,
): { blocks: TurnBlock[]; subagentIds: string[] } {
  const nextBlocks = [...blocks];
  const existingRefIds = new Set(
    nextBlocks
      .filter((b): b is Extract<TurnBlock, { type: 'subagent_ref' }> => b.type === 'subagent_ref')
      .map((b) => b.subagent_id),
  );
  const subagentIds: string[] = [...existingRefIds];

  for (const block of blocks) {
    if (!isSpawnTool(block)) continue;
    const specs = spawnSpecsFromArgs(block.args);
    if (specs.length === 0) continue;

    const resultText = typeof block.result === 'string' ? block.result : '';
    const isBatch = specs.length > 1 || block.name === 'subagents';

    for (const spec of specs) {
      const section = isBatch ? extractBatchSection(resultText, spec.roleName) : resultText;
      const runtimeId = extractRuntimeId(section);
      const id = runtimeId || (block.call_id ? `${block.call_id}:${spec.roleName}` : spec.roleName);

      if (!existingRefIds.has(id)) {
        nextBlocks.push({
          type: 'subagent_ref',
          subagent_id: id,
          parent_call_id: block.call_id,
        });
        existingRefIds.add(id);
        subagentIds.push(id);
      } else if (!subagentIds.includes(id)) {
        subagentIds.push(id);
      }

      if (!subagentsMap[id]) {
        const summary = summaryFromHandoff(section);
        const failed =
          /status:\s*failed/i.test(section) ||
          /\bfailed\b/i.test(section.slice(0, 120)) ||
          block.is_error;
        subagentsMap[id] = {
          id,
          role_name: spec.roleName,
          task: spec.task,
          status: failed ? 'error' : 'done',
          blocks: summary
            ? [{ type: 'assistant', text: summary, isStreaming: false }]
            : [],
          startTime: block.startTime ?? Date.now(),
          endTime: block.endTime ?? Date.now(),
        };
      } else if (!subagentsMap[id].role_name) {
        subagentsMap[id].role_name = spec.roleName;
      }
    }
  }

  return { blocks: nextBlocks, subagentIds };
}
