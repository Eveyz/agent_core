import React, { useState, useEffect, memo, useCallback, useMemo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import WrapTextIcon from 'lucide-react/dist/esm/icons/wrap-text.mjs';
import { getToolIcon } from './toolIcons';

function cleanTerminalOutput(text: string): string {
  if (!text) return '';

  // 1. Strip ANSI escape codes
  const ansiRegex = /[\u001b\u009b][[()#;?]*(?:[0-9]{1,4}(?:;[0-9]{0,4})*)?[0-9A-ORZcf-nqry=><]/g;
  let cleaned = text.replace(ansiRegex, '');

  // 2. Normalize \r\n to \n. After this, any remaining \r is a standalone carriage return.
  cleaned = cleaned.replace(/\r\n/g, '\n');

  // 3. Process carriage returns (\r) and backspaces (\x08) line by line
  const lines = cleaned.split('\n');
  const processedLines = lines.map(line => {
    if (!line.includes('\r') && !line.includes('\x08')) {
      return line;
    }

    const chars: string[] = [];
    let cursor = 0;

    for (let i = 0; i < line.length; i++) {
      const char = line[i];
      if (char === '\r') {
        cursor = 0;
      } else if (char === '\x08') {
        if (cursor > 0) {
          cursor--;
        }
      } else {
        chars[cursor] = char;
        cursor++;
      }
    }

    // Fill gaps with space
    for (let i = 0; i < chars.length; i++) {
      if (chars[i] === undefined) {
        chars[i] = ' ';
      }
    }

    return chars.join('');
  });

  return processedLines.join('\n');
}

const BashWidget = memo(function BashWidget({
  args,
  result,
  active,
  is_error,
  name,
}: {
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  name?: string;
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [copied, setCopied] = useState(false);
  const [wrapLines, setWrapLines] = useState(false);

  const cleanedResult = useMemo(() => {
    return cleanTerminalOutput(result || '');
  }, [result]);

  const toolName = name || 'bash';
  let command = (args as Record<string, unknown> | undefined)?.command as string || '';

  let customLabel: React.ReactNode = null;

  if (!command) {
    if (toolName === 'grep_search' || toolName === 'grep') {
      const q = ((args as Record<string, unknown> | undefined)?.Query || (args as Record<string, unknown> | undefined)?.query || (args as Record<string, unknown> | undefined)?.pattern || '') as string;
      const p = ((args as Record<string, unknown> | undefined)?.SearchPath || (args as Record<string, unknown> | undefined)?.search_path || (args as Record<string, unknown> | undefined)?.path || '') as string;
      command = `grep "${q}" ${p}`;
      const basename = p ? p.split('/').pop() : 'files';
      customLabel = <>{active ? 'Searching' : 'Searched'} <span className="bash-command">{basename}</span></>;
    } else if (toolName.includes('glob') || toolName.includes('find')) {
      const pat = ((args as Record<string, unknown> | undefined)?.Pattern || (args as Record<string, unknown> | undefined)?.pattern || '') as string;
      const p = ((args as Record<string, unknown> | undefined)?.SearchPath || (args as Record<string, unknown> | undefined)?.search_path || (args as Record<string, unknown> | undefined)?.path || (args as Record<string, unknown> | undefined)?.dir || '') as string;
      command = `find ${p} -name "${pat}"`;
      const basename = p ? p.split('/').pop() : 'directory';
      customLabel = <>{active ? 'Exploring' : 'Explored'} <span className="bash-command">{basename}</span></>;
    } else {
      command = typeof args === 'string' ? args : JSON.stringify(args);
    }
  }

  const displayCommand = command.length > 50 ? command.substring(0, 50) + '...' : command;
  
  const labelPrefix = active ? 'Running' : 'Ran';
  const ToolIcon = getToolIcon(toolName);

  const handleCopy = useCallback(async (e: React.MouseEvent) => {
    e.stopPropagation();
    try {
      await navigator.clipboard.writeText(cleanedResult || '');
      setCopied(true);
    } catch (error) {
      console.error('Failed to copy:', error);
    }
  }, [cleanedResult]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 2000);
    return () => clearTimeout(timer);
  }, [copied]);

  return (
    <div className="step-block bash-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''} step-row-pointer`}
        onClick={() => setCollapsed(!collapsed)}
      >
        <ToolIcon size={13} className="step-icon tool-icon-margin" color={is_error ? 'var(--danger)' : 'var(--text-muted)'} />
        <span className="step-label bash-label">
          {customLabel ? (
            customLabel
          ) : (
            <>{labelPrefix} <span className="bash-command">{displayCommand}</span></>
          )}
        </span>
        <div className="tool-controls-flex">
          {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
        </div>
      </div>
      {!collapsed && (
        <div className="bash-details-wrapper" style={{ marginLeft: '19px', marginTop: '8px', marginBottom: '8px', marginRight: '12px', borderRadius: '8px', background: 'var(--bg-secondary)', overflow: 'hidden', border: '1px solid var(--border-color)' }}>
          <div className="bash-details-header" style={{ padding: '8px 12px', borderBottom: '1px solid var(--border-color)' }}>
            <span style={{ fontSize: '12px', color: 'var(--text-muted)', fontWeight: 500 }}>Shell</span>
          </div>
          <div className="bash-details-body" style={{ fontSize: '12px', fontFamily: 'var(--font-mono)' }}>
            <div className="bash-details-command" style={{ padding: '12px', color: 'var(--text-primary)', whiteSpace: 'pre-wrap', wordBreak: 'break-word' }}>
              <span style={{ color: 'var(--text-muted)', marginRight: '8px' }}>$</span>
              {command}
            </div>
            {result && (
              <div className="bash-details-output-wrapper" style={{ position: 'relative', borderTop: '1px solid var(--border-color)' }}>
                <button
                  className="code-block-wrap-btn"
                  onClick={() => setWrapLines(!wrapLines)}
                  title={wrapLines ? "Unwrap lines (allow horizontal scroll)" : "Wrap lines"}
                  style={{
                    position: 'absolute',
                    top: '8px',
                    right: '40px',
                    display: 'flex',
                    background: wrapLines ? 'var(--overlay-0_08)' : 'var(--bg-secondary)',
                    border: '1px solid var(--border-color)',
                    borderRadius: '4px',
                    padding: '4px',
                    cursor: 'pointer',
                    color: wrapLines ? 'var(--accent)' : 'var(--text-muted)',
                    zIndex: 10
                  }}
                >
                  <WrapTextIcon size={14} />
                </button>
                <button
                  className="code-block-copy-btn"
                  onClick={handleCopy}
                  title="Copy output"
                  style={{
                    position: 'absolute',
                    top: '8px',
                    right: '12px',
                    display: 'flex',
                    background: 'var(--bg-secondary)',
                    border: '1px solid var(--border-color)',
                    borderRadius: '4px',
                    padding: '4px',
                    cursor: 'pointer',
                    color: 'var(--text-muted)',
                    zIndex: 10
                  }}
                >
                  {copied ? <CheckIcon size={14} color="var(--success)" /> : <CopyIcon size={14} />}
                </button>
                <div
                  className="bash-details-output"
                  style={{
                    padding: '12px',
                    paddingRight: '68px',
                    color: 'var(--text-muted)',
                    whiteSpace: wrapLines ? 'pre-wrap' : 'pre',
                    wordBreak: wrapLines ? 'break-word' : 'normal',
                    overflowX: wrapLines ? 'hidden' : 'auto',
                    maxHeight: '400px',
                    overflowY: 'auto',
                    background: 'var(--overlay-0_02)',
                    lineHeight: '1.6'
                  }}
                >
                  {cleanedResult}
                </div>
              </div>
            )}
            {!active && (
              <div className="bash-details-status" style={{ display: 'flex', justifyContent: 'flex-end', alignItems: 'center', padding: '8px 12px', color: is_error ? 'var(--error)' : 'var(--success)', fontWeight: 500, fontSize: '12px', borderTop: '1px solid var(--border-color)' }}>
                {is_error ? '✗ Fail' : '✓ Success'}
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
});

export default BashWidget;
