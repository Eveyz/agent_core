import { useMemo, memo } from 'react';
import DOMPurify from 'dompurify';
import { Marked } from 'marked';
import { CodeBlock } from './CodeBlock';

// ── Marked configuration ──────────────────────────────────────────────
const markedInstance = new Marked({
  gfm: true,
  breaks: true,
  async: false,
});

// ── DOMPurify configuration ──────────────────────────────────────────
const PURIFY_CONFIG = {
  ALLOWED_TAGS: [
    'p', 'br', 'hr', 'strong', 'em', 'del', 's', 'code', 'pre', 'blockquote',
    'ul', 'ol', 'li', 'table', 'thead', 'tbody', 'tr', 'th', 'td',
    'h1', 'h2', 'h3', 'h4', 'h5', 'h6', 'a', 'img', 'span', 'div',
    'sup', 'sub', 'mark',
  ],
  ALLOWED_ATTR: ['href', 'src', 'alt', 'title', 'class', 'target', 'rel', 'colspan', 'rowspan'],
  ALLOW_DATA_ATTR: false,
  FORBID_TAGS: ['style', 'script', 'iframe', 'object', 'embed', 'form', 'input'],
  FORBID_ATTR: ['style', 'onerror', 'onload', 'onclick', 'onmouseover'],
};

export function parseMarkdown(raw: string): { __html: string } {
  const html = markedInstance.parse(raw);
  const htmlStr = typeof html === 'string' ? html : '';
  return { __html: DOMPurify.sanitize(htmlStr, PURIFY_CONFIG) };
}

/**
 * Markdown renderer with custom code block rendering.
 *
 * Pass `plainText` to render content as preformatted text without any markdown
 * parsing (e.g. tool output that may contain markdown special chars like #).
 */
export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
  plainText = false,
}: {
  content: string;
  className?: string;
  plainText?: boolean;
}) {
  const trimmedContent = useMemo(() => content.trimEnd(), [content]);

  const handleClick = (e: React.MouseEvent) => {
    const target = e.target as HTMLElement;
    const a = target.closest('a');
    if (a && a.href) {
      e.preventDefault();
      import('@tauri-apps/plugin-opener')
        .then(({ openUrl }) => openUrl(a.href))
        .catch(console.error);
    }
  };

  // plainText mode: always render as preformatted text, no markdown.
  if (plainText) {
    return (
      <div
        className={className}
        style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}
        onClick={handleClick}
      >
        {trimmedContent}
      </div>
    );
  }

  // Parse markdown and extract code blocks for custom rendering
  const segments = useMemo(() => {
    const segments: Array<{ type: 'text' | 'code'; content: string; language?: string }> = [];
    const codeBlockRegex = /```(\w*)\n([\s\S]*?)```/g;
    let lastIndex = 0;
    let match;

    while ((match = codeBlockRegex.exec(trimmedContent)) !== null) {
      if (match.index > lastIndex) {
        const textContent = trimmedContent.substring(lastIndex, match.index);
        if (textContent.trim()) {
          segments.push({ type: 'text', content: textContent });
        }
      }

      segments.push({
        type: 'code',
        content: match[2],
        language: match[1] || 'plaintext',
      });

      lastIndex = match.index + match[0].length;
    }

    if (lastIndex < trimmedContent.length) {
      const remaining = trimmedContent.substring(lastIndex);
      const unclosedMatch = remaining.match(/```(\w*)\n([\s\S]*)$/);
      if (unclosedMatch) {
        const textBefore = remaining.substring(0, unclosedMatch.index);
        if (textBefore.trim()) {
          segments.push({ type: 'text', content: textBefore });
        }
        segments.push({
          type: 'code',
          content: unclosedMatch[2],
          language: unclosedMatch[1] || 'plaintext',
        });
      } else {
        if (remaining.trim()) {
          segments.push({ type: 'text', content: remaining });
        }
      }
    }

    return segments;
  }, [trimmedContent]);

  return (
    <div className={className} onClick={handleClick}>
      {segments.map((segment, idx) => {
        if (segment.type === 'code') {
          return (
            <CodeBlock
              key={`code-${idx}`}
              code={segment.content}
              language={segment.language || 'plaintext'}
            />
          );
        } else {
          const html = parseMarkdown(segment.content);
          return <div key={`text-${idx}`} dangerouslySetInnerHTML={html} />;
        }
      })}
    </div>
  );
});
