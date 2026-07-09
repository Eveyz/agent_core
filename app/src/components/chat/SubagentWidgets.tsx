import { useMemo, useCallback, useState, useEffect, memo } from 'react';
import { useSelector, useStore } from 'react-redux';
import type { RootState } from '../../store';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { viewSubagent } from '../../features/chat/chatSlice';
import { useTranslation } from 'react-i18next';
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
  const { t } = useTranslation();
  const count = countSpawnedAgents(args);
  const suffix = count > 1 ? '_plural' : '';
  const title = active 
    ? t(`chat.subagent.spawning${suffix}`, { count }) 
    : t(`chat.subagent.spawned${suffix}`, { count });
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
  const { t } = useTranslation();
  const dispatch = useAppDispatch();
  const store = useStore<RootState>();
  const subagent = useSelector((state: RootState) => (state.chat.subagents[state.project.activeSessionId ?? ''] ?? {})[subagentId]);

  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!subagent || subagent.status !== 'working') return;
    const interval = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(interval);
  }, [subagent?.status]);

  const statusIcon = useMemo(() => {
    if (!subagent) return null;
    if (subagent.status === 'working') return <div className="black-hole-spinner" style={{ width: 12, height: 12 }} />;
    if (subagent.status === 'done') return <CheckIcon size={12} color="var(--success)" />;
    if (subagent.status === 'error') return <XIcon size={12} color="var(--danger)" />;
    return null;
  }, [subagent?.status]);

  const toolCount = useMemo(() => subagent?.blocks?.filter((b) => b.type === 'tool').length || 0, [subagent?.blocks]);

  const statusText = useMemo(() => {
    if (!subagent) return '';
    if (subagent.status === 'working') {
      const elapsed = subagent.endTime
        ? formatTime(subagent.endTime - subagent.startTime)
        : formatTime(now - subagent.startTime);
      return t('chat.subagent.workingState', { toolCount, elapsed });
    }
    const iterText = subagent.iterations_used 
      ? t(subagent.iterations_used > 1 ? 'chat.subagent.iterations_plural' : 'chat.subagent.iterations', { count: subagent.iterations_used }) 
      : '';
    const toolText = toolCount > 0 
      ? t(toolCount > 1 ? 'chat.turn.tools_plural' : 'chat.turn.tools', { count: toolCount }) 
      : '';
    const timeText =
      subagent.endTime && subagent.startTime ? formatTime(subagent.endTime - subagent.startTime) : '';
    const statusLabel = subagent.status === 'done' ? t('chat.subagent.done') : t('chat.subagent.failed');
    const parts = [statusLabel];
    if (iterText) parts.push(iterText);
    if (toolText) parts.push(toolText);
    if (timeText) parts.push(timeText);
    return parts.join(' · ');
  }, [subagent, toolCount, now, t]);

  const displayStr = subagent?.role_name || subagent?.id || subagentId;
  const idText = typeof displayStr === 'string' ? displayStr : JSON.stringify(displayStr);

  const hasPendingApproval = useMemo(
    () => subagent?.blocks?.some((b) => b.type === 'approval' && b.status === 'pending'),
    [subagent?.blocks]
  );

  const handleViewDetails = useCallback(() => {
    const sessionId = store.getState().project.activeSessionId;
    if (!sessionId) return;
    dispatch(viewSubagent({ sessionId, id: subagentId, name: idText }));
  }, [dispatch, store, subagentId, idText]);

  if (!subagent) return null;

  return (
    <div
      className={`subagent-card ${subagent.status === 'working' ? 'subagent-working' : ''} ${hasPendingApproval ? 'subagent-needs-approval' : ''}`}
    >
      <div className="subagent-header">
        <span className="subagent-icon">{statusIcon}</span>
        <span className="subagent-id">{idText}</span>
        {hasPendingApproval && <span className="subagent-badge-pending">{t('chat.subagent.approvalRequired')}</span>}
        <span className="subagent-status">{statusText}</span>
        <button className="subagent-view-btn" onClick={handleViewDetails} title={t('chat.subagent.viewDetails')}>
          {t('chat.subagent.viewDetails')} <ChevronRightIcon size={12} />
        </button>
      </div>
    </div>
  );
});

export { SubagentCard };
export default SubagentSpawnWidget;
