import { useMemo, memo } from 'react';
import DOMPurify from 'dompurify';
import { Marked, type Links, type Token, type TokensList } from 'marked';
import { CodeBlock } from './CodeBlock';

// ── Marked configuration ──────────────────────────────────────────────
const markedInstance = new Marked({
  gfm: true,
  breaks: true,
  async: false,
});

const MAX_ACTIVE_RICH_BLOCK_CHARS = 2_048;

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
  return sanitizeMarkdownHtml(htmlStr);
}

function sanitizeMarkdownHtml(html: string): { __html: string } {
  const sanitized = DOMPurify.sanitize(html, PURIFY_CONFIG);
  return { __html: enhanceMarkdownTables(sanitized) };
}

/** Wrap tables for scroll/frame, and softly tint signed % cells. */
function enhanceMarkdownTables(html: string): string {
  let out = html
    .replace(/<table(\s[^>]*)?>/gi, '<div class="md-table-wrap"><table$1>')
    .replace(/<\/table>/gi, '</table></div>');

  out = out.replace(
    /<td([^>]*)>(\s*[+-]\d+(?:\.\d+)?%\s*)<\/td>/gi,
    (_match, attrs: string, content: string) => {
      const cls = content.trim().startsWith('-') ? 'md-cell-neg' : 'md-cell-pos';
      const nextAttrs = /class\s*=/i.test(attrs)
        ? attrs.replace(/class\s*=\s*(["'])/i, `class=$1${cls} `)
        : `${attrs} class="${cls}"`;
      return `<td${nextAttrs}>${content}</td>`;
    },
  );

  return out;
}

interface MarkdownContentProps {
  content: string;
  className?: string;
  plainText?: boolean;
  isStreaming?: boolean;
}

interface StreamingMarkdownBlock {
  type: string;
  raw: string;
  token: Token;
  active: boolean;
  definitionsSignature: string;
}

interface StreamingBlockProps {
  block: StreamingMarkdownBlock;
  links: Links;
  index: number;
}

function parseMarkdownToken(token: Token, links: Links): { __html: string } {
  const tokens = [token] as TokensList;
  tokens.links = links;
  const html = markedInstance.parser(tokens);
  return sanitizeMarkdownHtml(typeof html === 'string' ? html : '');
}

function StreamingPlainBlock({ text }: { text: string }) {
  return (
    <pre
      data-streaming-plain="true"
      style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}
    >
      {text}
    </pre>
  );
}

const StreamingBlock = memo(function StreamingBlock({
  block,
  links,
  index,
}: StreamingBlockProps) {
  if (block.type === 'code') {
    const code = 'text' in block.token && typeof block.token.text === 'string'
      ? block.token.text
      : block.raw;
    const language = 'lang' in block.token && typeof block.token.lang === 'string'
      ? block.token.lang
      : 'plaintext';
    if (block.active) {
      return (
        <div data-streaming-block={index}>
          <StreamingPlainBlock text={code} />
        </div>
      );
    }
    return (
      <div data-streaming-block={index}>
        <CodeBlock code={code} language={language || 'plaintext'} />
      </div>
    );
  }

  if (block.active && block.raw.length > MAX_ACTIVE_RICH_BLOCK_CHARS) {
    return (
      <div data-streaming-block={index}>
        <StreamingPlainBlock text={block.raw} />
      </div>
    );
  }

  try {
    return (
      <div
        data-streaming-block={index}
        dangerouslySetInnerHTML={parseMarkdownToken(block.token, links)}
      />
    );
  } catch {
    return (
      <div data-streaming-block={index}>
        <StreamingPlainBlock text={block.raw} />
      </div>
    );
  }
}, (prev, next) => (
  prev.block.type === next.block.type
  && prev.block.raw === next.block.raw
  && prev.block.active === next.block.active
  && prev.block.definitionsSignature === next.block.definitionsSignature
  && prev.index === next.index
));

function normalizeReferenceLabel(label: string): string {
  return label.trim().replace(/\s+/g, ' ').toLowerCase();
}

function referenceLabel(raw: string): string | null {
  const source = raw.startsWith('!') ? raw.slice(1) : raw;
  const full = source.match(/^\[([^\]]*)\]\[([^\]]*)\]$/);
  if (full) return normalizeReferenceLabel(full[2] || full[1]);
  const shortcut = source.match(/^\[([^\]]+)\]$/);
  return shortcut ? normalizeReferenceLabel(shortcut[1]) : null;
}

function referencedDefinitionsSignature(token: Token, links: Links): string {
  const referenced = new Set<string>();
  markedInstance.walkTokens([token], (child) => {
    if (child.type !== 'link' && child.type !== 'image') return;
    const label = referenceLabel(child.raw);
    if (label && links[label]) referenced.add(label);
  });
  return JSON.stringify(
    [...referenced]
      .sort((a, b) => a.localeCompare(b))
      .map((key) => [key, links[key].href, links[key].title]),
  );
}

function StreamingMarkdownContent({ content }: { content: string }) {
  const parsed = useMemo(() => {
    try {
      const tokens = markedInstance.lexer(content);
      const visible = tokens.filter((token) => token.type !== 'space' && token.type !== 'def');
      const lastIndex = visible.length - 1;
      const blocks: StreamingMarkdownBlock[] = visible.map((token, index) => ({
        type: token.type,
        raw: token.raw,
        token,
        active: index === lastIndex,
        definitionsSignature: referencedDefinitionsSignature(token, tokens.links),
      }));
      return {
        blocks,
        links: tokens.links,
        failed: false,
      };
    } catch {
      return {
        blocks: [],
        links: {} as Links,
        failed: true,
      };
    }
  }, [content]);

  if (parsed.failed) {
    return <StreamingPlainBlock text={content} />;
  }

  return (
    <>
      {parsed.blocks.map((block, index) => (
        <StreamingBlock
          key={index}
          block={block}
          links={parsed.links}
          index={index}
        />
      ))}
    </>
  );
}

function StaticMarkdownContent({ content }: { content: string }) {
  // Parse markdown and extract code blocks for custom rendering
  const segments = useMemo(() => {
    const segments: Array<{ type: 'text' | 'code'; content: string; language?: string }> = [];
    const codeBlockRegex = /```(\w*)\n([\s\S]*?)```/g;
    let lastIndex = 0;
    let match;

    while ((match = codeBlockRegex.exec(content)) !== null) {
      if (match.index > lastIndex) {
        const textContent = content.substring(lastIndex, match.index);
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

    if (lastIndex < content.length) {
      const remaining = content.substring(lastIndex);
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
  }, [content]);

  return (
    <>
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
    </>
  );
}

/**
 * Markdown renderer with custom code block rendering.
 *
 * Pass `plainText` to render content as preformatted text without any markdown
 * parsing (e.g. tool output that may contain markdown special chars like #).
 * While `isStreaming`, completed top-level blocks stay memoized and only the
 * active tail is reparsed.
 */
export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
  plainText = false,
  isStreaming = false,
}: MarkdownContentProps) {
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

  return (
    <div className={className} onClick={handleClick}>
      {isStreaming
        ? <StreamingMarkdownContent content={trimmedContent} />
        : <StaticMarkdownContent content={trimmedContent} />}
    </div>
  );
});
