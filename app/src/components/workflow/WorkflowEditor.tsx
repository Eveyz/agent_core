import { useCallback, useEffect, useState, useMemo, useRef } from "react";
import {
  type Node,
  type Edge,
} from "@xyflow/react";
import "@xyflow/react/dist/style.css";
import { useAppDispatch, useAppSelector } from "../../hooks/useAppDispatch";
import {
  saveWorkflow,
  runWorkflow,
  fetchWorkflows,
  createWorkflow,
  setActiveWorkflow,
  updateActiveWorkflowNodes,
  setInspectedNodeId,
  onNodesChange,
  onEdgesChange,
  onConnect,
  setSelectedNodeId,
  setSelectedEdgeId,
  setShowRunView,
  addNode,
  deleteNode,
  updateNodeData,
  updateEdgeData,
} from "../../features/workflow/workflowSlice";
import { fetchAgents } from "../../features/agents/agentSlice";
import type { WorkflowDef } from "../../features/workflow/types";
import { EdgeConfigPanel } from "./EdgeConfigPanel";
import { WorkflowRunView } from "./WorkflowRunView";
import { invoke } from "@tauri-apps/api/core";
import { ReactFlowProvider } from "@xyflow/react";
import { rfToNodeDef, rfToEdgeDef } from "../../features/workflow/converters";
import "./WorkflowEditor.css";

// ── Components ──────────────────────────────────────────────────────────
import { WorkflowToolbar } from "./WorkflowToolbar";
import { WorkflowSidebar } from "./WorkflowSidebar";
import { WorkflowCanvas } from "./WorkflowCanvas";
import { NodePropertiesPanel } from "./NodePropertiesPanel";
import { NodeInspectorDrawer } from "./NodeInspectorDrawer";

// ── Conversion helpers ──────────────────────────────────────────────
// Now inside converters.ts

export function WorkflowEditor() {
  const dispatch = useAppDispatch();
  const activeWorkflow = useAppSelector((s) => s.workflow.activeWorkflow);
  const agents = useAppSelector((s) => s.agents.agents);
  const isExecuting = useAppSelector((s) => s.workflow.isExecuting);
  const isReduxDirty = useAppSelector((s) => s.workflow.dirty);
  const workflows = useAppSelector((s) => s.workflow.workflows);
  const activeNodeResults = useAppSelector((s) => s.workflow.activeNodeResults);
  const inspectedNodeId = useAppSelector((s) => s.workflow.inspectedNodeId);
  
  const nodes = useAppSelector((s) => s.workflow.nodes);
  const edges = useAppSelector((s) => s.workflow.edges);
  const selectedNodeId = useAppSelector((s) => s.workflow.selectedNodeId);
  const selectedEdgeId = useAppSelector((s) => s.workflow.selectedEdgeId);
  const showRunView = useAppSelector((s) => s.workflow.showRunView);

  const [wfName, setWfName] = useState("");
  const [validationMsg, setValidationMsg] = useState<string | null>(null);
  const [nameFocusKey, setNameFocusKey] = useState(0);
  const [creatingWorkflow, setCreatingWorkflow] = useState(false);
  const [createError, setCreateError] = useState<string | null>(null);

  const dirty = isReduxDirty || (activeWorkflow && wfName !== activeWorkflow.name) || false;

  useEffect(() => {
    dispatch(fetchWorkflows());
    dispatch(fetchAgents());
  }, [dispatch]);

  // Only reset the title when switching to a different workflow.
  useEffect(() => {
    if (activeWorkflow) {
      setWfName(activeWorkflow.name);
    } else {
      setWfName("");
    }
  }, [activeWorkflow?.id]);

  // Sync back to Redux for save (debounced to avoid drag stutter)
  const syncTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const activeWorkflowId = activeWorkflow?.id;
  useEffect(() => {
    if (!activeWorkflowId || isExecuting) return;
    if (nodes.length === 0 && edges.length === 0) return;

    if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
    syncTimerRef.current = setTimeout(() => {
      dispatch(updateActiveWorkflowNodes({
        nodes: nodes.map(n => rfToNodeDef(n, activeWorkflowId)),
        edges: edges.map(e => rfToEdgeDef(e, activeWorkflowId))
      }));
    }, 500);

    return () => {
      if (syncTimerRef.current) clearTimeout(syncTimerRef.current);
    };
  }, [nodes, edges, activeWorkflowId, dispatch, isExecuting]);

  // Bug 1 Fix: Automatically open Inspector Drawer on failure
  useEffect(() => {
    if (isExecuting) {
      const failedNodeId = Object.keys(activeNodeResults).find(
        (id) => activeNodeResults[id].status === "failed"
      );
      if (failedNodeId && inspectedNodeId !== failedNodeId) {
        dispatch(setInspectedNodeId(failedNodeId));
        dispatch(setSelectedNodeId(failedNodeId)); 
      }
    }
  }, [isExecuting, activeNodeResults, inspectedNodeId, dispatch]);

  // Inject runtime statuses directly before passing to canvas
  const displayNodes = useMemo(() => {
    return nodes.map(n => {
      if (activeNodeResults[n.id]) {
        return { ...n, data: { ...n.data, status: activeNodeResults[n.id].status } };
      }
      return n;
    });
  }, [nodes, activeNodeResults]);

  const displayWorkflows = useMemo(() => {
    return workflows.map((w) => {
      if (activeWorkflow && w.id === activeWorkflow.id) {
        return { ...w, name: wfName || w.name };
      }
      return w;
    });
  }, [workflows, activeWorkflow, wfName]);

  const handleSave = async () => {
    if (!activeWorkflow) return;
    const nodeDefs = nodes.map((n) => rfToNodeDef(n, activeWorkflow.id));
    const edgeDefs = edges.map((e) => rfToEdgeDef(e, activeWorkflow.id));
    await dispatch(saveWorkflow({
      id: activeWorkflow.id,
      name: wfName || activeWorkflow.name,
      nodes: nodeDefs,
      edges: edgeDefs,
      trust_mode: activeWorkflow.trust_mode,
      max_concurrent: activeWorkflow.max_concurrent,
      on_node_failure: activeWorkflow.on_node_failure,
    }));
  };

  const handleValidate = async () => {
    if (!activeWorkflow) return;
    const nodeDefs = nodes.map((n) => rfToNodeDef(n, activeWorkflow.id));
    const edgeDefs = edges.map((e) => rfToEdgeDef(e, activeWorkflow.id));
    try {
      const result = await invoke<{ valid: boolean; issues: { severity: string; code: string; message: string }[] }>("validate_workflow", { nodes: nodeDefs, edges: edgeDefs });
      if (result.valid) {
        const warnings = result.issues.filter((i) => i.severity === "warning");
        setValidationMsg(warnings.length > 0 ? `Valid (${warnings.length} warning(s))` : "Valid");
      } else {
        const errors = result.issues.filter((i) => i.severity === "error");
        setValidationMsg(`${errors.length} error(s): ${errors.map((e) => e.message).join("; ")}`);
      }
    } catch (e) {
      setValidationMsg(`Validation failed: ${e}`);
    }
  };

  const handleRun = async () => {
    const workflowId = activeWorkflow?.id;
    if (!workflowId) return;

    if (dirty) {
      try {
        await handleSave();
      } catch (e) {
        setCreateError(`Failed to save before run: ${e}`);
        return;
      }
    }

    dispatch(runWorkflow({ workflowId, input: { task: "Run workflow" } }));
  };

  const handleNewWorkflow = async () => {
    setCreateError(null);
    setCreatingWorkflow(true);
    try {
      const action = await dispatch(createWorkflow({ name: "New Workflow" }));
      if (createWorkflow.fulfilled.match(action)) {
        setWfName("New Workflow");
        setNameFocusKey((k) => k + 1);
      } else if (createWorkflow.rejected.match(action)) {
        setCreateError(action.error.message ?? "Failed to create workflow");
      }
    } finally {
      setCreatingWorkflow(false);
    }
  };

  const selectWorkflow = (wf: WorkflowDef) => {
    dispatch(setActiveWorkflow(wf));
  };

  const handleNodeClick = useCallback((_: unknown, node: Node) => { 
    dispatch(setSelectedNodeId(node.id)); 
    dispatch(setSelectedEdgeId(null));
    dispatch(setInspectedNodeId(node.id));
  }, [dispatch]);
  
  const handleEdgeClick = useCallback((_: unknown, edge: Edge) => { 
    dispatch(setSelectedEdgeId(edge.id)); 
    dispatch(setSelectedNodeId(null));
    dispatch(setInspectedNodeId(null));
  }, [dispatch]);
  
  const handlePaneClick = useCallback(() => {
    dispatch(setSelectedNodeId(null));
    dispatch(setSelectedEdgeId(null));
    dispatch(setInspectedNodeId(null));
  }, [dispatch]);

  const handleUpdateNode = useCallback((nodeId: string, data: any) => {
    dispatch(updateNodeData({ nodeId, data }));
  }, [dispatch]);

  const handleDeleteNode = useCallback((nodeId: string) => {
    dispatch(deleteNode(nodeId));
  }, [dispatch]);

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);
  const selectedEdge = edges.find((e) => e.id === selectedEdgeId);

  return (
    <div className="workflow-editor-container">
      <WorkflowSidebar 
        workflows={displayWorkflows} 
        activeWorkflowId={activeWorkflow?.id} 
        creatingWorkflow={creatingWorkflow}
        onNewWorkflow={handleNewWorkflow} 
        onSelectWorkflow={selectWorkflow} 
      />

      <div className="workflow-editor-main">
        {createError && (
          <div className="workflow-create-error">{createError}</div>
        )}
        <WorkflowToolbar 
          wfName={wfName}
          setWfName={setWfName}
          hasActiveWorkflow={!!activeWorkflow}
          dirty={dirty}
          validationMsg={validationMsg}
          nameFocusKey={nameFocusKey}
          onSave={handleSave}
          onValidate={handleValidate}
          onRun={handleRun}
          onShowResults={() => dispatch(setShowRunView(true))}
        />

        {activeWorkflow ? (
          <ReactFlowProvider>
            <WorkflowCanvas 
              nodes={displayNodes}
              edges={edges}
              onNodesChange={(changes) => dispatch(onNodesChange(changes))}
              onEdgesChange={(changes) => dispatch(onEdgesChange(changes))}
              onConnect={(params) => dispatch(onConnect(params))}
              onNodeClick={handleNodeClick}
              onEdgeClick={handleEdgeClick}
              onPaneClick={handlePaneClick}
              onDropNode={(node) => dispatch(addNode(node))}
            />
          </ReactFlowProvider>
        ) : (
          <div className="workflow-empty-state">
            Select or create a workflow to begin.
          </div>
        )}
      </div>

      {selectedNode && (
        <NodePropertiesPanel 
          selectedNode={selectedNode}
          edges={edges}
          nodes={nodes}
          agents={agents}
          isExecuting={isExecuting}
          onUpdateNode={handleUpdateNode}
          onDeleteNode={handleDeleteNode}
        />
      )}

      {selectedEdge && (
        <EdgeConfigPanel
          edge={selectedEdge}
          onClose={() => dispatch(setSelectedEdgeId(null))}
          onUpdate={(updates) => {
            dispatch(updateEdgeData({ edgeId: selectedEdge.id, updates }));
          }}
        />
      )}

      {showRunView && activeWorkflow && (
        <WorkflowRunView workflowId={activeWorkflow.id} onClose={() => dispatch(setShowRunView(false))} />
      )}

      {inspectedNodeId && (
        <NodeInspectorDrawer 
          nodeId={inspectedNodeId}
          nodeLabel={(displayNodes.find(n => n.id === inspectedNodeId)?.data.label as string) || inspectedNodeId}
          result={activeNodeResults[inspectedNodeId]}
          onClose={() => dispatch(setInspectedNodeId(null))}
        />
      )}
    </div>
  );
}
