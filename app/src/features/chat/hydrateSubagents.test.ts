import { describe, expect, it } from 'vitest';
import type { SubagentEntry, TurnBlock } from './types';
import {
  extractBatchSection,
  hydrateSubagentsFromBlocks,
  spawnSpecsFromArgs,
} from './hydrateSubagents';

describe('hydrateSubagents', () => {
  it('parses single and batch spawn specs', () => {
    expect(spawnSpecsFromArgs({ id: 'weather-shanghai', task: 'check SH' })).toEqual([
      { roleName: 'weather-shanghai', task: 'check SH' },
    ]);
    expect(
      spawnSpecsFromArgs({
        tasks: [
          { id: 'a', task: 'one' },
          { id: 'b', task: 'two' },
        ],
      }),
    ).toEqual([
      { roleName: 'a', task: 'one' },
      { roleName: 'b', task: 'two' },
    ]);
  });

  it('extracts a batch section by role name', () => {
    const result = `=== Sub-agent Batch Results (2 tasks) ===

[1] weather-shanghai — success
Shanghai is rainy.

[2] weather-shenzhen — success
Shenzhen is sunny.

=== End batch results ===`;
    expect(extractBatchSection(result, 'weather-shenzhen')).toContain('Shenzhen is sunny');
    expect(extractBatchSection(result, 'weather-shenzhen')).not.toContain('Shanghai');
  });

  it('hydrates refs and SubagentEntry from spawn tool blocks', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: 'call-1',
        name: 'subagents',
        args: {
          tasks: [
            { id: 'weather-shanghai', task: 'SH weather' },
            { id: 'weather-shenzhen', task: 'SZ weather' },
          ],
        },
        result: `=== Sub-agent Batch Results (2 tasks) ===

[1] weather-shanghai — success
[subagent-handoff/v1]
runtime_id: 11111111-1111-1111-1111-111111111111
status: succeeded
context_sufficient: true
iterations: 1
tools: 1

Shanghai rainy
<context_status>{"sufficient":true,"missing":[],"unresolved":[]}</context_status>

[2] weather-shenzhen — success
[subagent-handoff/v1]
runtime_id: 22222222-2222-2222-2222-222222222222
status: succeeded
context_sufficient: true
iterations: 1
tools: 1

Shenzhen sunny

=== End batch results ===`,
        active: false,
        is_error: false,
      },
    ];
    const map: Record<string, SubagentEntry> = {};
    const { blocks: next, subagentIds } = hydrateSubagentsFromBlocks(blocks, map);

    expect(subagentIds).toEqual([
      '11111111-1111-1111-1111-111111111111',
      '22222222-2222-2222-2222-222222222222',
    ]);
    expect(next.filter((b) => b.type === 'subagent_ref')).toHaveLength(2);
    expect(map['11111111-1111-1111-1111-111111111111']?.role_name).toBe('weather-shanghai');
    expect(map['22222222-2222-2222-2222-222222222222']?.role_name).toBe('weather-shenzhen');
    const shText = map['11111111-1111-1111-1111-111111111111']?.blocks.find(
      (b) => b.type === 'assistant',
    );
    expect(shText && shText.type === 'assistant' && shText.text).toContain('Shanghai rainy');
    expect(shText && shText.type === 'assistant' && shText.text).not.toContain('context_status');
  });

  it('falls back to call_id:role when runtime_id missing', () => {
    const blocks: TurnBlock[] = [
      {
        type: 'tool',
        call_id: 'c9',
        name: 'subagent',
        args: { id: 'explore', task: 'look around' },
        result: 'All done',
        active: false,
        is_error: false,
      },
    ];
    const map: Record<string, SubagentEntry> = {};
    const { subagentIds } = hydrateSubagentsFromBlocks(blocks, map);
    expect(subagentIds).toEqual(['c9:explore']);
    expect(map['c9:explore']?.task).toBe('look around');
  });
});
