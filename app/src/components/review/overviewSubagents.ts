import type {
  ChatEntry,
  FrontendPrompt,
  SubagentBlock,
  SubagentEntry,
  TurnBlock,
} from '../../features/chat/types';
import { stripContextStatus } from '../../utils/chatUtils';

export type PromptSubagentGroup = {
  promptId: string;
  turnIndex: number;
  userPreview: string;
  subagents: SubagentEntry[];
};

const PREVIEW_MAX = 100;

export function truncateText(text: string, max = PREVIEW_MAX): string {
  const trimmed = text.replace(/\s+/g, ' ').trim();
  if (trimmed.length <= max) return trimmed;
  return `${trimmed.slice(0, max - 1)}…`;
}

export function getLastAssistantText(blocks: SubagentBlock[]): string {
  for (let i = blocks.length - 1; i >= 0; i--) {
    const block = blocks[i];
    if (block.type === 'assistant' && block.text?.trim()) {
      return stripContextStatus(block.text).trim();
    }
  }
  return '';
}

export function getToolNames(blocks: SubagentBlock[]): string[] {
  const names: string[] = [];
  const seen = new Set<string>();
  for (const block of blocks) {
    if (block.type === 'tool' && block.name && !seen.has(block.name)) {
      seen.add(block.name);
      names.push(block.name);
    }
  }
  return names;
}

function collectIdsFromBlocks(blocks: unknown): string[] {
  if (!Array.isArray(blocks)) return [];
  const ids: string[] = [];
  for (const block of blocks as TurnBlock[]) {
    if (block?.type === 'subagent_ref' && block.subagent_id) {
      ids.push(block.subagent_id);
    }
  }
  return ids;
}

function collectIdsFromPrompt(prompt: FrontendPrompt): string[] {
  const ids: string[] = [];
  const seen = new Set<string>();

  const push = (id: string) => {
    if (!id || seen.has(id)) return;
    seen.add(id);
    ids.push(id);
  };

  for (const msg of prompt.messages) {
    if (msg.role !== 'assistant' || !msg.metadata) continue;
    for (const id of collectIdsFromBlocks(msg.metadata.blocks)) {
      push(id);
    }
    const subMap = msg.metadata.subagents;
    if (subMap && typeof subMap === 'object') {
      for (const id of Object.keys(subMap as Record<string, unknown>)) {
        push(id);
      }
    }
  }

  return ids;
}

function userPreviewFromPrompt(prompt: FrontendPrompt): string {
  const userMsg = prompt.messages.find((m) => m.role === 'user');
  const text = typeof userMsg?.content === 'string' ? userMsg.content : '';
  return truncateText(text || 'Untitled prompt', 60);
}

function resolveSubagents(
  ids: string[],
  subagentsMap: Record<string, SubagentEntry>,
): SubagentEntry[] {
  const result: SubagentEntry[] = [];
  const seen = new Set<string>();
  for (const id of ids) {
    if (seen.has(id)) continue;
    seen.add(id);
    const entry = subagentsMap[id];
    if (entry) result.push(entry);
  }
  return result;
}

/**
 * Group session subagents under the prompt that spawned them.
 * Uses allPrompts for durable history, then merges live turn `subagentIds`
 * so mid-run spawns appear before metadata is persisted.
 */
export function groupSubagentsByPrompt(
  prompts: FrontendPrompt[],
  subagentsMap: Record<string, SubagentEntry>,
  liveEntries: ChatEntry[],
): PromptSubagentGroup[] {
  const liveIdsByPrompt = new Map<string, string[]>();
  for (const entry of liveEntries) {
    if (entry.type !== 'turn' || !entry.promptId || !entry.subagentIds?.length) continue;
    liveIdsByPrompt.set(entry.promptId, entry.subagentIds);
  }

  const groups: PromptSubagentGroup[] = [];
  const claimed = new Set<string>();

  for (const prompt of prompts) {
    const fromMeta = collectIdsFromPrompt(prompt);
    const fromLive = liveIdsByPrompt.get(prompt.id) ?? [];
    const mergedIds: string[] = [];
    const seen = new Set<string>();
    for (const id of [...fromMeta, ...fromLive]) {
      if (seen.has(id)) continue;
      seen.add(id);
      mergedIds.push(id);
    }

    const subagents = resolveSubagents(mergedIds, subagentsMap);
    for (const sa of subagents) claimed.add(sa.id);
    if (subagents.length === 0) continue;

    groups.push({
      promptId: prompt.id,
      turnIndex: prompt.turn_index,
      userPreview: userPreviewFromPrompt(prompt),
      subagents,
    });
  }

  // Orphan live subagents (e.g. turn not yet mirrored into allPrompts)
  for (const entry of liveEntries) {
    if (entry.type !== 'turn' || !entry.promptId || !entry.subagentIds?.length) continue;
    const alreadyGrouped = groups.some((g) => g.promptId === entry.promptId);
    if (alreadyGrouped) continue;

    const orphans = resolveSubagents(
      entry.subagentIds.filter((id) => !claimed.has(id)),
      subagentsMap,
    );
    if (orphans.length === 0) continue;
    for (const sa of orphans) claimed.add(sa.id);

    const userEntry = liveEntries.find(
      (e) => e.type === 'user' && e.promptId === entry.promptId,
    );
    groups.push({
      promptId: entry.promptId,
      turnIndex: entry.turnIndex ?? groups.length,
      userPreview: truncateText(userEntry?.text || 'Untitled prompt', 60),
      subagents: orphans,
    });
  }

  return groups;
}

export function countGroupedSubagents(groups: PromptSubagentGroup[]): number {
  return groups.reduce((n, g) => n + g.subagents.length, 0);
}
