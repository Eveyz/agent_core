// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({ cacheHitRate: null as number | null }));

vi.mock('../../hooks/useTokenCount', () => ({
  useTurnCount: () => 2,
  useCacheHitRate: () => mocks.cacheHitRate,
}));

vi.mock('react-i18next', () => ({
  useTranslation: () => ({
    t: (key: string, values?: { count?: number; pct?: string | number }) => {
      if (key === 'chat.stats.turns') return `${values?.count} turns`;
      if (key === 'chat.stats.cacheHit') return `Cache hit: ${values?.pct}`;
      return key;
    },
  }),
}));

import { ChatStats } from './ChatInput';

describe('ChatStats cache hit', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    mocks.cacheHitRate = null;
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  it('keeps a placeholder visible before the latest turn reports usage', () => {
    act(() => root.render(<ChatStats />));

    expect(container.textContent).toContain('Cache hit: --');
  });

  it('renders the reported hit rate as a percentage', () => {
    mocks.cacheHitRate = 0.421;
    act(() => root.render(<ChatStats />));

    expect(container.textContent).toContain('Cache hit: 42%');
  });
});
