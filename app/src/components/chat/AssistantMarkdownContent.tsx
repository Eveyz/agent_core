import { lazy, memo, Suspense } from 'react';
import { MarkdownContent } from './MarkdownContent';
import { isStreamdownAssistantEnabled } from './assistantRendererFlag';

export interface AssistantMarkdownContentProps {
  content: string;
  className?: string;
  isStreaming?: boolean;
}

const streamdownEnabled = isStreamdownAssistantEnabled();

const LazyStreamdownAssistantContent = lazy(async () => {
  const module = await import('./StreamdownAssistantContent');
  return { default: module.StreamdownAssistantContent };
});

export const AssistantMarkdownContent = memo(function AssistantMarkdownContent({
  content,
  className,
  isStreaming = false,
}: AssistantMarkdownContentProps) {
  const currentRenderer = (
    <MarkdownContent
      className={className}
      content={content}
      isStreaming={isStreaming}
    />
  );

  if (!streamdownEnabled) return currentRenderer;

  return (
    <Suspense fallback={currentRenderer}>
      <LazyStreamdownAssistantContent
        className={className}
        content={content}
        isStreaming={isStreaming}
      />
    </Suspense>
  );
});
