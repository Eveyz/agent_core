import { useRef, useEffect } from "react";
import {
  ReactFlow,
  Background,
  Controls,
  MiniMap,
  type Node,
  type Edge,
  type Connection,
  type NodeTypes,
  type NodeChange,
  type EdgeChange,
  useReactFlow,
} from "@xyflow/react";
import { nodeTypes } from "./nodes";
import type { NodeType } from "../../features/workflow/types";
import "./WorkflowCanvas.css";

interface WorkflowCanvasProps {
  nodes: Node[];
  edges: Edge[];
  onNodesChange: (changes: NodeChange[]) => void;
  onEdgesChange: (changes: EdgeChange[]) => void;
  onConnect: (params: Connection) => void;
  onNodeClick: (_: unknown, node: Node) => void;
  onEdgeClick: (_: unknown, edge: Edge) => void;
  onPaneClick: () => void;
  onDropNode: (node: Node) => void;
}

export function WorkflowCanvas({
  nodes,
  edges,
  onNodesChange,
  onEdgesChange,
  onConnect,
  onNodeClick,
  onEdgeClick,
  onPaneClick,
  onDropNode
}: WorkflowCanvasProps) {
  const { screenToFlowPosition } = useReactFlow();
  const containerRef = useRef<HTMLDivElement>(null);

  // Store latest callback in a ref so the native listener always sees the current version
  const onDropNodeRef = useRef(onDropNode);
  onDropNodeRef.current = onDropNode;
  const screenToFlowRef = useRef(screenToFlowPosition);
  screenToFlowRef.current = screenToFlowPosition;

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    const handleDragOver = (e: globalThis.DragEvent) => {
      e.preventDefault();
      if (e.dataTransfer) {
        e.dataTransfer.dropEffect = "move";
      }
    };

    const handleDrop = (e: globalThis.DragEvent) => {
      e.preventDefault();
      try {
        const dataStr =
          e.dataTransfer?.getData("application/reactflow") ||
          e.dataTransfer?.getData("text/plain");
        if (!dataStr) {
          return;
        }

        const payload = JSON.parse(dataStr);
        const type = payload.type as NodeType | "agent";
        if (!type) {
          return;
        }

        const id = `${type}-${Date.now()}`;
        const agentId = payload.agentId || "";
        const agentName = payload.agentName || "";
        const label =
          agentName || type.charAt(0).toUpperCase() + type.slice(1);

        const position = screenToFlowRef.current({
          x: e.clientX,
          y: e.clientY,
        });

        const newNode: Node = {
          id,
          type,
          position,
          data: { label, nodeType: type, agentId, config: {} },
        };

        onDropNodeRef.current(newNode);
      } catch (err) {
        console.error("Failed to parse dropped node data", err);
      }
    };

    el.addEventListener("dragover", handleDragOver);
    el.addEventListener("drop", handleDrop);

    return () => {
      el.removeEventListener("dragover", handleDragOver);
      el.removeEventListener("drop", handleDrop);
    };
  }, []);

  return (
    <div className="workflow-canvas-container" ref={containerRef}>
      <ReactFlow
        nodes={nodes}
        edges={edges}
        onNodesChange={onNodesChange}
        onEdgesChange={onEdgesChange}
        onConnect={onConnect}
        onNodeClick={onNodeClick}
        onEdgeClick={onEdgeClick}
        onPaneClick={onPaneClick}
        nodeTypes={nodeTypes as unknown as NodeTypes}
        fitView
      >
        <Background color="var(--border-color, var(--overlay-0_1))" gap={20} />
        <Controls className="workflow-canvas-controls" />
        <MiniMap
          className="workflow-canvas-minimap"
          nodeColor="rgba(59, 130, 246, 0.4)"
          maskColor="rgba(0, 0, 0, 0.4)"
        />
      </ReactFlow>
    </div>
  );
}

