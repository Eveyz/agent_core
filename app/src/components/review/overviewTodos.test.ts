import { describe, expect, it } from 'vitest';
import type { FrontendPrompt, PlanDetail } from '../../features/chat/types';
import { countPlanItems, groupPlansByPrompt, planProgress } from './overviewTodos';

function makePrompt(id: string, turnIndex: number, userText: string): FrontendPrompt {
  return {
    id,
    session_id: 's1',
    turn_index: turnIndex,
    model: 'm',
    status: 'completed',
    token_usage: {},
    started_at: null,
    ended_at: null,
    created_at: '2026-01-01',
    messages: [{ role: 'user', content: userText }],
  };
}

function makePlan(partial: Partial<PlanDetail> & { id: string }): PlanDetail {
  return {
    title: partial.title ?? 'Plan',
    status: partial.status ?? 'active',
    source_prompt_id: partial.source_prompt_id ?? null,
    updated_at: partial.updated_at ?? '2026-01-02T00:00:00Z',
    items: partial.items ?? [
      { id: '1', description: 'a', status: 'completed' },
      { id: '2', description: 'b', status: 'pending' },
    ],
    id: partial.id,
  };
}

describe('overviewTodos', () => {
  it('counts items and progress', () => {
    const items = [
      { id: '1', description: 'a', status: 'completed' as const },
      { id: '2', description: 'b', status: 'pending' as const },
      { id: '3', description: 'c', status: 'completed' as const },
    ];
    expect(planProgress(items)).toEqual({ completed: 2, total: 3 });
    expect(countPlanItems([makePlan({ id: 'p1', items })])).toBe(3);
  });

  it('groups plans by source_prompt_id in turn order', () => {
    const prompts = [
      makePrompt('p1', 0, 'first task'),
      makePrompt('p2', 1, 'second task'),
    ];
    const plans = [
      makePlan({ id: 'a', source_prompt_id: 'p2', title: 'Later', status: 'finished' }),
      makePlan({ id: 'b', source_prompt_id: 'p1', title: 'Earlier', status: 'cancelled' }),
      makePlan({ id: 'c', source_prompt_id: null, title: 'Loose', status: 'active' }),
    ];

    const groups = groupPlansByPrompt(plans, prompts);
    expect(groups).toHaveLength(3);
    expect(groups[0].promptId).toBe('p1');
    expect(groups[0].plans.map((p) => p.id)).toEqual(['b']);
    expect(groups[1].promptId).toBe('p2');
    expect(groups[1].userPreview).toContain('second');
    expect(groups[2].promptId).toBeNull();
    expect(groups[2].userPreview).toBe('Current');
    expect(groups[2].plans[0].id).toBe('c');
  });
});
