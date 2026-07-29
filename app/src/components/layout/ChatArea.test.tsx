// @vitest-environment jsdom
import { act, useLayoutEffect, useRef, useState } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

const mocks = vi.hoisted(() => ({
  state: {
    project: { activeSessionId: 's1' },
    chat: {
      allPrompts: { s1: [{}, {}, {}, {}] },
      visiblePromptsCount: { s1: 2 },
    },
  },
  dispatch: (() => {}) as (action: unknown) => void,
  ready: new Map<string, () => void>(),
}));

vi.mock('react-redux', () => ({
  shallowEqual: Object.is,
  useSelector: (selector: (state: typeof mocks.state) => unknown) => selector(mocks.state),
}));

vi.mock('../../hooks/useAppDispatch', () => ({
  useAppDispatch: () => mocks.dispatch,
}));

vi.mock('../../features/chat/chatSlice', () => ({
  loadMorePrompts: () => ({ type: 'chat/loadMorePrompts' }),
  selectActiveBtwEntries: () => [],
}));

vi.mock('../chat/LazyEntry', async () => {
  const React = await import('react');
  return {
    LazyEntry: (props: {
      entryId: string;
      onReady?: (entryId: string) => void;
    }) => {
      React.useEffect(() => {
        mocks.ready.set(props.entryId, () => props.onReady?.(props.entryId));
        return () => {
          mocks.ready.delete(props.entryId);
        };
      }, [props.entryId, props.onReady]);
      return <div data-entry-id={props.entryId}>{props.entryId}</div>;
    },
  };
});

import { ChatArea } from './ChatArea';

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  constructor(private readonly callback: ResizeObserverCallback) {
    ResizeObserverStub.instances.push(this);
  }
  observe = vi.fn();
  disconnect = vi.fn();
  unobserve = vi.fn();
  emit() {
    this.callback([], this as unknown as ResizeObserver);
  }
}

function Harness({ onRefs, onPrepend }: {
  onRefs: (scroll: HTMLDivElement, content: HTMLDivElement) => void;
  onPrepend?: () => void;
}) {
  const [entryIds, setEntryIds] = useState(['current-first', 'current-last']);
  const scrollRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  mocks.dispatch = () => {
    onPrepend?.();
    setEntryIds([
      'old-user',
      'old-turn-a',
      'old-steer',
      'old-turn-b',
      'current-first',
      'current-last',
    ]);
  };

  useLayoutEffect(() => {
    if (scrollRef.current && contentRef.current) {
      onRefs(scrollRef.current, contentRef.current);
    }
  });

  return (
    <ChatArea
      entryIds={entryIds}
      defaultModel="mock"
      isProcessing={false}
      scrollRef={scrollRef}
      contentRef={contentRef}
      isAtBottom={false}
      scrollToBottom={vi.fn()}
      handleRetry={vi.fn()}
      onSend={vi.fn()}
    />
  );
}

describe('ChatArea prepend transaction', () => {
  let container: HTMLDivElement;
  let root: Root;
  let frames: FrameRequestCallback[];

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    mocks.ready.clear();
    ResizeObserverStub.instances = [];
    frames = [];
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
    vi.stubGlobal('requestAnimationFrame', vi.fn((callback: FrameRequestCallback) => {
      frames.push(callback);
      return frames.length;
    }));
    vi.stubGlobal('cancelAnimationFrame', vi.fn());
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  function flushFrames() {
    while (frames.length > 0) {
      const callbacks = frames.splice(0);
      act(() => callbacks.forEach((callback) => callback(0)));
    }
  }

  it('holds the painted anchor through delayed resize, then releases after ready and stable', () => {
    let scroll!: HTMLDivElement;
    let contentBeforeAnchor = 0;
    act(() => {
      root.render(
        <Harness
          onRefs={(nextScroll) => { scroll = nextScroll; }}
          onPrepend={() => { contentBeforeAnchor = 480; }}
        />,
      );
    });
    const anchor = container.querySelector(
      '[data-entry-id="current-first"]',
    ) as HTMLElement;
    vi.spyOn(anchor, 'getBoundingClientRect').mockImplementation(() => ({
      top: contentBeforeAnchor - scroll.scrollTop,
    } as DOMRect));

    scroll.scrollTop = 0;
    act(() => scroll.dispatchEvent(new Event('scroll', { bubbles: true })));
    expect(anchor.getBoundingClientRect().top).toBe(0);

    // The controller-level test separately advances beyond the old one-second
    // cutoff; here we exercise the real ResizeObserver → pre-paint correction.
    contentBeforeAnchor = 1_360;
    act(() => ResizeObserverStub.instances[0].emit());
    expect(anchor.getBoundingClientRect().top).toBe(0);
    flushFrames();
    expect(anchor.getBoundingClientRect().top).toBe(0);

    act(() => {
      for (const id of ['old-user', 'old-turn-a', 'old-steer', 'old-turn-b']) {
        mocks.ready.get(id)?.();
      }
    });
    flushFrames();

    const settledScrollTop = scroll.scrollTop;
    contentBeforeAnchor = 1_500;
    act(() => ResizeObserverStub.instances[0].emit());
    flushFrames();
    expect(scroll.scrollTop).toBe(settledScrollTop);
  });
});
