// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { LazyEntry } from './LazyEntry';

let highlightLoading = false;
vi.mock('./EntryRow', () => ({
  EntryRow: () => (
    <div data-highlight-loading={highlightLoading ? 'true' : 'false'}>
      rendered entry
    </div>
  ),
}));

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  constructor(private readonly callback: ResizeObserverCallback) {
    ResizeObserverStub.instances.push(this);
  }
  observe = vi.fn();
  disconnect = vi.fn();
  unobserve = vi.fn();
  emit(height: number) {
    this.callback([
      { contentRect: { height } as DOMRectReadOnly } as ResizeObserverEntry,
    ], this as unknown as ResizeObserver);
  }
}

class IntersectionObserverStub {
  static instances: IntersectionObserverStub[] = [];
  constructor(private readonly callback: IntersectionObserverCallback) {
    IntersectionObserverStub.instances.push(this);
  }
  observe = vi.fn();
  disconnect = vi.fn();
  unobserve = vi.fn();
  takeRecords = vi.fn(() => []);
  root = null;
  rootMargin = '';
  thresholds = [];
  emit(isIntersecting: boolean) {
    this.callback([
      { isIntersecting } as IntersectionObserverEntry,
    ], this as unknown as IntersectionObserver);
  }
}

describe('LazyEntry measured placeholders', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    highlightLoading = false;
    ResizeObserverStub.instances = [];
    IntersectionObserverStub.instances = [];
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
    vi.stubGlobal('IntersectionObserver', IntersectionObserverStub);
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  function render(forceVisible: boolean, onReady = vi.fn()) {
    act(() => {
      root.render(
        <LazyEntry
          entryId="old-entry"
          defaultModel="mock"
          handleRetry={vi.fn()}
          isProcessing={false}
          scrollRef={{ current: container }}
          forceVisible={forceVisible}
          onReady={onReady}
        />,
      );
    });
    return onReady;
  }

  it('keeps the measured height when a force-mounted entry becomes a placeholder', () => {
    render(true);
    act(() => ResizeObserverStub.instances[0].emit(720));

    render(false);
    act(() => IntersectionObserverStub.instances[0].emit(false));

    const wrapper = container.querySelector('[data-entry-id="old-entry"]') as HTMLElement;
    expect(wrapper.className).toBe('lazy-entry-placeholder');
    expect(wrapper.style.minHeight).toBe('720px');
    expect(wrapper.childElementCount).toBe(0);
  });

  it('does not report ready until asynchronous highlighting has finished', async () => {
    highlightLoading = true;
    const onReady = render(true);
    expect(onReady).not.toHaveBeenCalled();

    highlightLoading = false;
    render(true, onReady);
    await act(async () => Promise.resolve());

    expect(onReady).toHaveBeenCalledWith('old-entry');
  });
});
