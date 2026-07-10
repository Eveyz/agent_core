import { useState, useEffect, useMemo, memo } from 'react';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import type { TurnBlock } from '../../features/chat/chatSlice';
import { formatTime } from '../../utils/format';
import type { TurnIteration, ApprovalBlock, SubagentRefBlock } from './turnHelpers';
import { isSubagentTool, isSubagentRefBlock } from './turnHelpers';
import ToolBlockUI from './ToolBlockUI';
import EditFileWidget from './EditFileWidget';
import ReadFileWidget from './ReadFileWidget';
import BashWidget from './BashWidget';
import SubagentSpawnWidget, { SubagentCard } from './SubagentWidgets';
import ClarificationOverlay from './ClarificationOverlay';
import { generateSmartToolsLabel } from './turnHelpers';
import { useTranslation } from 'react-i18next';

const TurnIterationUI = memo(function TurnIterationUI({
  iteration,
}: {
  iteration: TurnIteration;
}) {
  const { t } = useTranslation();
  const thinkingBlock = iteration.thinkingBlock;
  const isStreaming = !!thinkingBlock?.isStreaming;

  const [thoughtCollapsed, setThoughtCollapsed] = useState(!isStreaming && !iteration.isLast);
  const [toolsCollapsed, setToolsCollapsed] = useState(!isStreaming && !iteration.isLast);

  useEffect(() => {
    if (!isStreaming && !iteration.isLast) {
      setThoughtCollapsed(true);
      setToolsCollapsed(true);
    } else {
      setThoughtCollapsed(false);
      setToolsCollapsed(false);
    }
  }, [isStreaming, iteration.isLast]);

  const regularToolCount = useMemo(() => {
    return iteration.toolBlocks.filter(b => b.type === 'tool' && !isSubagentTool(b)).length;
  }, [iteration.toolBlocks]);

  const hasThinkingContent = thinkingBlock?.text && thinkingBlock.text.trim().length > 0;

  const hasRegularTools = iteration.toolBlocks.some(b => (b.type === 'tool' && !isSubagentTool(b)) || (b.type === 'approval' && b.status === 'pending'));
  const hasSubagents = iteration.toolBlocks.some(b => (b.type === 'tool' && isSubagentTool(b)) || b.type === 'subagent_ref');

  const thoughtLabel = useMemo(() => {
    if (isStreaming) return t('chat.thinking');
    if (thinkingBlock?.startTime && thinkingBlock?.endTime) {
      return t('chat.thoughtFor', { time: formatTime(thinkingBlock.endTime - thinkingBlock.startTime) });
    }
    return t('chat.thought');
  }, [isStreaming, thinkingBlock, t]);

  const toolsLabel = useMemo(() => {
    return generateSmartToolsLabel(iteration.toolBlocks, isStreaming, t);
  }, [iteration.toolBlocks, isStreaming, t]);

  const singleTopLevelTool = useMemo(() => {
    if (regularToolCount !== 1) return false;
    const regularTools = iteration.toolBlocks.filter(b => b.type === 'tool' && !isSubagentTool(b));
    if (regularTools.length !== 1) return false;
    const name = (regularTools[0] as Extract<TurnBlock, { type: 'tool' }>).name;
    return name === 'edit' || name === 'read_file' || name === 'bash' || name === 'grep_search' || name === 'glob_search' || name === 'grep' || name === 'glob' || name.startsWith('todo_') || name === 'write_file' || name === 'write_to_file' || name.startsWith('skill_') || name === 'archival_memory_search' || name === 'conversation_search';
  }, [regularToolCount, iteration.toolBlocks]);

  const renderRegularTools = () => (
    <>
      {iteration.toolBlocks.map((b: TurnBlock, idx: number) => {
        if (b.type === 'tool') {
          if (isSubagentTool(b)) return null;
          const name = b.name;
          const approvalBlock = iteration.toolBlocks.find(
            (tb): tb is ApprovalBlock => tb.type === 'approval' && tb.tool_name === name && tb.status !== 'pending'
          );
          const approvalStatus: 'approved' | 'denied' | undefined = approvalBlock?.status as 'approved' | 'denied' | undefined;

          if (name === 'edit') {
            if (b.phase === 'preparing') {
              return (
                <ToolBlockUI
                  key={b.call_id || idx}
                  name={name}
                  args={b.args}
                  result={b.result}
                  active={b.active}
                  is_error={b.is_error}
                  startTime={b.startTime}
                  endTime={b.endTime}
                  phase={b.phase}
                  hint_path={b.hint_path}
                />
              );
            }
            return (
              <EditFileWidget
                key={b.call_id || idx}
                args={b.args}
                result={b.result}
                active={b.active}
                is_error={b.is_error}
              />
            );
          } else if (name === 'read_file') {
            if (b.phase === 'preparing') {
              return (
                <ToolBlockUI
                  key={b.call_id || idx}
                  name={name}
                  args={b.args}
                  result={b.result}
                  active={b.active}
                  is_error={b.is_error}
                  startTime={b.startTime}
                  endTime={b.endTime}
                  phase={b.phase}
                  hint_path={b.hint_path}
                />
              );
            }
            return (
              <ReadFileWidget
                key={b.call_id || idx}
                args={b.args}
                active={b.active}
                is_error={b.is_error}
              />
            );
          } else if (name === 'bash' || name === 'grep_search' || name === 'glob_search' || name === 'grep' || name === 'glob') {
            if (b.phase === 'preparing') {
              return (
                <ToolBlockUI
                  key={b.call_id || idx}
                  name={name}
                  args={b.args}
                  result={b.result}
                  active={b.active}
                  is_error={b.is_error}
                  startTime={b.startTime}
                  endTime={b.endTime}
                  phase={b.phase}
                  hint_path={b.hint_path}
                />
              );
            }
            return (
              <BashWidget
                key={b.call_id || idx}
                args={b.args}
                result={b.result}
                active={b.active}
                is_error={b.is_error}
                name={name}
              />
            );
          }
          return (
            <ToolBlockUI
              key={b.call_id || idx}
              name={name}
              args={b.args}
              result={b.result}
              active={b.active}
              is_error={b.is_error}
              startTime={b.startTime}
              endTime={b.endTime}
              approvalStatus={approvalStatus}
              phase={b.phase}
              hint_path={b.hint_path}
            />
          );
        } else if (b.type === 'approval') {
          return null;
        } else if (b.type === 'clarification') {
          // Pending clarifications render as overlay in App.tsx; answered ones show inline.
          if (b.status === 'pending') return null;
          return <ClarificationOverlay key={b.prompt_id || idx} block={b} />;
        }
        return null;
      })}
    </>
  );

  const renderSubagentTools = () => (
    <>
      {iteration.toolBlocks.map((b: TurnBlock, idx: number) => {
        if (b.type === 'tool' && isSubagentTool(b)) {
          // Link subagent_ref blocks to their parent tool by call_id (via
          // parent_call_id on the ref). Falls back to showing all refs on
          // the first subagent tool when parent_call_id is absent (legacy).
          const refs = b.call_id
            ? iteration.toolBlocks.filter(
                (tb): tb is SubagentRefBlock => isSubagentRefBlock(tb) && tb.parent_call_id === b.call_id
              )
            : iteration.toolBlocks.filter(isSubagentRefBlock);
          return (
            <SubagentSpawnWidget
              key={b.call_id || idx}
              args={b.args}
              active={b.active}
              subagentRefs={refs}
            />
          );
        }
        // subagent_ref blocks are now rendered inline by SubagentSpawnWidget;
        // if a ref arrives without a matching wrapper, show the card directly.
        if (b.type === 'subagent_ref') {
          const hasSpawnWidget = iteration.toolBlocks.some(isSubagentTool);
          if (hasSpawnWidget) return null;

          return (
            <div key={`subagent-${b.subagent_id}-${idx}`} className="subagents-section">
              <SubagentCard subagentId={b.subagent_id} />
            </div>
          );
        }
        return null;
      })}
    </>
  );

  return (
    <>
      {hasThinkingContent && (
        <div className="step-block">
          <div
            className={`step-row ${isStreaming ? 'step-row-active' : ''}`}
            onClick={() => setThoughtCollapsed(!thoughtCollapsed)}
          >
            <BrainIcon size={13} className={`step-icon ${isStreaming ? 'step-icon-thinking' : ''}`} color={isStreaming ? undefined : "var(--text-tertiary)"} />
            <span className="step-label step-label-bold">{thoughtLabel}</span>
            {thoughtCollapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
          </div>
          {!thoughtCollapsed && (
            <div className="iteration-body">
              <div className="thinking-block">
                {thinkingBlock.text}
                {isStreaming ? <span className="typing-dot typing-dot-style">...</span> : null}
              </div>
            </div>
          )}
        </div>
      )}

      {hasRegularTools && (
        singleTopLevelTool ? (
          <div className={hasThinkingContent ? 'mt-4' : 'mt-0'}>
            {renderRegularTools()}
          </div>
        ) : (
          <div className={`step-block ${hasThinkingContent ? 'mt-4' : 'mt-0'}`}>
            <div
              className={`step-row ${isStreaming ? 'step-row-active' : ''}`}
              onClick={() => setToolsCollapsed(!toolsCollapsed)}
            >
              <WrenchIcon size={13} className="step-icon" color={isStreaming ? undefined : "var(--text-tertiary)"} />
              <span className="step-label step-label-bold">{toolsLabel}</span>
              {toolsCollapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
            </div>
            {!toolsCollapsed && (
              <div className="iteration-body">
                {renderRegularTools()}
              </div>
            )}
          </div>
        )
      )}

      {hasSubagents && (
        <div className={hasThinkingContent || hasRegularTools ? 'mt-4' : 'mt-0'}>
          {renderSubagentTools()}
        </div>
      )}
    </>
  );
}, (prev, next) => {
  return prev.iteration.id === next.iteration.id &&
         prev.iteration.isLast === next.iteration.isLast &&
         prev.iteration.thinkingBlock === next.iteration.thinkingBlock &&
         prev.iteration.toolBlocks.length === next.iteration.toolBlocks.length &&
         prev.iteration.toolBlocks.every((b, i) => b === next.iteration.toolBlocks[i]);
});

export default TurnIterationUI;
