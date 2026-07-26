import { useEffect, useMemo, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import PlayIcon from 'lucide-react/dist/esm/icons/play.mjs';
import SendIcon from 'lucide-react/dist/esm/icons/send.mjs';
import UploadIcon from 'lucide-react/dist/esm/icons/upload.mjs';
import type {
  PublishedWorkflowReceipt,
  WorkflowLibraryEntry,
  WorkflowRuntimeRunSummary,
} from '../../features/workflow/types';
import './RuntimeWorkflowDetail.css';

interface Props {
  entry: WorkflowLibraryEntry;
  sessionId?: string;
  projectId?: string;
  workspace?: string;
  onChanged: () => void;
  onContinueInChat: (entry: WorkflowLibraryEntry) => void;
}

export function RuntimeWorkflowDetail({
  entry,
  sessionId,
  projectId,
  workspace,
  onChanged,
  onContinueInChat,
}: Props) {
  const [status, setStatus] = useState('');
  const [revisions, setRevisions] = useState<PublishedWorkflowReceipt[]>([]);
  const [runs, setRuns] = useState<WorkflowRuntimeRunSummary[]>([]);
  const steps = entry.workflow?.steps ?? [];
  const dependents = useMemo(() => {
    const result = new Map<string, string[]>();
    for (const step of steps) {
      for (const dependency of step.after ?? []) {
        result.set(dependency, [...(result.get(dependency) ?? []), step.key]);
      }
    }
    return result;
  }, [steps]);

  const refreshHistory = async () => {
    const [revisionHistory, runHistory] = await Promise.all([
      invoke<PublishedWorkflowReceipt[]>('list_workflow_revision_history', {
        workflowId: entry.workflow_id,
      }),
      invoke<WorkflowRuntimeRunSummary[]>('list_runtime_workflow_runs', {
        workflowId: entry.workflow_id,
        limit: 20,
      }),
    ]);
    setRevisions(revisionHistory);
    setRuns(runHistory);
  };

  useEffect(() => {
    void refreshHistory().catch((error) => setStatus(`History unavailable: ${error}`));
  }, [entry.workflow_id]);

  const publish = async () => {
    setStatus('Publishing…');
    try {
      const receipt = await invoke<{ revision_number: number }>('publish_workflow_revision', {
        draftId: entry.draft_id,
        expectedVersion: entry.draft_version,
      });
      setStatus(`Published revision ${receipt.revision_number}`);
      await refreshHistory();
      onChanged();
    } catch (error) {
      setStatus(`Publish failed: ${error}`);
    }
  };

  const run = async () => {
    setStatus('Starting…');
    try {
      const receipt = await invoke<{ run_id: string }>('run_published_workflow', {
        workflowId: entry.workflow_id,
        revisionId: entry.latest_revision?.revision_id ?? null,
        input: {},
        sessionId: sessionId ?? null,
        projectId: projectId ?? null,
        workspace: workspace ?? null,
      });
      setStatus(`Run started: ${receipt.run_id}`);
      await refreshHistory();
    } catch (error) {
      setStatus(`Run failed: ${error}`);
    }
  };

  return (
    <div className="runtime-workflow-detail">
      <header className="runtime-workflow-header">
        <div>
          <div className="runtime-workflow-badges">
            <span>{entry.lifecycle}</span>
            <span>{entry.scope.kind}</span>
            {entry.latest_revision && <span>r{entry.latest_revision.revision_number}</span>}
          </div>
          <h2>{entry.name}</h2>
          <p>{entry.description || 'No description'}</p>
        </div>
        <div className="runtime-workflow-actions">
          {entry.latest_revision && (
            <button type="button" className="btn-primary" onClick={run}>
              <PlayIcon size={14} /> Run
            </button>
          )}
          {entry.draft_status !== 'published' && (
            <button type="button" className="btn-primary" onClick={publish}>
              <UploadIcon size={14} /> Publish
            </button>
          )}
          <button type="button" className="btn-secondary" onClick={() => onContinueInChat(entry)}>
            <SendIcon size={14} /> Continue in chat
          </button>
        </div>
      </header>

      {status && <div className="runtime-workflow-status">{status}</div>}

      <section className="runtime-workflow-section">
        <h3>Durable DAG</h3>
        <div className="runtime-workflow-dag">
          {steps.map((step) => (
            <article key={step.key} className="runtime-workflow-node">
              <div className="runtime-workflow-node-title">{step.key}</div>
              <div className="runtime-workflow-node-agent">
                {step.agent.type === 'saved'
                  ? `Saved agent · ${step.agent.agent_id}`
                  : `Inline agent · ${step.agent.blueprint?.name ?? 'Unnamed'}`}
              </div>
              <p>{step.instruction}</p>
              <div className="runtime-workflow-node-links">
                <span>After: {step.after?.length ? step.after.join(', ') : 'start'}</span>
                <span>Next: {dependents.get(step.key)?.join(', ') || 'result'}</span>
              </div>
            </article>
          ))}
        </div>
      </section>

      <section className="runtime-workflow-section runtime-workflow-metadata">
        <h3>Definition</h3>
        <dl>
          <dt>Draft</dt><dd>{entry.draft_id} · v{entry.draft_version}</dd>
          <dt>Program hash</dt><dd>{entry.program_hash}</dd>
          <dt>Updated</dt><dd>{entry.updated_at}</dd>
        </dl>
      </section>

      <section className="runtime-workflow-section">
        <h3>Revision history</h3>
        <div className="runtime-workflow-history">
          {revisions.map((revision) => (
            <div key={revision.revision_id}>
              <strong>r{revision.revision_number}</strong>
              <span>{revision.published_at}</span>
              <code>{revision.program_hash.slice(0, 16)}</code>
            </div>
          ))}
          {revisions.length === 0 && <p>No published revisions.</p>}
        </div>
      </section>

      <section className="runtime-workflow-section">
        <h3>Preview & run history</h3>
        <div className="runtime-workflow-history">
          {runs.map((runEntry) => (
            <div key={runEntry.run_id}>
              <strong>{runEntry.status}</strong>
              <span>{runEntry.trigger || 'workflow'}</span>
              <code>{runEntry.run_id}</code>
              {runEntry.failed_nodes.length > 0 && (
                <small>Failed: {runEntry.failed_nodes.join(', ')}</small>
              )}
            </div>
          ))}
          {runs.length === 0 && <p>No runs yet.</p>}
        </div>
      </section>
    </div>
  );
}
