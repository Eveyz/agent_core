// @vitest-environment jsdom
import { describe, expect, it, vi } from 'vitest';
import {
  PrependScrollAnchor,
  entriesThroughAnchor,
} from './prependScrollAnchor';

describe('prepend scroll anchoring', () => {
  it('keeps the same entry at the same viewport position across delayed height changes', () => {
    vi.useFakeTimers();
    const scroll = document.createElement('div');
    const anchor = document.createElement('div');
    anchor.dataset.entryId = 'current-first';
    scroll.appendChild(anchor);

    let contentBeforeAnchor = 0;
    scroll.scrollTop = 0;
    vi.spyOn(anchor, 'getBoundingClientRect').mockImplementation(() => ({
      top: contentBeforeAnchor - scroll.scrollTop,
      bottom: 0,
      left: 0,
      right: 0,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    }));

    const preservation = new PrependScrollAnchor();
    preservation.capture(scroll, 'current-first');

    contentBeforeAnchor = 480;
    preservation.restore(scroll);
    expect(anchor.getBoundingClientRect().top).toBe(0);

    vi.advanceTimersByTime(1_200);
    contentBeforeAnchor = 1_360;
    preservation.restore(scroll);

    expect(preservation.isActive()).toBe(true);
    expect(anchor.getBoundingClientRect().top).toBe(0);
    vi.useRealTimers();
  });

  it('force-mounts every prepended entry through the captured anchor', () => {
    expect(
      entriesThroughAnchor(
        ['old-user', 'old-turn-a', 'old-steer', 'old-turn-b', 'current-first', 'current-last'],
        'current-first',
      ),
    ).toEqual(new Set([
      'old-user',
      'old-turn-a',
      'old-steer',
      'old-turn-b',
      'current-first',
    ]));
  });

  it('immediately yields to explicit user scroll intent', () => {
    const scroll = document.createElement('div');
    const anchor = document.createElement('div');
    anchor.dataset.entryId = 'anchor';
    scroll.appendChild(anchor);
    vi.spyOn(anchor, 'getBoundingClientRect').mockReturnValue({
      top: 0,
    } as DOMRect);

    const preservation = new PrependScrollAnchor();
    preservation.capture(scroll, 'anchor');
    preservation.cancel();
    scroll.scrollTop = 42;
    preservation.restore(scroll);

    expect(preservation.isActive()).toBe(false);
    expect(scroll.scrollTop).toBe(42);
  });
});
