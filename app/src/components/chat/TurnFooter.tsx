import { useState, useMemo, useCallback, useEffect, memo } from 'react';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import type { ChatEntry, TurnBlock } from '../../features/chat/chatSlice';

const TurnFooter = memo(function TurnFooter({ entry }: { entry: ChatEntry }) {
  const [copied, setCopied] = useState(false);
  // PERF-7: Skip expensive text concatenation while streaming (no endTime).
  // The footer renders null during streaming anyway, so the computation is wasted.
  const isStreaming = !entry.endTime;
  const rawOutput = useMemo(() => {
    if (isStreaming || !entry.blocks) return '';
    return entry.blocks
      .filter((b): b is Extract<TurnBlock, { type: 'assistant' }> => b.type === 'assistant')
      .map((b) => b.text)
      .join('\n');
  }, [entry.blocks, isStreaming]);

  const endTimeText = useMemo(() => {
    if (!entry.endTime) return null;
    const d = new Date(entry.endTime);
    return d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit', second: '2-digit' });
  }, [entry.endTime]);

  const handleCopy = useCallback(async () => {
    if (!rawOutput) return;
    try {
      await navigator.clipboard.writeText(rawOutput);
      setCopied(true);
    } catch {
      // ignore
    }
  }, [rawOutput]);

  useEffect(() => {
    if (!copied) return;
    const timer = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(timer);
  }, [copied]);

  const isProcessing = !entry.endTime;

  if (isProcessing) {
    return (
      <div className="turn-footer turn-footer-processing">
        <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />
        <span className="turn-end-time">Working...</span>
      </div>
    );
  }

  if (!endTimeText && !rawOutput) return null;

  return (
    <div className="turn-footer">
      {rawOutput && !isProcessing && (
        <button className="turn-copy-btn" onClick={handleCopy} title="Copy Raw Assistant Output">
          {copied ? <CheckIcon size={11} color="var(--success)" /> : <CopyIcon size={11} />}
        </button>
      )}
      {endTimeText && <span className="turn-end-time">{endTimeText}</span>}
    </div>
  );
});

export default TurnFooter;
