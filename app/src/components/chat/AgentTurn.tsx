import React, { useState, useEffect, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import AlertTriangleIcon from 'lucide-react/dist/esm/icons/alert-triangle.mjs';
import type { ChatEntry, TurnBlock } from '../../features/chat/chatSlice';
import { formatTime } from '../../utils/format';
import { MarkdownContent } from './MarkdownContent';
import ProcessingTimer from './ProcessingTimer';
import TurnIterationUI from './TurnIterationUI';
import TurnFooter from './TurnFooter';
import { isSubagentTool, groupBlocksIntoItems } from './turnHelpers';

export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: ChatEntry }) {
  const isProcessing = !!(entry.startTime && !entry.endTime);
  const isDone = !!(entry.endTime);

  // Auto-collapse intermediate steps when turn is done
  const [collapsed, setCollapsed] = useState(false);
  useEffect(() => {
    if (isDone) setCollapsed(true);
  }, [isDone]);

  const { toolCount, thoughtCount } = useMemo(() => {
    let tools = 0, thoughts = 0;
    entry.blocks?.forEach((b: TurnBlock) => {
      if (b.type === 'tool') {
        if (!isSubagentTool(b)) tools++;
      }
      if (b.type === 'thinking') thoughts++;
    });
    return { toolCount: tools, thoughtCount: thoughts };
  }, [entry.blocks]);

  const totalTimeText = useMemo(() => {
    if (!entry.startTime || !entry.endTime) return null;
    return formatTime(entry.endTime - entry.startTime);
  }, [entry.startTime, entry.endTime]);

  const summaryParts = useMemo(() => {
    const parts: string[] = [];
    if (totalTimeText) parts.push(totalTimeText);
    if (toolCount > 0) parts.push(`${toolCount} tool${toolCount > 1 ? 's' : ''}`);
    if (thoughtCount > 0) parts.push(`${thoughtCount} thought${thoughtCount > 1 ? 's' : ''}`);
    return parts;
  }, [totalTimeText, toolCount, thoughtCount]);

  // Check if there are any intermediate blocks at all
  const hasIntermediateSteps = toolCount > 0 || thoughtCount > 0;

  const renderItems = useMemo(() => groupBlocksIntoItems(entry.blocks || []), [entry.blocks]);
  const lastIterIdx = useMemo(() => {
    for (let i = renderItems.length - 1; i >= 0; i--) {
      if (renderItems[i].type === 'iteration') return i;
    }
    return -1;
  }, [renderItems]);

  return (
    <div className="agent-turn">
      {(hasIntermediateSteps || isProcessing) && (
        <>
          <div
            className={`turn-header ${isProcessing ? 'processing-pulse step-row-default' : 'step-row-pointer'}`}
            onClick={() => {
              if (!isProcessing) setCollapsed(!collapsed);
            }}
          >
            {isProcessing ? (
              <>
                <LoaderIcon className="tool-loader-icon" size={12} />
                <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
                <ChevronDownIcon size={12} className="ml-4" />
              </>
            ) : (
              <>
                <span>Worked {summaryParts.join(' · ')}</span>
                {collapsed ? <ChevronRightIcon size={12} className="ml-4" /> : <ChevronDownIcon size={12} className="ml-4" />}
              </>
            )}
          </div>
          <div className="turn-divider" />
        </>
      )}

      {renderItems.map((item, idx) => {
        return (
          <React.Fragment key={item.type === 'iteration' ? item.data.id : `block-${idx}`}>
            {item.type === 'assistant' ? (
              (!collapsed || lastIterIdx === -1 || idx > lastIterIdx) && (
                <MarkdownContent
                  content={item.data.text}
                  className="assistant-msg"
                  isStreaming={item.data.isStreaming}
                />
              )
            ) : item.type === 'error' ? (
              (!collapsed || idx === renderItems.length - 1) && (
                <div className="error-block-style">
                  <AlertTriangleIcon size={16} style={{ flexShrink: 0 }} />
                  <span style={{ lineHeight: '1.4' }}>{item.data.text}</span>
                </div>
              )
            ) : (
              !collapsed && <TurnIterationUI iteration={item.data} />
            )}
          </React.Fragment>
        );
      })}

      <TurnFooter entry={entry} />
    </div>
  );
});
