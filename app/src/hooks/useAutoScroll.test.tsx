// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAutoScroll } from './useAutoScroll';

type ResizeCallback = ResizeObserverCallback;

class ResizeObserverStub {
  static instances: ResizeObserverStub[] = [];
  readonly callback: ResizeCallback;
  disconnect = vi.fn();
  observe = vi.fn();
  unobserve = vi.fn();

  constructor(callback: ResizeCallback) {
    this.callback = callback;
    ResizeObserverStub.instances.push(this);
  }

  emit() {
    this.callback([], this as unknown as ResizeObserver);
  }
}

function Harness({ isProcessing = true }: { isProcessing?: boolean }) {
  const { scrollRef, contentRef } = useAutoScroll<HTMLDivElement, HTMLDivElement>({
    deps: [],
    isProcessing,
  });
  return (
    <div ref={scrollRef} data-testid="scroll">
      <div ref={contentRef} data-testid="content" />
    </div>
  );
}

describe('useAutoScroll resize-driven pinning', () => {
  let container: HTMLDivElement;
  let root: Root;
  let nextFrameId: number;
  let frames: Map<number, FrameRequestCallback>;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    ResizeObserverStub.instances = [];
    frames = new Map();
    nextFrameId = 1;
    vi.stubGlobal('ResizeObserver', ResizeObserverStub);
    vi.stubGlobal('requestAnimationFrame', vi.fn((callback: FrameRequestCallback) => {
      const id = nextFrameId++;
      frames.set(id, callback);
      return id;
    }));
    vi.stubGlobal('cancelAnimationFrame', vi.fn((id: number) => {
      frames.delete(id);
    }));
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
    vi.unstubAllGlobals();
  });

  it('does not schedule frames merely because a run is processing', () => {
    act(() => root.render(<Harness />));
    expect(requestAnimationFrame).not.toHaveBeenCalled();
    expect(ResizeObserverStub.instances).toHaveLength(1);
  });

  it('coalesces repeated content resizes into one pin frame', () => {
    act(() => root.render(<Harness />));
    const observer = ResizeObserverStub.instances[0];

    observer.emit();
    observer.emit();
    observer.emit();

    expect(requestAnimationFrame).toHaveBeenCalledTimes(1);
    expect(frames).toHaveLength(1);
  });

  it('does not re-stick after the user scrolls away', () => {
    act(() => root.render(<Harness />));
    const scroll = container.querySelector('[data-testid="scroll"]') as HTMLDivElement;
    Object.defineProperties(scroll, {
      clientHeight: { configurable: true, value: 200 },
      scrollHeight: { configurable: true, value: 1_000 },
    });
    scroll.scrollTop = 400;
    act(() => {
      scroll.dispatchEvent(new WheelEvent('wheel', { deltaY: -1 }));
      ResizeObserverStub.instances[0].emit();
    });
    const [frame] = frames.values();
    act(() => frame(0));

    expect(scroll.scrollTop).toBe(400);
  });

  it('corrects an overscrolled viewport when content shrinks', () => {
    act(() => root.render(<Harness />));
    const scroll = container.querySelector('[data-testid="scroll"]') as HTMLDivElement;
    Object.defineProperties(scroll, {
      clientHeight: { configurable: true, value: 200 },
      scrollHeight: { configurable: true, value: 500 },
    });
    scroll.scrollTop = 800;

    act(() => ResizeObserverStub.instances[0].emit());
    const [frame] = frames.values();
    act(() => frame(0));

    expect(scroll.scrollTop).toBe(300);
  });
});
