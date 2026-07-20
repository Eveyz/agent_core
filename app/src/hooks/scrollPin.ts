/** Soft zone: hide the jump button / treat as "near bottom" for UI. */
export const BOTTOM_THRESHOLD_PX = 48;

/**
 * Must get this close to the bottom before stick re-enables after the user
 * scrolled away. Larger than this (but still inside BOTTOM_THRESHOLD) must
 * NOT re-stick — otherwise a wheel-up escape is undone by the next scroll
 * event while the stream pin loop yanks the viewport back down.
 */
export const STICK_REJOIN_PX = 12;

export function maxScrollTop(scrollHeight: number, clientHeight: number): number {
  return Math.max(0, scrollHeight - clientHeight);
}

export function distanceFromBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
): number {
  return maxScrollTop(scrollHeight, clientHeight) - scrollTop;
}

export function isNearBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  threshold = BOTTOM_THRESHOLD_PX,
): boolean {
  return distanceFromBottom(scrollTop, scrollHeight, clientHeight) <= threshold;
}

export type StickScrollDecision = {
  stickToBottom: boolean;
  isAtBottom: boolean;
};

/**
 * Decide stick / at-bottom flags after a *user* scroll (not programmatic).
 * Returns null when the event should be ignored (layout overshoot).
 */
export function decideStickAfterScroll(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  stickToBottom: boolean,
  isProcessing: boolean,
): StickScrollDecision | null {
  const max = maxScrollTop(scrollHeight, clientHeight);
  // Overshoot is a layout artifact — keep sticking; the pin loop corrects it.
  if (scrollTop > max + 1) return null;

  const distance = max - scrollTop;

  // Truly docked → rejoin (or stay) stuck.
  if (distance <= STICK_REJOIN_PX) {
    return { stickToBottom: true, isAtBottom: true };
  }

  // Soft near zone: never force stick back on. Preserve an intentional leave
  // so streaming pins cannot re-capture the user mid-gesture.
  if (distance <= BOTTOM_THRESHOLD_PX) {
    return { stickToBottom, isAtBottom: stickToBottom };
  }

  // Far from bottom. While streaming, only wheel/touch may clear stick —
  // content can outrun a pin frame and briefly look far.
  return {
    stickToBottom: isProcessing ? stickToBottom : false,
    isAtBottom: false,
  };
}

/**
 * If stick-to-bottom is on, return the scrollTop we should assign so the
 * viewport stays pinned. Returns null when no write is needed.
 *
 * Critical: correct BOTH undershoot (content grew) AND overshoot (content
 * shrank after markdown/code remounts). Only checking `scrollTop < max`
 * leaves the viewport past the content — a blank pane until the user
 * nudges the scrollbar.
 */
export function pinnedScrollTop(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  stickToBottom: boolean,
): number | null {
  if (!stickToBottom || clientHeight <= 0) return null;
  const max = maxScrollTop(scrollHeight, clientHeight);
  if (Math.abs(scrollTop - max) <= 1) return null;
  return max;
}
