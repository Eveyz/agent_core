import { useCallback, useEffect, useState, type DragEvent } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  addEdge,
  useNodesState,
  useEdgesState,
  type Node,
  type Edge,
  type Connection,
  type NodeTypes,
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
} from "../../features/workflow/workflowSlice";
import { fetchAgents } from "../../features/agents/agentSlice";
import type { NodeDef, EdgeDef, NodeType, WorkflowDef } from "../../features/workflow/types";
import { nodeTypes } from "./nodes";
import { EdgeConfigPanel, RouterEditor, type RouterConfig } from "./EdgeConfigPanel";
import { WorkflowRunView } from "./WorkflowRunView";
import { listen } from "@tauri-apps/api/event";
import PlayIcon from "lucide-react/dist/esm/icons/play.mjs";
import SaveIcon from "lucide-react/dist/esm/icons/save.mjs";
import PlusIcon from "lucide-react/dist/esm/icons/plus.mjs";
import BarChartIcon from "lucide-react/dist/esm/icons/bar-chart-3.mjs";
import ShieldCheckIcon from "lucide-react/dist/esm/icons/shield-check.mjs";
import { invoke } from "@tauri-apps/api/core";

const PALETTE: { type: NodeType; label: string }[] = [
  { type: "input", label: "Input" },
  { type: "agent", label: "Agent" },
  { type: "transform", label: "Transform" },
  { type: "human_approval", label: "Approval" },
  { type: "output", label: "Output" },
];

// ── Conversion helpers ──────────────────────────────────────────────

function nodeDefToRF(n: NodeDef): Node {
  return {
    id: n.id,
    type: n.node_type,
    position: { x: n.position_x, y: n.position_y },
    data: { label: n.label, nodeType: n.node_type, agentId: n.agent_id, config: n.config },
  };
}

function rfToNodeDef(n: Node, workflowId: string): NodeDef {
  const data = (n.data ?? {}) as Record<string, unknown>;
  return {
    id: n.id,
    workflow_id: workflowId,
    node_type: (n.type ?? "transform") as NodeType,
    label: (data.label as string) ?? "",
    agent_id: (data.agentId as string) ?? "",
    config: (data.config as Record<string, unknown>) ?? {},
    position_x: n.position.x,
    position_y: n.position.y,
    created_at: "",
  };
}

function edgeDefToRF(e: EdgeDef): Edge {
  return {
    id: e.id,
    source: e.source_node_id,
    target: e.target_node_id,
    label: e.label || undefined,
    animated: true,
  };
}

function rfToEdgeDef(e: Edge, workflowId: string): EdgeDef {
  return {
    id: e.id,
    workflow_id: workflowId,
    source_node_id: e.source,
    target_node_id: e.target,
    source_handle: (e.sourceHandle as string) ?? "",
    target_handle: (e.targetHandle as string) ?? "",
    label: (e.label as string) ?? "",
    condition: "",
    data_mapping: { pass_through: true },
    created_at: "",
  };
}

export function WorkflowEditor() {
  const dispatch = useAppDispatch();
  const activeWorkflow = useAppSelector((s) => s.workflow.activeWorkflow);
  const agents = useAppSelector((s) => s.agents.agents);
  const running = useAppSelector((s) => s.workflow.running);
  const dirty = useAppSelector((s) => s.workflow.dirty);
  const workflows = useAppSelector((s) => s.workflow.workflows);

  const [nodes, setNodes, onNodesChange] = useNodesState<Node>([]);
  const [edges, setEdges, onEdgesChange] = useEdgesState<Edge>([]);
  const [selectedNodeId, setSelectedNodeId] = useState<string | null>(null);
  const [selectedEdgeId, setSelectedEdgeId] = useState<string | null>(null);
  const [wfName, setWfName] = useState("");
  const [showRunView, setShowRunView] = useState(false);
  const [validationMsg, setValidationMsg] = useState<string | null>(null);

  // Load workflow list + agents on mount.
  useEffect(() => {
    dispatch(fetchWorkflows());
    dispatch(fetchAgents());
  }, [dispatch]);

  // Sync canvas when the active workflow changes.
  useEffect(() => {
    if (activeWorkflow) {
      setNodes(activeWorkflow.nodes.map(nodeDefToRF));
      setEdges(activeWorkflow.edges.map(edgeDefToRF));
      setWfName(activeWorkflow.name);
    }
  }, [activeWorkflow, setNodes, setEdges]);

  // Listen for workflow node status events to update node colors.
  useEffect(() => {
    const unlisten = listen<{ WorkflowNodeStarted?: { node_id: string }; WorkflowNodeEnded?: { node_id: string; status: string }; type?: string }>(
      "workflow_event",
      (event) => {
        const payload = event.payload as unknown;
        // The payload is an AgentEvent serialized as { type: "...", ...fields }
        const ev = payload as Record<string, unknown>;
        const evType = (ev["type"] as string) ?? "";
        if (evType.includes("NodeStarted")) {
          const nodeId = ev["node_id"] as string;
          setNodes((nds) => nds.map((n) => (n.id === nodeId ? { ...n, data: { ...n.data, status: "running" } } : n)));
        } else if (evType.includes("NodeEnded")) {
          const nodeId = ev["node_id"] as string;
          const status = (ev["status"] as string) ?? "completed";
          setNodes((nds) => nds.map((n) => (n.id === nodeId ? { ...n, data: { ...n.data, status } } : n)));
        }
      },
    );
    return () => { unlisten.then((fn) => fn()); };
  }, [setNodes]);

  const onConnect = useCallback(
    (params: Connection) => setEdges((eds) => addEdge({ ...params, animated: true }, eds)),
    [setEdges],
  );

  const addNode = (type: NodeType) => {
    const id = `${type}-${Date.now()}`;
    const newNode: Node = {
      id,
      type,
      position: { x: 200 + Math.random() * 200, y: 150 + Math.random() * 100 },
      data: { label: type.charAt(0).toUpperCase() + type.slice(1), nodeType: type, agentId: "", config: {} },
    };
    setNodes((nds) => [...nds, newNode]);
    dispatch(updateActiveWorkflowNodes({ nodes: [...nodes, newNode].map((n) => rfToNodeDef(n, activeWorkflow?.id ?? "")), edges: edges.map((e) => rfToEdgeDef(e, activeWorkflow?.id ?? "")) }));
  };

  const onDrop = (e: DragEvent) => {
    e.preventDefault();
    const type = e.dataTransfer.getData("application/reactflow") as NodeType;
    if (!type) return;
    const id = `${type}-${Date.now()}`;
    const newNode: Node = {
      id,
      type,
      position: { x: e.clientX - 280, y: e.clientY - 60 },
      data: { label: type.charAt(0).toUpperCase() + type.slice(1), nodeType: type, agentId: "", config: {} },
    };
    setNodes((nds) => [...nds, newNode]);
  };

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

  const handleNewWorkflow = async () => {
    await dispatch(createWorkflow({ name: "New Workflow" }));
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
    if (!activeWorkflow || dirty) {
      await handleSave();
    }
    if (activeWorkflow) {
      // Reset node statuses.
      setNodes((nds) => nds.map((n) => ({ ...n, data: { ...n.data, status: undefined } })));
      dispatch(runWorkflow({ workflowId: activeWorkflow.id, input: { task: "Run workflow" } }));
    }
  };

  const selectWorkflow = (wf: WorkflowDef) => {
    dispatch(setActiveWorkflow(wf));
  };

  const selectedNode = nodes.find((n) => n.id === selectedNodeId);
  const selectedEdge = edges.find((e) => e.id === selectedEdgeId);

  const onNodeClick = useCallback((_: unknown, node: Node) => { setSelectedNodeId(node.id); setSelectedEdgeId(null); }, []);
  const onEdgeClick = useCallback((_: unknown, edge: Edge) => { setSelectedEdgeId(edge.id); setSelectedNodeId(null); }, []);

  return (
    <div style={{ display: "flex", height: "100%", width: "100%", background: "var(--bg-main, #0d0d14)" }}>
      {/* Left: workflow list + palette */}
      <div style={{ width: "220px", borderRight: "1px solid var(--border-color)", display: "flex", flexDirection: "column", overflow: "hidden" }}>
        <div style={{ padding: "10px", borderBottom: "1px solid var(--border-color)" }}>
          <button className="btn-primary" style={{ width: "100%", fontSize: "12px" }} onClick={handleNewWorkflow}>
            <PlusIcon size={12} /> New Workflow
          </button>
        </div>
        <div style={{ overflowY: "auto", flex: 1, padding: "4px" }}>
          {workflows.map((wf) => (
            <div
              key={wf.id}
              className="nav-item"
              onClick={() => selectWorkflow(wf)}
              style={{ fontSize: "12px", padding: "6px 10px", cursor: "pointer", background: activeWorkflow?.id === wf.id ? "rgba(82,168,255,0.12)" : "transparent", borderRadius: "4px" }}
            >
              {wf.name || "Untitled"}
            </div>
          ))}
        </div>
        {/* Palette */}
        <div style={{ borderTop: "1px solid var(--border-color)", padding: "10px" }}>
          <div style={{ fontSize: "11px", fontWeight: 600, color: "var(--text-muted)", marginBottom: "6px", textTransform: "uppercase" }}>Add Node</div>
          {PALETTE.map((p) => (
            <div
              key={p.type}
              draggable
              onDragStart={(e) => e.dataTransfer.setData("application/reactflow", p.type)}
              onDoubleClick={() => addNode(p.type)}
              className="nav-item"
              style={{ fontSize: "12px", padding: "5px 8px", cursor: "grab", marginBottom: "2px" }}
            >
              {p.label}
            </div>
          ))}
        </div>
      </div>

      {/* Center: canvas */}
      <div style={{ flex: 1, display: "flex", flexDirection: "column", overflow: "hidden" }} onDrop={onDrop} onDragOver={(e) => e.preventDefault()}>
        {/* Toolbar */}
        <div style={{ display: "flex", alignItems: "center", gap: "8px", padding: "8px 12px", borderBottom: "1px solid var(--border-color)" }}>
          <input
            className="settings-input"
            value={wfName}
            onChange={(e) => setWfName(e.target.value)}
            style={{ width: "200px", fontSize: "13px" }}
            placeholder="Workflow name"
          />
          <button className="btn-secondary" style={{ fontSize: "12px" }} onClick={handleSave} disabled={!activeWorkflow}>
            <SaveIcon size={12} /> Save
          </button>
          <button className="btn-secondary" style={{ fontSize: "12px" }} onClick={handleValidate} disabled={!activeWorkflow}>
            <ShieldCheckIcon size={12} /> Validate
          </button>
          <button className="btn-primary" style={{ fontSize: "12px" }} onClick={handleRun} disabled={!activeWorkflow || running}>
            <PlayIcon size={12} /> {running ? "Running…" : "Run"}
          </button>
          <button className="btn-secondary" style={{ fontSize: "12px" }} onClick={() => setShowRunView(true)} disabled={!activeWorkflow}>
            <BarChartIcon size={12} /> Results
          </button>
          {dirty && <span style={{ fontSize: "11px", color: "var(--warning, #f59e0b)" }}>● unsaved</span>}
          {validationMsg && (
            <span style={{ fontSize: "11px", color: validationMsg.startsWith("Valid") ? "var(--success, #22c55e)" : "var(--danger)" }}>{validationMsg}</span>
          )}
        </div>

        {/* React Flow canvas */}
        <div style={{ flex: 1, position: "relative" }}>
          {activeWorkflow ? (
            <ReactFlow
              nodes={nodes}
              edges={edges}
              onNodesChange={onNodesChange}
              onEdgesChange={onEdgesChange}
              onConnect={onConnect}
              onNodeClick={onNodeClick}
              onEdgeClick={onEdgeClick}
              nodeTypes={nodeTypes as unknown as NodeTypes}
              fitView
              style={{ background: "var(--bg-main, #0d0d14)" }}
            >
              <Background color="var(--border-color, #2a2a3a)" gap={20} />
              <Controls />
              <MiniMap style={{ background: "var(--bg-secondary, #1e1e2e)" }} />
            </ReactFlow>
          ) : (
            <div style={{ display: "flex", alignItems: "center", justifyContent: "center", height: "100%", color: "var(--text-muted)" }}>
              Select or create a workflow to begin.
            </div>
          )}
        </div>
      </div>

      {/* Right: node config panel */}
      {selectedNode && (
        <div style={{ width: "260px", borderLeft: "1px solid var(--border-color)", padding: "12px", overflowY: "auto" }}>
          <div style={{ fontSize: "12px", fontWeight: 600, marginBottom: "10px", textTransform: "uppercase", color: "var(--text-muted)" }}>Node Config</div>
          <div style={{ marginBottom: "10px" }}>
            <label style={{ fontSize: "12px", display: "block", marginBottom: "4px" }}>Label</label>
            <input
              className="settings-input"
              value={(selectedNode.data as Record<string, unknown>).label as string ?? ""}
              onChange={(e) => {
                setNodes((nds) => nds.map((n) => n.id === selectedNode.id ? { ...n, data: { ...n.data, label: e.target.value } } : n));
              }}
              style={{ width: "100%", fontSize: "12px" }}
            />
          </div>
          {selectedNode.type === "agent" && (
            <div style={{ marginBottom: "10px" }}>
              <label style={{ fontSize: "12px", display: "block", marginBottom: "4px" }}>Agent</label>
              <select
                className="settings-input"
                value={(selectedNode.data as Record<string, unknown>).agentId as string ?? ""}
                onChange={(e) => {
                  setNodes((nds) => nds.map((n) => n.id === selectedNode.id ? { ...n, data: { ...n.data, agentId: e.target.value } } : n));
                }}
                style={{ width: "100%", fontSize: "12px" }}
              >
                <option value="">— select agent —</option>
                {agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
              </select>
            </div>
          )}
          {(() => {
            const downstream = edges.filter(e => e.source === selectedNode.id).map(e => {
              const tn = nodes.find(n => n.id === e.target);
              return { id: e.target, label: (tn?.data as Record<string, unknown>)?.label as string ?? e.target };
            });
            const nodeConfig = ((selectedNode.data as Record<string, unknown>).config ?? {}) as Record<string, unknown>;
            const router = (nodeConfig.router ?? null) as RouterConfig | null;
            return (
              <RouterEditor
                router={router}
                downstreamNodes={downstream}
                onChange={(newRouter) => {
                  setNodes((nds) => nds.map((n) => n.id === selectedNode.id ? { ...n, data: { ...n.data, config: { ...nodeConfig, router: newRouter } } } : n));
                }}
              />
            );
          })()}
          <button className="btn-secondary" style={{ fontSize: "12px", width: "100%" }} onClick={() => {
            setNodes((nds) => nds.filter((n) => n.id !== selectedNode.id));
            setEdges((eds) => eds.filter((e) => e.source !== selectedNode.id && e.target !== selectedNode.id));
            setSelectedNodeId(null);
          }}>
            Delete Node
          </button>
        </div>
      )}

      {/* Edge config panel */}
      {selectedEdge && (
        <EdgeConfigPanel
          edge={selectedEdge}
          onClose={() => setSelectedEdgeId(null)}
          onUpdate={(updates) => {
            setEdges((eds) => eds.map((e) => e.id === selectedEdge.id ? { ...e, ...updates } : e));
          }}
        />
      )}

      {/* Run results modal */}
      {showRunView && activeWorkflow && (
        <WorkflowRunView workflowId={activeWorkflow.id} onClose={() => setShowRunView(false)} />
      )}
    </div>
  );
}
