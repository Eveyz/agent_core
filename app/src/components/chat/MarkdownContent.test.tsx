// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { MarkdownContent } from './MarkdownContent';

describe('MarkdownContent streaming rendering', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    (globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function render(content: string, isStreaming = true) {
    act(() => {
      root.render(
        <MarkdownContent
          content={content}
          className="assistant-msg"
          isStreaming={isStreaming}
        />,
      );
    });
  }

  it('keeps completed blocks mounted while only the active tail grows', () => {
    render('First **complete** paragraph.\n\nActive tail');
    const message = container.querySelector('.assistant-msg');
    expect(message?.children).toHaveLength(2);
    const completedBlock = message?.children[0];
    expect(completedBlock?.querySelector('strong')?.textContent).toBe('complete');

    render('First **complete** paragraph.\n\nActive tail keeps growing');

    const updatedMessage = container.querySelector('.assistant-msg');
    expect(updatedMessage?.children[0]).toBe(completedBlock);
    expect(updatedMessage?.children[1]?.textContent).toContain('keeps growing');
  });

  it('preserves multi-line GFM structures as one rendered block', () => {
    render('- first\n\n- second\n\nTail');
    const message = container.querySelector('.assistant-msg');
    expect(message?.querySelectorAll('ul')).toHaveLength(1);
    expect(message?.querySelectorAll('li')).toHaveLength(2);
    expect(message?.children).toHaveLength(2);
  });

  it('renders headings, blockquotes, and GFM tables while streaming', () => {
    render([
      'Heading',
      '-------',
      '',
      '> quoted text',
      '',
      '| Name | Value |',
      '| --- | ---: |',
      '| Alpha | +12% |',
      '',
      'Tail',
    ].join('\n'));

    expect(container.querySelector('h2')?.textContent).toBe('Heading');
    expect(container.querySelector('blockquote')?.textContent).toContain('quoted text');
    expect(container.querySelector('table')).not.toBeNull();
    expect(container.querySelector('td.md-cell-pos')?.textContent).toContain('+12%');
  });

  it('promotes incomplete inline markup when its closing delimiter arrives', () => {
    render('**partial');
    expect(container.querySelector('strong')).toBeNull();

    render('**partial**');

    expect(container.querySelector('strong')?.textContent).toBe('partial');
  });

  it('resolves reference links using document-level definitions', () => {
    render('[OpenAI][site]\n\n[site]: https://openai.com\n\nTail');
    expect(container.querySelector('a')?.getAttribute('href')).toBe('https://openai.com');
  });

  it('invalidates only blocks affected by a changed reference definition', () => {
    render('No reference here.\n\n[OpenAI][site]\n\n[site]: https://one.example\n\nTail');
    const initialParagraphs = container.querySelectorAll('p');
    const unrelatedParagraph = initialParagraphs[0];
    const referencedParagraph = initialParagraphs[1];

    render('No reference here.\n\n[OpenAI][site]\n\n[site]: https://two.example\n\nTail');

    const updatedParagraphs = container.querySelectorAll('p');
    expect(updatedParagraphs[0]).toBe(unrelatedParagraph);
    expect(updatedParagraphs[1]).not.toBe(referencedParagraph);
    expect(updatedParagraphs[1].querySelector('a')?.getAttribute('href')).toBe('https://two.example');
  });

  it('renders an active incomplete code fence without starting CodeBlock highlighting', () => {
    render('```rust\nfn main() {');
    expect(container.querySelector('.code-block-wrapper')).toBeNull();
    expect(container.querySelector('pre')?.textContent).toContain('fn main() {');
  });

  it('waits for a following block before highlighting a closed streaming fence', () => {
    const code = '```rust\nfn main() {}\n```';
    render(code);
    expect(container.querySelector('.code-block-wrapper')).toBeNull();

    render(`${code}\n\nTail`);

    expect(container.querySelector('.code-block-wrapper')).not.toBeNull();
  });

  it('falls back to bounded plain text for a very long active block', () => {
    const content = `**${'x'.repeat(2_100)}`;
    render(content);
    expect(container.querySelector('[data-streaming-plain="true"]')?.textContent).toBe(content);
    expect(container.querySelector('strong')).toBeNull();
  });

  it('uses the canonical full-document renderer after streaming completes', () => {
    render('**finished**', false);
    expect(container.querySelector('strong')?.textContent).toBe('finished');
    expect(container.querySelector('[data-streaming-block]')).toBeNull();
  });

  it('sanitizes raw HTML in streaming blocks', () => {
    render('<img src="x" onerror="alert(1)">\n\n[unsafe](javascript:alert(1))\n\nTail');
    const image = container.querySelector('img');
    expect(image).not.toBeNull();
    expect(image?.hasAttribute('onerror')).toBe(false);
    expect(container.querySelector('a')?.hasAttribute('href')).toBe(false);
  });
});
