/**
 * PROTOTYPE — delete or absorb after deciding whether Streamdown should replace
 * MarkdownContent. Renderer variants live at ?variant=A|B.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { Root } from 'react-dom/client';
import { flushSync } from 'react-dom';
import { Streamdown } from 'streamdown';
import { createMathPlugin } from '@streamdown/math';
import { MarkdownContent } from '../MarkdownContent';
import { streamdownCodePlugin } from '../StreamdownAssistantContent';
import 'streamdown/styles.css';
import 'katex/dist/katex.min.css';
import '../../../App.css';
import './streamdown-prototype.css';

type VariantKey = 'A' | 'B';

const VARIANTS: Record<VariantKey, { name: string; description: string }> = {
  A: {
    name: 'Current baseline',
    description: '现有 Marked + DOMPurify + 自定义流式分块 + Shiki 队列',
  },
  B: {
    name: 'Streamdown matched',
    description: 'Streamdown + Code + Math，关闭动画与额外控制，尽量对齐现有能力',
  },
};

const MATH = createMathPlugin({ singleDollarTextMath: true });

const SAMPLE = [
  '# Streamdown 真实流式评估',
  '',
  '这段内容覆盖 **粗体**、*斜体*、~~删除线~~、`inline code` 与 [外部链接](https://example.com)。',
  '',
  '> 流式阶段会故意经过未闭合的 **强调、链接和代码围栏，观察页面是否抖动。',
  '',
  '## 表格',
  '',
  '| 指标 | 当前值 | 变化 |',
  '| --- | ---: | ---: |',
  '| 延迟 | 42 ms | -18% |',
  '| 吞吐 | 128 tok/s | +12% |',
  '',
  '行内公式 $E = mc^2$，展示公式：',
  '',
  '$$\\int_{-\\infty}^{\\infty} e^{-x^2} dx = \\sqrt{\\pi}$$',
  '',
  '## TypeScript',
  '',
  '```typescript',
  'type StreamState = {',
  "  status: 'idle' | 'streaming' | 'done';",
  '  chunks: number;',
  '};',
  '',
  'export const append = (state: StreamState): StreamState => ({',
  "  ...state, status: 'streaming', chunks: state.chunks + 1,",
  '});',
  '```',
  '',
  '## Mermaid',
  '',
  '```mermaid',
  'flowchart LR',
  '  SSE --> Rust',
  '  Rust --> Tauri',
  '  Tauri --> Redux',
  '  Redux --> React',
  '```',
  '',
  'CJK 边界：中文**加粗**不应吞掉标点；「你好」~~旧内容~~新内容。',
  '',
  '<img src="x" onerror="alert(1)" alt="安全测试">',
  '',
  '最后一段用于确认流结束后 DOM 高度不会突然跳变。',
].join('\n');

const CHUNK_SIZE = 9;
const CHUNK_INTERVAL_MS = 28;

function readVariant(): VariantKey {
  const value = new URLSearchParams(window.location.search).get('variant');
  return value === 'B' ? value : 'A';
}

function readInitialState(): { cursor: number; playing: boolean; speed: number } {
  const params = new URLSearchParams(window.location.search);
  const requestedSpeed = Number(params.get('speed'));
  const requestedCursor = Number(params.get('cursor'));
  return {
    cursor: params.get('full') === '1'
      ? SAMPLE.length
      : Number.isFinite(requestedCursor)
        ? Math.min(SAMPLE.length, Math.max(0, requestedCursor))
        : 0,
    playing: params.get('autoplay') === '1',
    speed: [0.5, 1, 2, 4].includes(requestedSpeed) ? requestedSpeed : 1,
  };
}

function Renderer({ variant, content, streaming }: {
  variant: VariantKey;
  content: string;
  streaming: boolean;
}) {
  if (variant === 'A') {
    return <MarkdownContent content={content} className="assistant-msg" isStreaming={streaming} />;
  }

  return (
    <div className="assistant-msg streamdown-shell" onClick={(event) => {
      const anchor = (event.target as HTMLElement).closest('a');
      if (anchor) event.preventDefault();
    }}>
      <Streamdown
        animated={false}
        controls={false}
        dir="auto"
        isAnimating={streaming}
        lineNumbers={false}
        linkSafety={{ enabled: false }}
        mode={streaming ? 'streaming' : 'static'}
        plugins={{ code: streamdownCodePlugin, math: MATH }}
        shikiTheme={['vitesse-light', 'vitesse-dark']}
      >
        {content}
      </Streamdown>
    </div>
  );
}

function PrototypeSwitcher({ current }: { current: VariantKey }) {
  const keys = Object.keys(VARIANTS) as VariantKey[];

  const select = useCallback((offset: number) => {
    const index = keys.indexOf(current);
    const next = keys[(index + offset + keys.length) % keys.length];
    const url = new URL(window.location.href);
    url.searchParams.set('variant', next);
    window.location.assign(url);
  }, [current, keys]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.matches('input, textarea, [contenteditable="true"]')) return;
      if (event.key === 'ArrowLeft') select(-1);
      if (event.key === 'ArrowRight') select(1);
    };
    window.addEventListener('keydown', handleKeyDown);
    return () => window.removeEventListener('keydown', handleKeyDown);
  }, [select]);

  return (
    <nav className="prototype-switcher" aria-label="Renderer variants">
      <button onClick={() => select(-1)} aria-label="Previous variant">←</button>
      <span>{current} — {VARIANTS[current].name}</span>
      <button onClick={() => select(1)} aria-label="Next variant">→</button>
    </nav>
  );
}

function StreamdownPrototype() {
  const variant = readVariant();
  const initialState = useMemo(readInitialState, []);
  const [cursor, setCursor] = useState(initialState.cursor);
  const [playing, setPlaying] = useState(initialState.playing);
  const [speed, setSpeed] = useState(initialState.speed);
  const [renderSamples, setRenderSamples] = useState<number[]>([]);
  const surfaceRef = useRef<HTMLDivElement>(null);
  const resizeCountRef = useRef(0);
  const mutationCountRef = useRef(0);
  const content = SAMPLE.slice(0, cursor);
  const streaming = cursor < SAMPLE.length;

  useEffect(() => {
    if (!playing || !streaming) {
      if (!streaming) setPlaying(false);
      return;
    }
    const timer = window.setInterval(() => {
      const started = performance.now();
      flushSync(() => {
        setCursor((value) => Math.min(SAMPLE.length, value + CHUNK_SIZE));
      });
      const elapsed = performance.now() - started;
      setRenderSamples((samples) => [...samples.slice(-199), elapsed]);
    }, CHUNK_INTERVAL_MS / speed);
    return () => window.clearInterval(timer);
  }, [playing, speed, streaming]);

  useEffect(() => {
    const surface = surfaceRef.current;
    if (!surface) return;
    const resizeObserver = new ResizeObserver(() => { resizeCountRef.current += 1; });
    const mutationObserver = new MutationObserver((records) => {
      mutationCountRef.current += records.length;
    });
    resizeObserver.observe(surface);
    mutationObserver.observe(surface, { attributes: true, childList: true, subtree: true });
    return () => {
      resizeObserver.disconnect();
      mutationObserver.disconnect();
    };
  }, [variant]);

  const stats = useMemo(() => {
    const sorted = [...renderSamples].sort((a, b) => a - b);
    const p95 = sorted[Math.floor(sorted.length * 0.95)] ?? 0;
    const max = sorted[sorted.length - 1] ?? 0;
    return { p95, max };
  }, [renderSamples]);

  const reset = () => {
    setPlaying(false);
    setCursor(0);
    setRenderSamples([]);
    resizeCountRef.current = 0;
    mutationCountRef.current = 0;
  };

  return (
    <main className="streamdown-prototype">
      <header className="prototype-header">
        <div>
          <span className="prototype-kicker">THROWAWAY PROTOTYPE</span>
          <h1>Streamdown renderer evaluation</h1>
          <p>{VARIANTS[variant].description}</p>
        </div>
        <div className="prototype-actions">
          <button onClick={reset}>重置</button>
          <button className="prototype-primary" onClick={() => setPlaying((value) => !value)}>
            {playing ? '暂停' : streaming ? '播放流' : '已完成'}
          </button>
        </div>
      </header>

      <section className="prototype-controls">
        <label>
          进度
          <input
            type="range"
            min="0"
            max={SAMPLE.length}
            value={cursor}
            onChange={(event) => {
              setPlaying(false);
              setCursor(Number(event.target.value));
            }}
          />
          <span>{cursor} / {SAMPLE.length}</span>
        </label>
        <label>
          速度
          <select value={speed} onChange={(event) => setSpeed(Number(event.target.value))}>
            <option value="0.5">0.5×</option>
            <option value="1">1×</option>
            <option value="2">2×</option>
            <option value="4">4×</option>
          </select>
        </label>
      </section>

      <section className="prototype-metrics" aria-label="Live metrics">
        <div><strong>{renderSamples.length}</strong><span>React commits sampled</span></div>
        <div><strong>{stats.p95.toFixed(2)} ms</strong><span>commit p95</span></div>
        <div><strong>{stats.max.toFixed(2)} ms</strong><span>commit max</span></div>
        <div><strong>{resizeCountRef.current}</strong><span>resize callbacks</span></div>
        <div><strong>{mutationCountRef.current}</strong><span>DOM mutation batches</span></div>
      </section>

      <section className="prototype-stage">
        <div className="prototype-stage-label">
          <span>Assistant output</span>
          <span className={streaming ? 'is-streaming' : 'is-complete'}>
            {streaming ? 'streaming' : 'complete'}
          </span>
        </div>
        <article ref={surfaceRef} className="prototype-render-surface">
          <Renderer variant={variant} content={content || '等待开始…'} streaming={streaming} />
        </article>
      </section>

      <PrototypeSwitcher current={variant} />
    </main>
  );
}

export function renderStreamdownPrototype(root: Root): void {
  root.render(<StreamdownPrototype />);
}
