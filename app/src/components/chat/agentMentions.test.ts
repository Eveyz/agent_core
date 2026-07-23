import { describe, expect, it } from 'vitest';
import { resolveAgentMentions } from './agentMentions';
import type { AgentDef } from '../../features/agents/types';

const coder: AgentDef = {
  id: 'agent-coder',
  name: 'coder',
  description: 'Writes code',
  system_prompt: 'Write code',
  model: 'test/model',
  tools: [],
  skills: [],
  permission_mode: 'standard',
  permission_rules: {},
  max_iterations: 20,
  max_context_tokens: 32_000,
  memory_enabled: 0,
  memory_group: '',
  icon: '',
  color: '',
  created_at: '2026-07-24T00:00:00Z',
  updated_at: 'rev-coder',
};

describe('resolveAgentMentions', () => {
  it('turns a manually typed unique saved-agent mention into a structured payload', () => {
    expect(resolveAgentMentions('@coder 你可以干嘛', [], [coder])).toEqual([
      { agent_id: 'agent-coder', revision_id: 'rev-coder' },
    ]);
  });

  it('does not bind unknown or ambiguous mention text', () => {
    const duplicate = { ...coder, id: 'agent-coder-2' };
    expect(resolveAgentMentions('@unknown hello', [], [coder])).toEqual([]);
    expect(resolveAgentMentions('@coder hello', [], [coder, duplicate])).toEqual([]);
    expect(resolveAgentMentions('@coder2 hello', [], [coder])).toEqual([]);
  });
});
