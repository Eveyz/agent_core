import { memo, useCallback } from 'react';
import { Streamdown, type CodeHighlighterPlugin } from 'streamdown';
import { createMathPlugin } from '@streamdown/math';
import { CORE_LANGS, highlightCodeTokens } from './codeHighlighting';
import type { AssistantMarkdownContentProps } from './AssistantMarkdownContent';
import 'streamdown/styles.css';
import 'katex/dist/katex.min.css';
import './streamdown-assistant.css';

const math = createMathPlugin({ singleDollarTextMath: true });
export const streamdownCodePlugin: CodeHighlighterPlugin = {
  name: 'shiki',
  type: 'code-highlighter',
  getSupportedLanguages: () => [...CORE_LANGS] as ReturnType<CodeHighlighterPlugin['getSupportedLanguages']>,
  getThemes: () => ['vitesse-light', 'vitesse-dark'],
  supportsLanguage: () => true,
  highlight: ({ code, language }, callback) => {
    highlightCodeTokens(code, language)
      .then((result) => callback?.(result))
      .catch(console.error);
    return null;
  },
};
const plugins = { code: streamdownCodePlugin, math };

export const StreamdownAssistantContent = memo(function StreamdownAssistantContent({
  content,
  className,
  isStreaming = false,
}: AssistantMarkdownContentProps) {
  const handleClick = useCallback((event: React.MouseEvent<HTMLDivElement>) => {
    const anchor = (event.target as HTMLElement).closest('a');
    if (!anchor?.href) return;
    event.preventDefault();
    import('@tauri-apps/plugin-opener')
      .then(({ openUrl }) => openUrl(anchor.href))
      .catch(console.error);
  }, []);

  return (
    <div className={`${className ?? ''} streamdown-assistant`.trim()} onClick={handleClick}>
      <Streamdown
        animated={false}
        controls={false}
        dir="auto"
        isAnimating={isStreaming}
        lineNumbers={false}
        linkSafety={{ enabled: false }}
        mode={isStreaming ? 'streaming' : 'static'}
        plugins={plugins}
        shikiTheme={['vitesse-light', 'vitesse-dark']}
      >
        {content.trimEnd()}
      </Streamdown>
    </div>
  );
});
