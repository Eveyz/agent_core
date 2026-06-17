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

export const MarkdownContent = memo(function MarkdownContent({
  content,
  className,
}: {
  content: string;
  className?: string;
}) {
  const html = useMemo(() => parseMarkdown(content), [content]);
  return <div className={className} dangerouslySetInnerHTML={html} />;
});
