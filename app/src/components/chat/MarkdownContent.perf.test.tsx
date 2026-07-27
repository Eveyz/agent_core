// @vitest-environment jsdom
import { act } from 'react';
import { createRoot } from 'react-dom/client';
import { describe, expect, it } from 'vitest';
import { MarkdownContent } from './MarkdownContent';

const perfEnabled = (
  globalThis as typeof globalThis & { process?: { env?: Record<string, string | undefined> } }
).process?.env?.RUN_MARKDOWN_PERF === '1';

describe.runIf(perfEnabled)('MarkdownContent streaming frame benchmark', () => {
  it('keeps representative incremental renders within a 60 Hz frame at p95', () => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    const container = document.createElement('div');
    document.body.appendChild(container);
    const root = createRoot(container);
    const paragraph = [
      'Streaming **markdown** with [links](https://example.com), `inline code`,',
      'and enough prose to model a realistic answer without one giant active block.',
      '',
    ].join('\n');
    const finalMessage = paragraph.repeat(170);
    const samples: number[] = [];

    for (let end = 64; end <= finalMessage.length; end += 64) {
      const started = performance.now();
      act(() => {
        root.render(
          <MarkdownContent
            content={finalMessage.slice(0, end)}
            className="assistant-msg"
            isStreaming
          />,
        );
      });
      samples.push(performance.now() - started);
    }

    act(() => root.unmount());
    container.remove();
    samples.sort((a, b) => a - b);
    const p95 = samples[Math.floor(samples.length * 0.95)] ?? Number.POSITIVE_INFINITY;
    const max = samples[samples.length - 1] ?? Number.POSITIVE_INFINITY;
    console.log({
      finalChars: finalMessage.length,
      updates: samples.length,
      p95Ms: Number(p95.toFixed(2)),
      maxMs: Number(max.toFixed(2)),
    });
    expect(p95).toBeLessThan(16.67);
  });
});
