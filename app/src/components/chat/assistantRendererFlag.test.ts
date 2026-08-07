import { describe, expect, it } from 'vitest';
import { resolveStreamdownAssistantFlag } from './assistantRendererFlag';

describe('resolveStreamdownAssistantFlag', () => {
  it('keeps the current renderer as the safe default', () => {
    expect(resolveStreamdownAssistantFlag(undefined, null)).toBe(false);
  });

  it('accepts the build-time flag for a controlled rollout', () => {
    expect(resolveStreamdownAssistantFlag('true', null)).toBe(true);
    expect(resolveStreamdownAssistantFlag('false', null)).toBe(false);
  });

  it('lets a local override win over the build-time default', () => {
    expect(resolveStreamdownAssistantFlag('false', 'true')).toBe(true);
    expect(resolveStreamdownAssistantFlag('true', 'false')).toBe(false);
  });

  it('ignores malformed local overrides', () => {
    expect(resolveStreamdownAssistantFlag('true', 'maybe')).toBe(true);
  });
});
