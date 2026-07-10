/** Distance from the bottom (px) at which we still consider the user "at bottom". */
export const BOTTOM_THRESHOLD_PX = 40;

export function maxScrollTop(scrollHeight: number, clientHeight: number): number {
  return Math.max(0, scrollHeight - clientHeight);
}

export function isNearBottom(
  scrollTop: number,
  scrollHeight: number,
  clientHeight: number,
  threshold = BOTTOM_THRESHOLD_PX,
): boolean {
  return maxScrollTop(scrollHeight, clientHeight) - scrollTop <= threshold;
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
