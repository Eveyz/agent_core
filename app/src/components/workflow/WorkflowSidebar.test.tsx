import { renderToStaticMarkup } from 'react-dom/server';
import { describe, expect, it, vi } from 'vitest';
import type { WorkflowDef } from '../../features/workflow/types';
import { WorkflowSidebar } from './WorkflowSidebar';
import { WorkflowToolbar } from './WorkflowToolbar';

vi.mock('../../hooks/useAppDispatch', () => ({
  useAppSelector: (selector: (state: unknown) => unknown) => selector({
    agents: { agents: [] },
    workflow: { isExecuting: false, runStatus: 'idle', activeRunId: null },
  }),
  useAppDispatch: () => vi.fn(),
}));

const legacyWorkflow: WorkflowDef = {
  id: 'legacy-1',
  name: 'Legacy builder',
  description: '',
  input_schema: {},
  trust_mode: 'inherit',
  max_concurrent: 3,
  on_node_failure: 'abort',
  config: {},
  nodes: [],
  edges: [],
  created_at: '',
  updated_at: '',
};

describe('WorkflowSidebar legacy entries', () => {
  it('does not embed a publish action in the legacy selection row', () => {
    const html = renderToStaticMarkup(
      <WorkflowSidebar
        workflows={[legacyWorkflow]}
        libraryEntries={[]}
        activeWorkflowId={legacyWorkflow.id}
        onNewWorkflow={() => {}}
        onSelectWorkflow={() => {}}
        onSelectLibrary={() => {}}
      />,
    );

    expect(html).not.toContain('Publish for chat reuse');
  });
});

describe('WorkflowToolbar canvas publishing', () => {
  it('provides publishing as an explicit toolbar action', () => {
    const html = renderToStaticMarkup(
      <WorkflowToolbar
        wfName="Legacy builder"
        setWfName={() => {}}
        hasActiveWorkflow
        dirty={false}
        validationMsg={null}
        nameFocusKey={0}
        onSave={() => {}}
        onValidate={() => {}}
        onPublish={() => {}}
        onRun={() => {}}
        onShowResults={() => {}}
      />,
    );

    expect(html).toContain('Publish for chat');
  });
});
