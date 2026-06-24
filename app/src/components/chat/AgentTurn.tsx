import React, { useState, useEffect, memo, useMemo, useCallback } from 'react';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { useSelector } from 'react-redux';
import type { RootState } from '../../store';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import LoaderIcon from 'lucide-react/dist/esm/icons/loader.mjs';
import CopyIcon from 'lucide-react/dist/esm/icons/copy.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import WrenchIcon from 'lucide-react/dist/esm/icons/wrench.mjs';
import FileIcon from 'lucide-react/dist/esm/icons/file.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import EyeIcon from 'lucide-react/dist/esm/icons/eye.mjs';
import TerminalIcon from 'lucide-react/dist/esm/icons/terminal.mjs';
import GlobeIcon from 'lucide-react/dist/esm/icons/globe.mjs';
import GitBranchIcon from 'lucide-react/dist/esm/icons/git-branch.mjs';
import GitCommitIcon from 'lucide-react/dist/esm/icons/git-commit.mjs';
import GitCompareIcon from 'lucide-react/dist/esm/icons/git-compare.mjs';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import FolderSearchIcon from 'lucide-react/dist/esm/icons/folder-search.mjs';
import FileSearchIcon from 'lucide-react/dist/esm/icons/file-search.mjs';
import SaveIcon from 'lucide-react/dist/esm/icons/save.mjs';
import WandIcon from 'lucide-react/dist/esm/icons/wand.mjs';
import BookOpenIcon from 'lucide-react/dist/esm/icons/book-open.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import ReplaceIcon from 'lucide-react/dist/esm/icons/replace.mjs';
import ScanTextIcon from 'lucide-react/dist/esm/icons/scan-text.mjs';
import ListTodoIcon from 'lucide-react/dist/esm/icons/list-todo.mjs';
import CalendarIcon from 'lucide-react/dist/esm/icons/calendar.mjs';
import UsersIcon from 'lucide-react/dist/esm/icons/users.mjs';
import AlertTriangleIcon from 'lucide-react/dist/esm/icons/alert-triangle.mjs';
import { invoke } from '@tauri-apps/api/core';
import { toolApprovalResponded, viewSubagent } from '../../features/chat/chatSlice';
import type { ChatEntry, TurnBlock, SubagentEntry } from '../../features/chat/chatSlice';
import { MarkdownContent, formatTime, parseMarkdown } from './MarkdownContent';

export { formatTime, parseMarkdown };

const ml4 = { marginLeft: '4px' };

// ── Shared style constants (P1-5: avoid inline objects that break memo) ──
const iterationBodyStyle: React.CSSProperties = {
  marginLeft: '6px',
  paddingLeft: '12px',
  borderLeft: '1px solid var(--text-muted)',
  display: 'flex',
  flexDirection: 'column',
  gap: '8px',
  marginTop: '6px',
  paddingBottom: '4px',
};
const stepLabelBold: React.CSSProperties = { fontWeight: 500 };
const toolIconMargin: React.CSSProperties = { marginRight: '5px' };
const approvalBadgeStyle: React.CSSProperties = { fontSize: '10px', padding: '1px 6px', fontWeight: 'bold' };
const approvalActionsStyle: React.CSSProperties = { display: 'flex', gap: '8px', flexWrap: 'wrap' };
const spawnBlockChildrenStyle: React.CSSProperties = { marginLeft: '16px', marginTop: '4px', display: 'flex', flexDirection: 'column', gap: '4px' };
const typingDotStyle: React.CSSProperties = { display: 'inline-block', marginLeft: '4px' };
const errorBlockStyle: React.CSSProperties = {
  color: '#ef4444',
  fontSize: '13px',
  padding: '10px 14px',
  background: 'var(--danger-bg, rgba(239,68,68,0.06))',
  border: '1px solid var(--danger-border, rgba(239,68,68,0.2))',
  borderRadius: '8px',
  marginTop: '12px',
  marginBottom: '12px',
  display: 'flex',
  alignItems: 'center',
  gap: '10px',
};

// ── Per-tool icon mapping ───────────────────────────────────────────
// Each tool gets a distinct icon so the user can tell at a glance what the
// agent is doing. Falls back to WrenchIcon for unknown tools.
const TOOL_ICONS: Record<string, React.ComponentType<{ size?: number; color?: string; className?: string; style?: React.CSSProperties }>> = {
  bash: TerminalIcon,
  edit: FileIcon,
  sed: ScanTextIcon,
  read_file: EyeIcon,
  write_file: SaveIcon,
  glob: FolderSearchIcon,
  grep: FileSearchIcon,
  git_status: GitBranchIcon,
  git_diff: GitCompareIcon,
  git_commit: GitCommitIcon,
  git_log: GitCompareIcon,
  git_show: GitCompareIcon,
  webfetch: GlobeIcon,
  tavily_search: SearchIcon,
  subagent: UsersIcon,
  subagents: UsersIcon,
  invoke_subagent: UsersIcon,
  core_memory_read: DatabaseIcon,
  core_memory_append: PlusIcon,
  core_memory_replace: ReplaceIcon,
  archival_memory_search: SearchIcon,
  archival_memory_insert: PlusIcon,
  archival_memory_delete: TrashIcon,
  conversation_search: SearchIcon,
  conversation_search_date: CalendarIcon,
  skill_load: WandIcon,
  skill_reload: WandIcon,
  skill_deactivate: WandIcon,
  skill_list: BookOpenIcon,
  todo_read: ListTodoIcon,
  todo_write: ListTodoIcon,
  todo_update: ListTodoIcon,
};

function getToolIcon(name: string): React.ComponentType<{ size?: number; color?: string; className?: string; style?: React.CSSProperties }> {
  return TOOL_ICONS[name] || WrenchIcon;
}



const ProcessingTimer = memo(function ProcessingTimer({
  startTime,
  endTime,
}: {
  startTime?: number;
  endTime?: number;
}) {
  const [now, setNow] = useState(Date.now());

  useEffect(() => {
    if (!startTime || endTime) return;
    const interval = setInterval(() => setNow(Date.now()), 250);
    return () => clearInterval(interval);
  }, [startTime, endTime]);

  if (!startTime) return null;
  if (endTime) return <span>Processed {formatTime(endTime - startTime)}</span>;
  const diff = now - startTime;
  return <span>Processed {formatTime(diff)}</span>;
});

type ThinkingBlock = Extract<TurnBlock, { type: 'thinking' }>;
type AssistantBlock = Extract<TurnBlock, { type: 'assistant' }>;
type ApprovalBlock = Extract<TurnBlock, { type: 'approval' }>;
type SubagentRefBlock = Extract<TurnBlock, { type: 'subagent_ref' }>;

const SUBAGENT_TOOL_NAMES = ['subagent', 'subagents', 'invoke_subagent'];

function isSubagentTool(b: TurnBlock): boolean {
  return b.type === 'tool' && SUBAGENT_TOOL_NAMES.includes(b.name);
}

function isSubagentRefBlock(b: TurnBlock): b is SubagentRefBlock {
  return b.type === 'subagent_ref';
}

interface TurnIteration {
  id: string;
  thinkingBlock?: ThinkingBlock;
  toolBlocks: TurnBlock[];
  isLast: boolean;
}

export type TurnRenderItem =
  | { type: 'iteration'; data: TurnIteration }
  | { type: 'assistant'; data: AssistantBlock }
  | { type: 'error'; data: Extract<TurnBlock, { type: 'error' }> };

function groupBlocksIntoItems(blocks: TurnBlock[]): TurnRenderItem[] {
  const items: TurnRenderItem[] = [];
  let currentIter: TurnIteration | null = null;
  
  const pushCurrentIter = () => {
    if (currentIter) {
      items.push({ type: 'iteration', data: currentIter });
      currentIter = null;
    }
  };

  blocks.forEach((b, idx) => {
    if (b.type === 'assistant') {
      pushCurrentIter();
      items.push({ type: 'assistant', data: b as AssistantBlock });
      return;
    }
    
    if (b.type === 'error') {
      pushCurrentIter();
      items.push({ type: 'error', data: b });
      return;
    }
    
    if (b.type === 'thinking') {
      pushCurrentIter();
      currentIter = { id: `iter-${idx}`, thinkingBlock: b as ThinkingBlock, toolBlocks: [], isLast: false };
    } else {
      if (!currentIter) currentIter = { id: `iter-init-${idx}`, toolBlocks: [], isLast: false };
      currentIter.toolBlocks.push(b);
    }
  });
  
  pushCurrentIter();
  
  const lastIter = items.slice().reverse().find(i => i.type === 'iteration');
  if (lastIter && lastIter.type === 'iteration') {
    lastIter.data.isLast = true;
  }
  
  return items;
}

/** Count subagents from the tool args: single = 1, batch = tasks.length. */
function countSpawnedAgents(args?: unknown): number {
  if (!args || typeof args !== 'object') return 1;
  const obj = args as Record<string, unknown>;
  if (Array.isArray(obj.tasks)) return obj.tasks.length;
  return 1;
}

const TurnIterationUI = memo(function TurnIterationUI({
  iteration,
  subagents,
}: {
  iteration: TurnIteration;
  subagents?: Record<string, SubagentEntry>;
}) {
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

  const toolCount = useMemo(() => {
    return iteration.toolBlocks.filter(b => b.type === 'tool').length;
  }, [iteration.toolBlocks]);

  const subagentTools = useMemo(() => {
    return iteration.toolBlocks.filter(isSubagentTool);
  }, [iteration.toolBlocks]);

  const hasThinkingContent = thinkingBlock?.text && thinkingBlock.text.trim().length > 0;
  const hasTools = iteration.toolBlocks.length > 0;
  
  const hasOnlySubagents = toolCount > 0 && toolCount === subagentTools.length;

  const thoughtLabel = useMemo(() => {
    if (isStreaming) return 'Thinking...';
    if (thinkingBlock?.startTime && thinkingBlock?.endTime) {
      return `Thought for ${formatTime(thinkingBlock.endTime - thinkingBlock.startTime)}`;
    }
    return 'Thought';
  }, [isStreaming, thinkingBlock]);

  const toolsLabel = useMemo(() => {
    if (isStreaming) {
      return toolCount > 0 ? `Calling ${toolCount} tool${toolCount > 1 ? 's' : ''}...` : 'Calling tool...';
    } else {
      return toolCount > 0 ? `Called ${toolCount} tool${toolCount > 1 ? 's' : ''}` : 'Called tool';
    }
  }, [isStreaming, toolCount]);

  const renderToolBlocks = () => (
    <>
      {iteration.toolBlocks.map((b: TurnBlock, idx: number) => {
        if (b.type === 'tool') {
          const name = b.name;
          const approvalBlock = iteration.toolBlocks.find(
            (tb): tb is ApprovalBlock => tb.type === 'approval' && tb.tool_name === name && tb.status !== 'pending'
          );
          const approvalStatus: 'approved' | 'denied' | undefined = approvalBlock?.status as 'approved' | 'denied' | undefined;

          if (name === 'edit') {
            return (
              <EditFileWidget
                key={b.call_id || idx}
                args={b.args}
                result={b.result}
                active={b.active}
                is_error={b.is_error}
              />
            );
          }
          if (isSubagentTool(b)) {
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
                subagents={subagents}
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
            />
          );
        } else if (b.type === 'approval') {
          // The user requested to hide the standalone approval block completely
          // and only show the inline "Approved" badge on the tool itself.
          if (b.status !== 'pending') return null;
          return <ApprovalBlockUI key={`approval-${b.prompt_id}-${idx}`} block={b} />;
        }
        // subagent_ref blocks are now rendered inline by SubagentSpawnWidget;
        // if a ref arrives without a matching wrapper, show the card directly.
        if (b.type === 'subagent_ref') {
          const hasSpawnWidget = iteration.toolBlocks.some(isSubagentTool);
          if (hasSpawnWidget) return null;

          const sa = subagents?.[b.subagent_id];
          if (!sa) return null;
          return (
            <div key={`subagent-${b.subagent_id}-${idx}`} className="subagents-section">
              <SubagentCard subagent={sa} />
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
            <BrainIcon size={13} className={`step-icon ${isStreaming ? 'step-icon-thinking' : ''}`} color={isStreaming ? undefined : "#888"} />
            <span className="step-label" style={stepLabelBold}>{thoughtLabel}</span>
            {thoughtCollapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
          </div>
          {!thoughtCollapsed && (
            <div className="iteration-body" style={{ marginLeft: '6px', paddingLeft: '12px', borderLeft: '1px solid var(--text-muted)', display: 'flex', flexDirection: 'column', gap: '8px', marginTop: '6px', paddingBottom: '4px' }}>
              <div className="thinking-block">
                {thinkingBlock.text}
                {isStreaming ? <span className="typing-dot" style={typingDotStyle}>...</span> : null}
              </div>
            </div>
          )}
        </div>
      )}

      {hasOnlySubagents ? (
        <div style={{ marginTop: hasThinkingContent ? '4px' : '0' }}>
          {renderToolBlocks()}
        </div>
      ) : (
        hasTools && (
          <div className="step-block" style={{ marginTop: hasThinkingContent ? '4px' : '0' }}>
            <div
              className={`step-row ${isStreaming ? 'step-row-active' : ''}`}
              onClick={() => setToolsCollapsed(!toolsCollapsed)}
            >
              <WrenchIcon size={13} className="step-icon" color={isStreaming ? undefined : "#888"} />
              <span className="step-label" style={stepLabelBold}>{toolsLabel}</span>
              {toolsCollapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
            </div>
            {!toolsCollapsed && (
            <div className="iteration-body" style={iterationBodyStyle}>
                {renderToolBlocks()}
              </div>
            )}
          </div>
        )
      )}
    </>
  );
}, (prev, next) => {
  return prev.iteration.id === next.iteration.id &&
         prev.iteration.isLast === next.iteration.isLast &&
         prev.iteration.thinkingBlock === next.iteration.thinkingBlock &&
         prev.iteration.toolBlocks.length === next.iteration.toolBlocks.length &&
         prev.iteration.toolBlocks.every((b, i) => b === next.iteration.toolBlocks[i]) &&
         prev.subagents === next.subagents;
});




const ToolBlockUI = memo(function ToolBlockUI({
  name,
  args,
  result,
  active,
  is_error,
  startTime,
  endTime,
  approvalStatus,
}: {
  name: string;
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
  startTime?: number;
  endTime?: number;
  approvalStatus?: 'approved' | 'denied';
}) {
  const [collapsed, setCollapsed] = useState(!active);
  const [showMore, setShowMore] = useState(false);

  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);

  const formattedArgs = useMemo(() => {
    if (!args) return '';
    if (typeof args === 'string') return args;
    try {
      return JSON.stringify(args, null, 2);
    } catch {
      return String(args);
    }
  }, [args]);

  const displayResult = useMemo(() => {
    if (!result) return '';
    if (!showMore && result.length > 500) {
      return result.substring(0, 500) + '...\n\n*(Truncated. Click Show More to see full output)*';
    }
    return result;
  }, [result, showMore]);

  return (
    <div className="step-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''}`}
        onClick={() => setCollapsed(!collapsed)}
        style={{ cursor: 'pointer' }}
      >
        {(() => { const ToolIcon = getToolIcon(name); return <ToolIcon size={13} className="step-icon" style={toolIconMargin} color={is_error ? '#f87171' : (active ? 'var(--text-muted)' : 'var(--text-muted)')} />; })()}
        <span className="step-label tool-name" style={{ display: 'flex', alignItems: 'center', flex: 1 }}>
          <span>{name}</span>
          {startTime && endTime && (
            <span style={{ opacity: 0.5, fontWeight: 'normal', marginLeft: '4px' }}>· {formatTime(endTime - startTime)}</span>
          )}
        </span>
        <div style={{ display: 'flex', alignItems: 'center', gap: '8px' }}>
          {approvalStatus === 'approved' && (
            <span className="approval-status-badge status-approved" style={approvalBadgeStyle}>Approved</span>
          )}
          {approvalStatus === 'denied' && (
            <span className="approval-status-badge status-denied" style={approvalBadgeStyle}>Denied</span>
          )}
          {collapsed ? <ChevronRightIcon size={12} className="step-chevron" /> : <ChevronDownIcon size={12} className="step-chevron" />}
        </div>
      </div>
      {!collapsed && (
        <div className="step-body">
          {formattedArgs && (
            <div className="tool-section">
              <div className="tool-section-label">INPUT</div>
              <pre className="tool-args-pre">{formattedArgs}</pre>
            </div>
          )}
          {!active && (
            <div className="tool-section">
              <div className="tool-section-label">OUTPUT</div>
              {result ? (
                <>
                  <MarkdownContent content={displayResult} className="tool-result-content assistant-msg" />
                  {!showMore && result.length > 500 && (
                    <div 
                      className="tool-show-more-btn"
                      onClick={(e) => { e.stopPropagation(); setShowMore(true); }}
                      style={{
                        color: 'var(--accent)',
                        cursor: 'pointer',
                        fontSize: '12px',
                        textAlign: 'center',
                        padding: '6px 0',
                        marginTop: '8px',
                        borderRadius: '6px',
                        background: 'var(--overlay-0_04)',
                        transition: 'background 0.2s',
                      }}
                      onMouseEnter={(e) => e.currentTarget.style.background = 'var(--overlay-0_08)'}
                      onMouseLeave={(e) => e.currentTarget.style.background = 'var(--overlay-0_04)'}
                    >
                      Show More
                    </div>
                  )}
                </>
              ) : (
                <div style={{ opacity: 0.5, fontStyle: 'italic', fontSize: '12px' }}>(No output returned)</div>
              )}
            </div>
          )}
        </div>
      )}
    </div>
  );
});

const APPROVAL_LABELS: Record<string, string> = {
  deny: 'Denied (once)',
  deny_persistent: 'Denied (always)',
  allow_once: 'Allowed (once)',
  allow_session: 'Allowed (session)',
  allow_persistent: 'Allowed (always)',
};

const ApprovalBlockUI = memo(function ApprovalBlockUI({ block }: { block: ApprovalBlock }) {
  const dispatch = useAppDispatch();
  const [chosenAction, setChosenAction] = useState<string | null>(null);

  const promptId = block.prompt_id ?? '';
  const handleApprove = async (choice: string) => {
    setChosenAction(choice);
    dispatch(toolApprovalResponded({ promptId, approved: !choice.startsWith('deny') }));
    try {
      await invoke('approve_tool', { promptId, choice });
    } catch (e) {
      console.error('Failed to approve tool', e);
    }
  };

  const isResolved = block.status === 'approved' || block.status === 'denied';
  const statusLabel = chosenAction
    ? APPROVAL_LABELS[chosenAction] ?? (block.status === 'approved' ? 'Approved' : 'Denied')
    : block.status === 'approved' ? 'Approved' : block.status === 'denied' ? 'Denied' : '';

  if (isResolved) {
    return (
      <div className="approval-block approval-resolved">
        <div className="approval-header">
          <span className="approval-title">{block.tool_name}</span>
          {block.danger_level ? (
            <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
          ) : null}
          <span className={`approval-status-badge ${block.status === 'approved' ? 'status-approved' : 'status-denied'}`}>
            {statusLabel}
          </span>
        </div>
      </div>
    );
  }

  return (
    <div className="approval-block">
      <div className="approval-header">
        <span className="approval-title">Approval Required: {block.tool_name}</span>
        {block.danger_level ? (
          <span className={`danger-badge danger-${block.danger_level}`}>{block.danger_level}</span>
        ) : null}
      </div>
      <div className="approval-explanation">{block.explanation}</div>
      <div className="approval-args">
        <pre>{typeof block.tool_input === 'string' ? block.tool_input : JSON.stringify(block.tool_input, null, 2)}</pre>
      </div>
      <div className="approval-actions" style={approvalActionsStyle}>
        <button className="btn-deny" onClick={() => handleApprove('deny')}>Deny Once</button>
        <button className="btn-deny" onClick={() => handleApprove('deny_persistent')}>Deny Always</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_once')}>Allow Once</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_session')}>Allow Session</button>
        <button className="btn-allow" onClick={() => handleApprove('allow_persistent')}>Always Allow</button>
      </div>
    </div>
  );
});
// ── Edit File Widget ─────────────────────────────────────────────────

/** Extract a filename from a path string. */
function basename(path: string): string {
  const parts = path.replace(/\\/g, '/').split('/');
  return parts[parts.length - 1] || path;
}

/** Parse a unified diff into side-by-side rows. */
interface DiffRow {
  oldLineNo: number | null;
  newLineNo: number | null;
  oldText: string;
  newText: string;
  type: 'context' | 'add' | 'del' | 'empty';
}

function parseUnifiedDiff(diffStr: string): DiffRow[] {
  const lines = diffStr.split('\n');
  const rows: DiffRow[] = [];
  let oldLine = 0;
  let newLine = 0;
  let inHunk = false;

  for (const line of lines) {
    if (line.startsWith('@@')) {
      const m = line.match(/@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@/);
      if (m) {
        oldLine = parseInt(m[1] ? line.match(/@@ -(\d+)/)![1] : '0', 10);
        newLine = parseInt(m[2], 10);
      }
      inHunk = true;
      continue;
    }
    if (!inHunk) continue;
    if (line.startsWith('---') || line.startsWith('+++')) continue;

    if (line.startsWith('+')) {
      rows.push({ oldLineNo: null, newLineNo: newLine++, oldText: '', newText: line.slice(1), type: 'add' });
    } else if (line.startsWith('-')) {
      rows.push({ oldLineNo: oldLine++, newLineNo: null, oldText: line.slice(1), newText: '', type: 'del' });
    } else if (line.startsWith(' ')) {
      rows.push({ oldLineNo: oldLine++, newLineNo: newLine++, oldText: line.slice(1), newText: line.slice(1), type: 'context' });
    } else if (line.startsWith('\\')) {
      // "\ No newline at end of file" — skip
      continue;
    }
  }
  return rows;
}

/** Extract the line-range summary line the backend emits:
 *  "Edited lines 12–18 (3 additions, 2 deletions)" → {start,end,adds,dels}. */
interface EditSummary {
  start: number;
  end: number;
  additions: number;
  deletions: number;
}

function parseEditSummary(result: string): EditSummary | null {
  const m = result.match(/Edited lines (\d+)–(\d+) \((\d+) additions?, (\d+) deletions?\)/);
  if (!m) return null;
  return { start: +m[1], end: +m[2], additions: +m[3], deletions: +m[4] };
}

const EditFileWidget = memo(function EditFileWidget({
  args,
  result,
  active,
  is_error,
}: {
  args?: unknown;
  result?: string;
  active?: boolean;
  is_error?: boolean;
}) {
  const [collapsed, setCollapsed] = useState(!active);

  useEffect(() => {
    if (active === false) setCollapsed(true);
    else if (active === true) setCollapsed(false);
  }, [active]);

  const filePath = (args as Record<string, unknown> | undefined)?.file_path as string | undefined;
  const fileName = filePath ? basename(filePath) : 'file';

  const summary = useMemo(() => (result ? parseEditSummary(result) : null), [result]);
  const diffRows = useMemo(() => {
    if (!result) return [];
    // The diff starts after the summary line.
    const diffStart = result.indexOf('--- ');
    if (diffStart === -1) return [];
    return parseUnifiedDiff(result.slice(diffStart));
  }, [result]);

  const range = useMemo(() => {
    if (!summary) return null;
    return summary.start === summary.end ? `L${summary.start}` : `L${summary.start}–L${summary.end}`;
  }, [summary]);

  const labelPrefix = active ? 'Editing' : is_error ? 'Edit failed:' : summary ? 'Edited' : 'Edited';

  return (
    <div className="step-block edit-file-block">
      <div
        className={`step-row ${active ? 'step-row-active' : ''} ${is_error ? 'step-row-error' : ''}`}
        onClick={() => !active && setCollapsed(!collapsed)}
        style={{ cursor: active ? 'default' : 'pointer' }}
      >
        {(() => { const ToolIcon = getToolIcon('edit'); return <ToolIcon size={13} className="step-icon" style={toolIconMargin} color={is_error ? '#f87171' : (active ? 'var(--text-muted)' : 'var(--text-muted)')} />; })()}
        <span className="step-label edit-file-label">
          {labelPrefix} <span className="edit-file-name">{fileName}</span>
          {range && <span className="edit-file-range"> · {range}</span>}
          {summary && (
            <span className="edit-file-stats">
              {' '}(<span className="stat-add">+{summary.additions}</span> <span className="stat-del">−{summary.deletions}</span>)
            </span>
          )}
        </span>
        {!active && !is_error && diffRows.length > 0 && (
          collapsed
            ? <ChevronRightIcon size={12} className="step-chevron" />
            : <ChevronDownIcon size={12} className="step-chevron" />
        )}
      </div>
      {!collapsed && diffRows.length > 0 && (
        <div className="edit-diff-body">
          <div className="edit-diff-path">{filePath}</div>
          <div className="edit-diff-table">
            {diffRows.map((row, idx) => (
              <div key={idx} className={`diff-row diff-${row.type}`}>
                <span className="diff-lineno diff-old">{row.oldLineNo ?? ''}</span>
                <span className="diff-linecontent diff-old-content">
                  {row.type === 'add' ? '' : row.oldText}
                </span>
                <span className="diff-lineno diff-new">{row.newLineNo ?? ''}</span>
                <span className="diff-linecontent diff-new-content">
                  {row.type === 'del' ? '' : row.newText}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  );
});



const SubagentSpawnWidget = memo(function SubagentSpawnWidget({
  args,
  active,
  subagentRefs,
  subagents,
}: {
  args: unknown;
  active?: boolean;
  subagentRefs?: SubagentRefBlock[];
  subagents?: Record<string, SubagentEntry>;
}) {
  const count = countSpawnedAgents(args);
  const title = active ? `Spawning ${count} agent${count > 1 ? 's' : ''}...` : `Spawned ${count} agent${count > 1 ? 's' : ''}`;
  return (
    <div className="step-block spawn-block">
      <div className={`step-row ${active ? 'step-row-active' : ''}`} style={{ cursor: 'default' }}>
        <UsersIcon size={13} className="step-icon" color={active ? undefined : 'var(--text-muted)'} />
        <span className="step-label" style={stepLabelBold}>{title}</span>
      </div>
      {subagentRefs && subagentRefs.length > 0 && (
        <div className="spawn-block-children" style={spawnBlockChildrenStyle}>
          {subagentRefs.map((refBlock, idx) => {
            const sa = subagents?.[refBlock.subagent_id];
            if (!sa) return null;
            return <SubagentCard key={idx} subagent={sa} />;
          })}
        </div>
      )}
    </div>
  );
}, (prev, next) => {
  if (prev.active !== next.active) return false;
  if (prev.args !== next.args) return false;
  if (prev.subagents !== next.subagents) return false;
  if (!prev.subagentRefs && !next.subagentRefs) return true;
  if (!prev.subagentRefs || !next.subagentRefs) return false;
  if (prev.subagentRefs.length !== next.subagentRefs.length) return false;
  return prev.subagentRefs.every((r, i) => r === next.subagentRefs![i]);
});

const SubagentCard = memo(function SubagentCard({ subagent }: { subagent: SubagentEntry }) {
  const dispatch = useAppDispatch();

  const statusIcon = useMemo(() => {
    if (subagent.status === 'working') return <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="var(--success)" />;
    if (subagent.status === 'error') return <XIcon size={12} color="#f87171" />;
    return null;
  }, [subagent.status]);

  const toolCount = useMemo(() => subagent.blocks?.filter((b) => b.type === 'tool').length || 0, [subagent.blocks]);

  const statusText = useMemo(() => {
    if (subagent.status === 'working') {
      const elapsed = subagent.endTime
        ? formatTime(subagent.endTime - subagent.startTime)
        : formatTime(Date.now() - subagent.startTime);
      return `Working · ${toolCount} tools · ${elapsed}`;
    }
    const iterText = subagent.iterations_used ? `${subagent.iterations_used} iter` : '';
    const toolText = toolCount > 0 ? `${toolCount} tools` : '';
    const timeText =
      subagent.endTime && subagent.startTime ? formatTime(subagent.endTime - subagent.startTime) : '';
    const parts = [subagent.status === 'done' ? 'Done' : 'Failed'];
    if (iterText) parts.push(iterText);
    if (toolText) parts.push(toolText);
    if (timeText) parts.push(timeText);
    return parts.join(' · ');
  }, [subagent, toolCount]);

  const displayStr = subagent.role_name || subagent.id;
  const idText = typeof displayStr === 'string' ? displayStr : JSON.stringify(displayStr);

  const hasPendingApproval = useMemo(
    () => subagent.blocks?.some((b) => b.type === 'approval' && b.status === 'pending'),
    [subagent.blocks]
  );

  const handleViewDetails = useCallback(() => {
    dispatch(viewSubagent({ id: subagent.id, name: idText }));
  }, [dispatch, subagent.id, idText]);

  return (
    <div
      className={`subagent-card ${subagent.status === 'working' ? 'subagent-working' : ''} ${hasPendingApproval ? 'subagent-needs-approval' : ''}`}
    >
      <div className="subagent-header">
        <span className="subagent-icon">{statusIcon}</span>
        <span className="subagent-id">{idText}</span>
        {hasPendingApproval && <span className="subagent-badge-pending">Approval Required</span>}
        <span className="subagent-status">{statusText}</span>
        <button className="subagent-view-btn" onClick={handleViewDetails} title="View details">
          View Details <ChevronRightIcon size={12} />
        </button>
      </div>
    </div>
  );
});


const TurnFooter = memo(function TurnFooter({ entry }: { entry: ChatEntry }) {
  const [copied, setCopied] = useState(false);

  const rawOutput = useMemo(() => {
    if (!entry.blocks) return '';
    return entry.blocks
      .filter((b): b is Extract<TurnBlock, { type: 'assistant' }> => b.type === 'assistant')
      .map((b) => b.text)
      .join('\n');
  }, [entry.blocks]);

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
      setTimeout(() => setCopied(false), 1500);
    } catch {
      // ignore
    }
  }, [rawOutput]);

  if (!endTimeText && !rawOutput) return null;

  return (
    <div className="turn-footer">
      {rawOutput && (
        <button className="turn-copy-btn" onClick={handleCopy} title="Copy Raw Assistant Output">
          {copied ? <CheckIcon size={11} color="var(--success)" /> : <CopyIcon size={11} />}
        </button>
      )}
      {endTimeText && <span className="turn-end-time">{endTimeText}</span>}
    </div>
  );
});

export const AgentTurnUI = memo(function AgentTurnUI({ entry }: { entry: ChatEntry }) {
  const subagents = useSelector((state: RootState) => state.chat.subagents);
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
            className={`turn-header ${isProcessing ? 'processing-pulse' : ''}`}
            style={{ cursor: !isProcessing ? 'pointer' : 'default' }}
            onClick={() => {
              if (!isProcessing) setCollapsed(!collapsed);
            }}
          >
            {isProcessing ? (
              <>
                <LoaderIcon className="tool-loader-icon" size={12} />
                <ProcessingTimer startTime={entry.startTime} endTime={entry.endTime} />
                <ChevronDownIcon size={12} style={ml4} />
              </>
            ) : (
              <>
                <span>Worked {summaryParts.join(' · ')}</span>
                {collapsed ? <ChevronRightIcon size={12} style={ml4} /> : <ChevronDownIcon size={12} style={ml4} />}
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
                <div style={errorBlockStyle}>
                  <AlertTriangleIcon size={16} style={{ flexShrink: 0 }} />
                  <span style={{ lineHeight: '1.4' }}>{item.data.text}</span>
                </div>
              )
            ) : (
              !collapsed && <TurnIterationUI iteration={item.data} subagents={subagents} />
            )}
          </React.Fragment>
        );
      })}

      <TurnFooter entry={entry} />
    </div>
  );
});
