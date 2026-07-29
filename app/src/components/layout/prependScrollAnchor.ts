export interface CapturedScrollAnchor {
  entryId: string;
  viewportTop: number;
}

function findEntry(scrollEl: HTMLElement, entryId: string): HTMLElement | null {
  return Array.from(
    scrollEl.querySelectorAll<HTMLElement>('[data-entry-id]'),
  ).find((entry) => entry.dataset.entryId === entryId) ?? null;
}

/**
 * Keeps one existing entry fixed in viewport coordinates while older entries
 * are inserted and continue resolving dynamic heights.
 *
 * It intentionally has no timer. The owner cancels it only on explicit user
 * scroll intent, session replacement, or when a newer prepend captures a new
 * anchor.
 */
export class PrependScrollAnchor {
  private captured: CapturedScrollAnchor | null = null;

  capture(scrollEl: HTMLElement, entryId: string): boolean {
    const anchor = findEntry(scrollEl, entryId);
    if (!anchor) return false;
    this.captured = {
      entryId,
      viewportTop: anchor.getBoundingClientRect().top,
    };
    return true;
  }

  restore(scrollEl: HTMLElement): void {
    if (!this.captured) return;
    const anchor = findEntry(scrollEl, this.captured.entryId);
    if (!anchor) return;
    const delta = anchor.getBoundingClientRect().top - this.captured.viewportTop;
    if (Math.abs(delta) > 0.5) {
      scrollEl.scrollTop += delta;
    }
  }

  cancel(): void {
    this.captured = null;
  }

  isActive(): boolean {
    return this.captured !== null;
  }

  anchorId(): string | null {
    return this.captured?.entryId ?? null;
  }
}

/** IDs that must mount synchronously for a prepend, including the old anchor. */
export function entriesThroughAnchor(
  entryIds: string[],
  anchorId: string | null,
): Set<string> {
  if (!anchorId) return new Set();
  const anchorIndex = entryIds.indexOf(anchorId);
  if (anchorIndex < 0) return new Set();
  return new Set(entryIds.slice(0, anchorIndex + 1));
}
