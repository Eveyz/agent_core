import { describe, expect, it } from 'vitest';
import type { ChatEntry, FrontendPrompt, SubagentEntry } from '../../features/chat/types';
import {
  countGroupedSubagents,
  getLastAssistantText,
  getToolNames,
  groupSubagentsByPrompt,
  truncateText,
} from './overviewSubagents';

function makeSubagent(partial: Partial<SubagentEntry> & { id: string }): SubagentEntry {
  return {
    role_name: partial.role_name ?? 'explore',
    task: partial.task ?? 'do something',
    status: partial.status ?? 'done',
    blocks: partial.blocks ?? [{ type: 'assistant', text: 'final answer here', isStreaming: false }],
    startTime: partial.startTime ?? 1,
    endTime: partial.endTime,
    iterations_used: partial.iterations_used,
    id: partial.id,
  };
}

function makePrompt(
  id: string,
  turnIndex: number,
  userText: string,
  subagentIds: string[],
  withSubMap = false,
): FrontendPrompt {
  const blocks = subagentIds.map((subagent_id) => ({
    type: 'subagent_ref' as const,
    subagent_id,
  }));
  const subagents = withSubMap
    ? Object.fromEntries(subagentIds.map((sid) => [sid, makeSubagent({ id: sid })]))
    : undefined;
  return {
    id,
    session_id: 's1',
    turn_index: turnIndex,
    model: 'test',
    status: 'completed',
    token_usage: {},
    started_at: null,
    ended_at: null,
    created_at: '2026-01-01',
    messages: [
      { role: 'user', content: userText },
      {
        role: 'assistant',
        content: '',
        metadata: {
          blocks,
          ...(subagents ? { subagents } : {}),
        },
      },
    ],
  };
}

describe('overviewSubagents helpers', () => {
  it('getLastAssistantText returns the last non-empty assistant block', () => {
    expect(
      getLastAssistantText([
        { type: 'assistant', text: 'first', isStreaming: false },
        { type: 'tool', call_id: '1', name: 'read', result: 'ok', active: false, is_error: false },
        { type: 'assistant', text: '  final  ', isStreaming: false },
      ]),
    ).toBe('final');
  });

  it('getLastAssistantText strips context_status tags', () => {
    expect(
      getLastAssistantText([
        {
          type: 'assistant',
          text: 'Shenzhen is sunny.\n<context_status>{"sufficient":true,"missing":[],"unresolved":[]}</context_status>',
          isStreaming: false,
        },
      ]),
    ).toBe('Shenzhen is sunny.');
  });

  it('getLastAssistantText strips empty Evidence trailer', () => {
    expect(
      getLastAssistantText([
        {
          type: 'assistant',
          text: '上海小雨，26–34°C。\n\nEvidence:\n',
          isStreaming: false,
        },
      ]),
    ).toBe('上海小雨，26–34°C。');
  });

  it('getToolNames dedupes tool names in order', () => {
    expect(
      getToolNames([
        { type: 'tool', call_id: '1', name: 'read', result: '', active: false, is_error: false },
        { type: 'tool', call_id: '2', name: 'edit', result: '', active: false, is_error: false },
        { type: 'tool', call_id: '3', name: 'read', result: '', active: false, is_error: false },
      ]),
    ).toEqual(['read', 'edit']);
  });

  it('truncateText ellipsizes long strings', () => {
    expect(truncateText('short')).toBe('short');
    expect(truncateText('x'.repeat(120)).endsWith('…')).toBe(true);
  });

  it('groups subagents by prompt from metadata.blocks', () => {
    const map = {
      a: makeSubagent({ id: 'a', role_name: 'explore' }),
      b: makeSubagent({ id: 'b', role_name: 'shell' }),
      c: makeSubagent({ id: 'c', role_name: 'review' }),
    };
    const prompts = [
      makePrompt('p1', 0, 'how does auth work?', ['a', 'b']),
      makePrompt('p2', 1, 'refactor login', ['c']),
    ];

    const groups = groupSubagentsByPrompt(prompts, map, []);
    expect(groups).toHaveLength(2);
    expect(groups[0].promptId).toBe('p1');
    expect(groups[0].userPreview).toContain('how does auth');
    expect(groups[0].subagents.map((s) => s.id)).toEqual(['a', 'b']);
    expect(groups[1].subagents.map((s) => s.id)).toEqual(['c']);
    expect(countGroupedSubagents(groups)).toBe(3);
  });

  it('merges live turn subagentIds before metadata is persisted', () => {
    const map = {
      live1: makeSubagent({ id: 'live1', status: 'working', blocks: [] }),
    };
    const prompts = [
      makePrompt('p1', 0, 'spawn agents', []), // no refs in metadata yet
    ];
    const entries: ChatEntry[] = [
      { id: 'user-p1', type: 'user', promptId: 'p1', text: 'spawn agents' },
      { id: 'turn-p1', type: 'turn', promptId: 'p1', turnIndex: 0, subagentIds: ['live1'], blocks: [] },
    ];

    const groups = groupSubagentsByPrompt(prompts, map, entries);
    expect(groups).toHaveLength(1);
    expect(groups[0].subagents.map((s) => s.id)).toEqual(['live1']);
  });

  it('creates orphan group when prompt missing from allPrompts', () => {
    const map = {
      orphan: makeSubagent({ id: 'orphan' }),
    };
    const entries: ChatEntry[] = [
      { id: 'user-px', type: 'user', promptId: 'px', text: 'live only' },
      { id: 'turn-px', type: 'turn', promptId: 'px', turnIndex: 2, subagentIds: ['orphan'], blocks: [] },
    ];

    const groups = groupSubagentsByPrompt([], map, entries);
    expect(groups).toHaveLength(1);
    expect(groups[0].promptId).toBe('px');
    expect(groups[0].userPreview).toContain('live only');
    expect(groups[0].subagents[0].id).toBe('orphan');
  });

  it('falls back to metadata.subagents keys when blocks lack refs', () => {
    const map = {
      fromMap: makeSubagent({ id: 'fromMap', role_name: 'from-map' }),
    };
    const prompts = [makePrompt('p1', 0, 'resume case', ['fromMap'], true)];
    // Wipe blocks to force subagents-map fallback
    prompts[0].messages[1].metadata!.blocks = [];

    const groups = groupSubagentsByPrompt(prompts, map, []);
    expect(groups[0].subagents.map((s) => s.id)).toEqual(['fromMap']);
  });
});
