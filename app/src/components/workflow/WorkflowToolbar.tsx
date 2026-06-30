import { useEffect, useRef } from "react";
import SaveIcon from "lucide-react/dist/esm/icons/save.mjs";
import ShieldCheckIcon from "lucide-react/dist/esm/icons/shield-check.mjs";
import PlayIcon from "lucide-react/dist/esm/icons/play.mjs";
import SquareIcon from "lucide-react/dist/esm/icons/square.mjs";
import BarChartIcon from "lucide-react/dist/esm/icons/bar-chart-3.mjs";
import LoaderIcon from "lucide-react/dist/esm/icons/loader-2.mjs";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import { cancelWorkflowRun } from "../../features/workflow/workflowSlice";
import "./WorkflowToolbar.css";

interface WorkflowToolbarProps {
  wfName: string;
  setWfName: (name: string) => void;
  hasActiveWorkflow: boolean;
  dirty: boolean;
  validationMsg: string | null;
  /** Increment to request focus + select-all on the name input. */
  nameFocusKey: number;
  onSave: () => void;
  onValidate: () => void;
  onRun: () => void;
  onShowResults: () => void;
}

export function WorkflowToolbar({
  wfName,
  setWfName,
  hasActiveWorkflow,
  dirty,
  validationMsg,
  nameFocusKey,
  onSave,
  onValidate,
  onRun,
  onShowResults,
}: WorkflowToolbarProps) {
  const dispatch = useAppDispatch();
  const isExecuting = useAppSelector((s) => s.workflow.isExecuting);
  const runStatus = useAppSelector((s) => s.workflow.runStatus);
  const activeRunId = useAppSelector((s) => s.workflow.activeRunId);
  const nameInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!nameFocusKey || !hasActiveWorkflow) return;

    const focusAndSelect = () => {
      const el = nameInputRef.current;
      if (!el || el.disabled) return false;
      el.focus();
      el.select();
      return true;
    };

    if (!focusAndSelect()) {
      const timer = window.setTimeout(focusAndSelect, 0);
      return () => window.clearTimeout(timer);
    }
  }, [nameFocusKey, hasActiveWorkflow]);

  const handleStop = async () => {
    if (activeRunId) {
      // The UI immediately enters the cancelling state as requested
      // The thunk or backend event will eventually update the status to cancelled
      await dispatch(cancelWorkflowRun(activeRunId));
    }
  };

  return (
    <div className="workflow-toolbar">
      <input
        ref={nameInputRef}
        id="workflow-name-input"
        className="settings-input workflow-name-input"
        value={wfName}
        onChange={(e) => setWfName(e.target.value)}
        onBlur={() => {
          if (dirty && wfName.trim()) onSave();
        }}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            e.currentTarget.blur();
          }
        }}
        placeholder="Workflow name"
        disabled={!hasActiveWorkflow}
      />
      
      <div className="toolbar-divider" />

      <button className="btn-secondary toolbar-btn" onClick={onSave} disabled={!hasActiveWorkflow || (!dirty && Boolean(wfName))}>
        <SaveIcon size={14} /> Save
      </button>
      <button className="btn-secondary toolbar-btn" onClick={onValidate} disabled={!hasActiveWorkflow}>
        <ShieldCheckIcon size={14} /> Validate
      </button>

      <div className="toolbar-spacer" />

      {dirty && <span className="toolbar-unsaved"><span className="toolbar-unsaved-dot">●</span> Unsaved Changes</span>}
      {validationMsg && (
        <span className={validationMsg.startsWith("Valid") ? "toolbar-msg-success" : "toolbar-msg-danger"}>{validationMsg}</span>
      )}
      
      <div className="toolbar-divider" />

      <button className="btn-secondary toolbar-btn" onClick={onShowResults} disabled={!hasActiveWorkflow}>
        <BarChartIcon size={14} /> Results
      </button>

      {!isExecuting ? (
        <button 
          className="btn-primary toolbar-run-btn"
          onClick={onRun} 
          disabled={!hasActiveWorkflow}
        >
          <PlayIcon size={14} fill="currentColor" /> Run Workflow
        </button>
      ) : (
        <button 
          className="btn-primary toolbar-stop-btn"
          onClick={handleStop} 
          disabled={runStatus === 'cancelling'}
        >
          {runStatus === 'cancelling' ? (
            <LoaderIcon size={14} className="spin" />
          ) : (
            <SquareIcon size={14} fill="currentColor" />
          )}
          {runStatus === 'cancelling' ? "Stopping..." : "Stop Workflow"}
        </button>
      )}
    </div>
  );
}
