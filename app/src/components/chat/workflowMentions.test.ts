import { describe, expect, it } from 'vitest';
import { resolveWorkflowMentions } from './workflowMentions';

describe('resolveWorkflowMentions', () => {
  const mention = {
    workflow_id: 'workflow-a',
    revision_id: 'workflow-a:r2:hash',
    scope: 'project',
    display_token: '@workflow:Research',
    token: '@workflow:Research',
  };

  it('keeps the pinned revision while the visible token remains', () => {
    expect(resolveWorkflowMentions('Please run @workflow:Research now', [mention])).toEqual([
      {
        workflow_id: 'workflow-a',
        revision_id: 'workflow-a:r2:hash',
        scope: 'project',
        display_token: '@workflow:Research',
        optional: undefined,
      },
    ]);
  });

  it('drops composer selections whose token was deleted', () => {
    expect(resolveWorkflowMentions('Please run this now', [mention])).toEqual([]);
  });

  it('deduplicates the same workflow identity', () => {
    expect(resolveWorkflowMentions('@workflow:Research', [mention, mention])).toHaveLength(1);
  });
});
