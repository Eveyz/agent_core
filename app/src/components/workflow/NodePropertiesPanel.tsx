import type { Node, Edge } from "@xyflow/react";
import type { AgentDef } from "../../features/agents/types";
import { RouterEditor, type RouterConfig } from "./EdgeConfigPanel";
import InfoIcon from "lucide-react/dist/esm/icons/info.mjs";
import "./NodePropertiesPanel.css";

interface NodePropertiesPanelProps {
  selectedNode: Node;
  edges: Edge[];
  nodes: Node[];
  agents: AgentDef[];
  isExecuting: boolean;
  onUpdateNode: (nodeId: string, data: any) => void;
  onDeleteNode: (nodeId: string) => void;
}

export function NodePropertiesPanel({
  selectedNode,
  edges,
  nodes,
  agents,
  isExecuting,
  onUpdateNode,
  onDeleteNode,
}: NodePropertiesPanelProps) {
  const nodeData = (selectedNode.data || {}) as Record<string, any>;
  const nodeConfig = (nodeData.config || {}) as Record<string, any>;
  const outputConstraint = nodeConfig.output_constraint || "text";
  
  if (isExecuting) {
    return (
      <div className="node-properties-locked">
        Properties locked while workflow is running.
      </div>
    );
  }

  return (
    <div className="node-properties-panel">
      <div>
        <div className="node-properties-title">Node Properties</div>
        
        <div className="node-properties-group">
          <label className="node-properties-label">Label</label>
          <input
            className="settings-input node-properties-input"
            value={nodeData.label as string ?? ""}
            onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, label: e.target.value })}
          />
        </div>

        {selectedNode.type === "agent" && (
          <div className="node-properties-group">
            <label className="node-properties-label">Agent</label>
            <select
              className="settings-input node-properties-input"
              value={nodeData.agentId as string ?? ""}
              onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, agentId: e.target.value })}
            >
              <option value="">— select agent —</option>
              {agents.map((a) => <option key={a.id} value={a.id}>{a.name}</option>)}
            </select>
          </div>
        )}
      </div>

      {selectedNode.type === "agent" && (
        <div className="node-properties-section">
          <div>
            <label className="node-properties-label">Input Template</label>
            <textarea
              className="settings-input node-properties-input"
              value={nodeConfig.input_template || ""}
              onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, input_template: e.target.value } })}
              placeholder="e.g. {node_1.output} please analyze..."
              style={{ minHeight: "80px", resize: "vertical" }}
            />
            <div className="node-properties-help-text">Available vars: {"{node_id.output}"}</div>
          </div>

          <div className="node-properties-grid">
            <div>
              <label className="node-properties-label">Model Override</label>
              <input
                className="settings-input node-properties-input"
                value={nodeConfig.model_override || ""}
                onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, model_override: e.target.value } })}
                placeholder="Leave blank for agent default"
              />
            </div>
            <div>
              <label className="node-properties-label">Max Iterations</label>
              <input
                type="number"
                className="settings-input node-properties-input"
                value={nodeConfig.max_iterations_override || ""}
                onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, max_iterations_override: e.target.value ? Number(e.target.value) : undefined } })}
                placeholder="Default"
              />
            </div>
          </div>

          <div>
            <label className="node-properties-label">Output Constraint</label>
            <select
              className="settings-input node-properties-input"
              value={outputConstraint}
              onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, output_constraint: e.target.value } })}
            >
              <option value="text">Text (Default)</option>
              <option value="json">JSON Object</option>
            </select>

            {outputConstraint === "json" && (
              <div style={{ marginTop: "12px", display: "flex", flexDirection: "column", gap: "12px" }}>
                <div className="node-properties-json-info">
                  <InfoIcon size={14} color="var(--accent)" style={{ flexShrink: 0, marginTop: "2px" }} />
                  <div className="node-properties-json-info-text">
                    System will automatically append strict JSON formatting instructions and attempt API-level enforcement if the model supports it.
                  </div>
                </div>
                <div>
                  <label className="node-properties-schema-label">JSON Schema (Optional)</label>
                  <textarea
                    className="settings-input node-properties-schema-input"
                    value={nodeConfig.response_schema || ""}
                    onChange={(e) => onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, response_schema: e.target.value } })}
                    placeholder='{"type": "object", "properties": {...}}'
                    spellCheck={false}
                  />
                </div>
              </div>
            )}
          </div>
        </div>
      )}

      <div className="node-properties-section">
        {(() => {
          const downstream = edges.filter(e => e.source === selectedNode.id).map(e => {
            const tn = nodes.find(n => n.id === e.target);
            return { id: e.target, label: (tn?.data as Record<string, unknown>)?.label as string ?? e.target };
          });
          const router = (nodeConfig.router ?? null) as RouterConfig | null;
          return (
            <RouterEditor
              router={router}
              downstreamNodes={downstream}
              onChange={(newRouter) => {
                onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, router: newRouter } });
              }}
            />
          );
        })()}
      </div>

      <div className="node-properties-footer">
        <button className="btn-secondary node-properties-delete-btn" onClick={() => onDeleteNode(selectedNode.id)}>
          Delete Node
        </button>
      </div>
    </div>
  );
}
