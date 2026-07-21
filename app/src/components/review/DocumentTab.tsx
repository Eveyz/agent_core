import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import { MarkdownContent } from '../chat/MarkdownContent';

interface DocumentTabProps {
  projectPath: string | undefined;
  relativePaths: string[];
  title: string;
  placeholderMessage: string;
}

export function DocumentTab({ projectPath, relativePaths, title, placeholderMessage }: DocumentTabProps) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let isMounted = true;
    setLoading(true);

    const tryReadPaths = async () => {
      for (let i = 0; i < relativePaths.length; i++) {
        if (!isMounted) return;
        const candidate = relativePaths[i];
        const isAbsolute =
          candidate.startsWith('~/') ||
          candidate.startsWith('/') ||
          /^[A-Za-z]:[\\/]/.test(candidate);
        if (!isAbsolute && !projectPath) {
          continue;
        }
        const fullPath = isAbsolute ? candidate : `${projectPath}/${candidate}`;
        try {
          const fileContent = await invoke<string>('read_file', { path: fullPath });
          if (isMounted) {
            setContent(fileContent);
            setLoading(false);
          }
          return;
        } catch {
          // Try next path
        }
      }
      // All paths failed
      if (isMounted) {
        setContent(null);
        setLoading(false);
      }
    };

    tryReadPaths();

    return () => {
      isMounted = false;
    };
  }, [projectPath, relativePaths]);

  if (loading) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '100%', gap: '12px', color: 'var(--text-muted)' }}>
        <LoaderIcon className="animate-spin" size={20} style={{ color: 'var(--accent)' }} />
        <span>Loading {title}...</span>
      </div>
    );
  }

  if (!content) {
    return (
      <div style={{ display: 'flex', flexDirection: 'column', alignItems: 'center', justifyContent: 'center', height: '100%', padding: '24px', textAlign: 'center', color: 'var(--text-dim)', gap: '12px' }}>
        <FileTextIcon size={24} style={{ opacity: 0.5 }} />
        <div style={{ fontSize: '14px', fontWeight: 500, color: 'var(--text-muted)' }}>No {title} Found</div>
        <p style={{ fontSize: '12px', maxWidth: '280px', margin: 0, lineHeight: 1.5 }}>
          {placeholderMessage}
        </p>
      </div>
    );
  }

  return (
    <div className="document-tab-container" style={{ flex: 1, overflowY: 'auto', padding: '16px', display: 'flex', flexDirection: 'column' }}>
      <div className="markdown-body" style={{ fontSize: '13.5px', lineHeight: 1.6 }}>
        <MarkdownContent content={content} />
      </div>
    </div>
  );
}
