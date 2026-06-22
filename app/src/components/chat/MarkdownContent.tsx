import { useMemo, memo } from 'react';
import DOMPurify from 'dompurify';
import { marked } from 'marked';

export function parseMarkdown(raw: string): { __html: string } {
  const html = marked.parse(raw) as string;
  return { __html: DOMPurify.sanitize(html) };
}

export function formatTime(ms: number): string {
  if (ms < 60000) return `${(ms / 1000).toFixed(1)}s`;
  const m = Math.floor(ms / 60000);
  const s = Math.floor((ms % 60000) / 1000);
  return `${m}m ${s}s`;
}

// While streaming, re-parsing the full markdown on every token is O(n²) in the
// message length and drops frames on long turns. We render plain text while the
// block is still streaming (cheap, and partial markdown reads fine as text),
// and only do the expensive parse+sanitize once the stream settles. If the
// streamed buffer gets very long we fall back to rendering it as markdown
// anyway so very long live output still looks reasonable.
const STREAM_PLAINTEXT_LIMIT = 4000;

/**
 * Markdown renderer with a streaming fast-path.
 *
 * Pass `isStreaming` for blocks whose `content` is still being appended to.
 * While streaming we skip markdown parsing entirely (plain text with preserved
 * whitespace); once streaming ends we parse once and memoize.
 */
export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
  isStreaming = false,
}: {
  content: string;
  className?: string;
  isStreaming?: boolean;
}) {
  const renderAsMarkdown = !isStreaming || content.length > STREAM_PLAINTEXT_LIMIT;
  const html = useMemo(
    () => (renderAsMarkdown ? parseMarkdown(content) : null),
    [renderAsMarkdown, content],
  );

  if (html) {
    return <div className={className} dangerouslySetInnerHTML={html} />;
  }
  // Streaming fast path: cheap plain-text render, no parse.
  return (
    <div className={className} style={{ whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
      {content}
    </div>
  );
});
