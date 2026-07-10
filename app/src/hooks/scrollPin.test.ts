import { describe, expect, it } from 'vitest';
import { isNearBottom, maxScrollTop, pinnedScrollTop } from './scrollPin';

describe('pinnedScrollTop', () => {
  it('pins upward when content grew past the viewport (undershoot)', () => {
    // viewport 400, content 2000 → max = 1600; currently at 1500
    expect(pinnedScrollTop(1500, 2000, 400, true)).toBe(1600);
  });

  it('pins downward when content shrank under scrollTop (overshoot / blank viewport)', () => {
    // After a markdown/code remount, scrollHeight dropped but scrollTop stayed high.
    // max = 1000; scrollTop = 1400 → blank cosmic background until corrected.
    expect(pinnedScrollTop(1400, 1400, 400, true)).toBe(1000);
  });

  it('does not write when already within 1px of the bottom', () => {
    expect(pinnedScrollTop(1600, 2000, 400, true)).toBeNull();
    expect(pinnedScrollTop(1599.5, 2000, 400, true)).toBeNull();
  });

  it('does nothing when stick-to-bottom is off', () => {
    expect(pinnedScrollTop(1400, 1400, 400, false)).toBeNull();
  });

  it('does nothing when the container has no layout yet', () => {
    expect(pinnedScrollTop(0, 2000, 0, true)).toBeNull();
  });
});

describe('maxScrollTop / isNearBottom', () => {
  it('never returns a negative max', () => {
    expect(maxScrollTop(100, 400)).toBe(0);
  });

  it('treats within-threshold as near bottom', () => {
    expect(isNearBottom(1560, 2000, 400)).toBe(true); // 40px away
    expect(isNearBottom(1559, 2000, 400)).toBe(false); // 41px away
  });
});

/**
 * Reproduce the OLD buggy guard that only corrected undershoot.
 * Kept as a documentation assertion so we don't regress to it.
 */
describe('legacy undershoot-only guard (must stay wrong)', () => {
  function legacyPinned(scrollTop: number, scrollHeight: number, clientHeight: number): number | null {
    const max = Math.max(0, scrollHeight - clientHeight);
    if (scrollTop < max - 1) return max; // ← never corrects overshoot
    return null;
  }

  it('fails to correct overshoot — the blank-viewport bug', () => {
    expect(legacyPinned(1400, 1400, 400)).toBeNull();
    // Fixed helper does correct it:
    expect(pinnedScrollTop(1400, 1400, 400, true)).toBe(1000);
  });
});
