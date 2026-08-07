// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { StreamdownAssistantContent } from './StreamdownAssistantContent';

describe('StreamdownAssistantContent', () => {
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

  async function render(content: string, isStreaming: boolean) {
    await act(async () => {
      root.render(
        <StreamdownAssistantContent
          className="assistant-msg"
          content={content}
          isStreaming={isStreaming}
        />,
      );
    });
  }

  it('repairs incomplete markdown while streaming', async () => {
    await render('Start **partial', true);

    expect(container.textContent).toContain('partial');
    expect(container.querySelector('[data-streamdown="strong"]')).not.toBeNull();
  });

  it('blocks unsafe raw HTML and keeps the assistant class', async () => {
    await render('<img src="x" onerror="alert(1)" alt="unsafe">', false);

    expect(container.querySelector('.assistant-msg')).not.toBeNull();
    expect(container.querySelector('img')).toBeNull();
    expect(container.textContent).toContain('Image blocked');
  });
});
