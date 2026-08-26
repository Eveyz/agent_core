import React, { useState, useEffect, useCallback, useMemo } from 'react';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import { highlightCode, normalizeHighlightSource } from './codeHighlighting';

// Language display name mapping
const LANGUAGE_NAMES: Record<string, string> = {
  rust: 'Rust',
  python: 'Python',
  javascript: 'JavaScript',
  typescript: 'TypeScript',
  json: 'JSON',
  toml: 'TOML',
  yaml: 'YAML',
  yml: 'YAML',
  html: 'HTML',
  css: 'CSS',
  sql: 'SQL',
  bash: 'Bash',
  sh: 'Shell',
  shell: 'Shell',
  powershell: 'PowerShell',
  ps1: 'PowerShell',
  pwsh: 'PowerShell',
  c: 'C',
  cpp: 'C++',
  'c++': 'C++',
  java: 'Java',
  go: 'Go',
  golang: 'Go',
  ruby: 'Ruby',
  php: 'PHP',
  swift: 'Swift',
  kotlin: 'Kotlin',
  scala: 'Scala',
  r: 'R',
  lua: 'Lua',
  perl: 'Perl',
  elixir: 'Elixir',
  erlang: 'Erlang',
  haskell: 'Haskell',
  clojure: 'Clojure',
  dart: 'Dart',
  protobuf: 'Protocol Buffer',
  proto: 'Protocol Buffer',
  xml: 'XML',
  markdown: 'Markdown',
  md: 'Markdown',
  makefile: 'Makefile',
  make: 'Makefile',
  cmake: 'CMake',
  dockerfile: 'Dockerfile',
  docker: 'Dockerfile',
  diff: 'Diff',
  'git-commit': 'Git Commit',
  git: 'Git',
  ini: 'INI',
  cfg: 'INI',
  conf: 'INI',
};

interface CodeBlockProps {
  code: string;
  language: string;
  className?: string;
}

export const CodeBlock: React.FC<CodeBlockProps> = ({ code, language, className = '' }) => {
  const [highlightedCode, setHighlightedCode] = useState<string>('');
  const [copied, setCopied] = useState(false);
  const [loading, setLoading] = useState(true);
  const displayCode = useMemo(() => normalizeHighlightSource(code), [code]);

  useEffect(() => {
    let mounted = true;
    setLoading(true);

    const lang = language.toLowerCase() || 'plaintext';
    highlightCode(displayCode, lang).then((html) => {
      if (mounted) {
        setHighlightedCode(html);
        setLoading(false);
      }
    });

    return () => {
      mounted = false;
    };
  }, [displayCode, language]);

  const handleCopy = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(displayCode);
      setCopied(true);
    } catch (error) {
      console.error('Failed to copy:', error);
    }
  }, [displayCode]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  const displayName = LANGUAGE_NAMES[language.toLowerCase()] || language || 'Plain Text';

  if (loading) {
    return (
      <div className={`code-block-wrapper ${className}`} data-highlight-loading="true">
        <div className="code-block-header">
          <span className="code-block-language">{displayName}</span>
          <button className="code-block-copy-btn" onClick={handleCopy}>
            {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
          </button>
        </div>
        <div className="code-block-content">
          <pre><code>{displayCode}</code></pre>
        </div>
      </div>
    );
  }

  return (
    <div className={`code-block-wrapper ${className}`} data-highlight-loading="false">
      <div className="code-block-header">
        <span className="code-block-language">{displayName}</span>
        <button className="code-block-copy-btn" onClick={handleCopy}>
          {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
        </button>
      </div>
      <div 
        className="code-block-content"
        dangerouslySetInnerHTML={{ __html: highlightedCode }}
      />
    </div>
  );
};
