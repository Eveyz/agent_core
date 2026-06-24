import { useMemo, useCallback, memo } from 'react';
import { useSelector } from 'react-redux';
import type { RootState } from '../../store';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { viewSubagent } from '../../features/chat/chatSlice';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import UsersIcon from 'lucide-react/dist/esm/icons/users.mjs';
import { formatTime } from '../../utils/format';
import { countSpawnedAgents } from './turnHelpers';
import type { SubagentRefBlock } from './turnHelpers';

const SubagentSpawnWidget = memo(function SubagentSpawnWidget({
  args,
  active,
  subagentRefs,
}: {
  args: unknown;
  active?: boolean;
  subagentRefs?: SubagentRefBlock[];
}) {
  const count = countSpawnedAgents(args);
  const title = active ? `Spawning ${count} agent${count > 1 ? 's' : ''}...` : `Spawned ${count} agent${count > 1 ? 's' : ''}`;
  return (
    <div className="step-block spawn-block">
      <div className={`step-row step-row-default ${active ? 'step-row-active' : ''}`}>
        <UsersIcon size={13} className="step-icon" color={active ? undefined : 'var(--text-muted)'} />
        <span className="step-label step-label-bold">{title}</span>
      </div>
      {subagentRefs && subagentRefs.length > 0 && (
        <div className="spawn-block-children spawn-block-children-style">
          {subagentRefs.map((refBlock, idx) => {
            return <SubagentCard key={idx} subagentId={refBlock.subagent_id} />;
          })}
        </div>
      )}
    </div>
  );
}, (prev, next) => {
  if (prev.active !== next.active) return false;
  if (prev.args !== next.args) return false;
  if (!prev.subagentRefs && !next.subagentRefs) return true;
  if (!prev.subagentRefs || !next.subagentRefs) return false;
  if (prev.subagentRefs.length !== next.subagentRefs.length) return false;
  return prev.subagentRefs.every((r, i) => r === next.subagentRefs![i]);
});

const SubagentCard = memo(function SubagentCard({ subagentId }: { subagentId: string }) {
  const dispatch = useAppDispatch();
  const subagent = useSelector((state: RootState) => state.chat.subagents[subagentId]);

  const statusIcon = useMemo(() => {
    if (!subagent) return null;
    if (subagent.status === 'working') return <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="var(--success)" />;
    if (subagent.status === 'error') return <XIcon size={12} color="#f87171" />;
    return null;
  }, [subagent?.status]);

  const toolCount = useMemo(() => subagent?.blocks?.filter((b) => b.type === 'tool').length || 0, [subagent?.blocks]);

  const statusText = useMemo(() => {
    if (!subagent) return '';
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

  const displayStr = subagent?.role_name || subagent?.id || subagentId;
  const idText = typeof displayStr === 'string' ? displayStr : JSON.stringify(displayStr);

  const hasPendingApproval = useMemo(
    () => subagent?.blocks?.some((b) => b.type === 'approval' && b.status === 'pending'),
    [subagent?.blocks]
  );

  const handleViewDetails = useCallback(() => {
    dispatch(viewSubagent({ id: subagentId, name: idText }));
  }, [dispatch, subagentId, idText]);

  if (!subagent) return null;

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

export { SubagentCard };
export default SubagentSpawnWidget;
