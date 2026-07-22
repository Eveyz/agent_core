import { describe, expect, it } from 'vitest';
import {
  BOTTOM_THRESHOLD_PX,
  decideStickAfterScroll,
  isNearBottom,
  maxScrollTop,
  pinnedScrollTop,
  STICK_REJOIN_PX,
} from './scrollPin';

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

  it('treats within soft threshold as near bottom', () => {
    const max = 1600;
    expect(isNearBottom(max - BOTTOM_THRESHOLD_PX, 2000, 400)).toBe(true);
    expect(isNearBottom(max - BOTTOM_THRESHOLD_PX - 1, 2000, 400)).toBe(false);
  });
});

describe('decideStickAfterScroll', () => {
  const height = 2000;
  const view = 400;
  const max = 1600;

  it('rejoins stick only when essentially docked', () => {
    expect(decideStickAfterScroll(max - STICK_REJOIN_PX, height, view, false, true)).toEqual({
      stickToBottom: true,
      isAtBottom: true,
    });
  });

  it('does not re-stick inside the soft zone after the user left (escape vs stream pin)', () => {
    // Wheel-up cleared stick; user is still within BOTTOM_THRESHOLD — must not
    // snap stick back on, or the next rAF pin re-captures them. Button can hide.
    const distance = Math.floor((STICK_REJOIN_PX + BOTTOM_THRESHOLD_PX) / 2);
    expect(distance).toBeGreaterThan(STICK_REJOIN_PX);
    expect(distance).toBeLessThanOrEqual(BOTTOM_THRESHOLD_PX);

    expect(decideStickAfterScroll(max - distance, height, view, false, true)).toEqual({
      stickToBottom: false,
      isAtBottom: true,
    });
  });

  it('keeps stick while still following inside the soft zone', () => {
    const distance = Math.floor((STICK_REJOIN_PX + BOTTOM_THRESHOLD_PX) / 2);
    expect(decideStickAfterScroll(max - distance, height, view, true, true)).toEqual({
      stickToBottom: true,
      isAtBottom: true,
    });
  });

  it('clears stick when far and not streaming', () => {
    expect(decideStickAfterScroll(max - BOTTOM_THRESHOLD_PX - 1, height, view, true, false)).toEqual({
      stickToBottom: false,
      isAtBottom: false,
    });
  });

  it('preserves stick when far during streaming (growth can outrun a pin frame)', () => {
    expect(decideStickAfterScroll(max - BOTTOM_THRESHOLD_PX - 1, height, view, true, true)).toEqual({
      stickToBottom: true,
      isAtBottom: false,
    });
  });

  it('ignores layout overshoot', () => {
    expect(decideStickAfterScroll(max + 40, height, view, true, true)).toBeNull();
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
