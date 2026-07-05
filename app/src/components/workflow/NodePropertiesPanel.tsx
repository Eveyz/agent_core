import { useMemo, useCallback, useRef } from "react";
import type { Node, Edge } from "@xyflow/react";
import type { AgentDef } from "../../features/agents/types";
import { RouterEditor, type RouterConfig } from "./EdgeConfigPanel";
import InfoIcon from "lucide-react/dist/esm/icons/info.mjs";
import "./NodePropertiesPanel.css";

// Named constants
const DEBOUNCE_MS = 300;

interface NodePropertiesPanelProps {
  selectedNode: Node;
  edges: Edge[];
  nodes: Node[];
  agents: AgentDef[];
  isExecuting: boolean;
  onUpdateNode: (nodeId: string, data: Record<string, unknown>) => void;
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
  const nodeData = (selectedNode.data || {}) as Record<string, unknown>;
  const nodeConfig = (nodeData.config || {}) as Record<string, unknown>;
  const outputConstraint = (nodeConfig.output_constraint as string) || "text";

  const debounceRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const debouncedUpdate = useCallback((data: Record<string, unknown>) => {
    if (debounceRef.current) clearTimeout(debounceRef.current);
    debounceRef.current = setTimeout(() => {
      onUpdateNode(selectedNode.id, data);
    }, DEBOUNCE_MS);
  }, [selectedNode.id, onUpdateNode]);

  // Downstream nodes for router editor — computed once, not on every render
  const downstream = useMemo(() =>
    edges.filter(e => e.source === selectedNode.id).map(e => {
      const tn = nodes.find(n => n.id === e.target);
      return { id: e.target, label: (tn?.data as Record<string, unknown> | undefined)?.label as string ?? e.target };
    }), [edges, nodes, selectedNode.id]);
  const router = (nodeConfig.router ?? null) as RouterConfig | null;

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
          <label htmlFor="np-label" className="node-properties-label">Label</label>
          <input
            id="np-label"
            className="settings-input node-properties-input"
            value={nodeData.label as string ?? ""}
            onChange={(e) => {
              const label = e.target.value;
              const updated = { ...nodeData, label };
              debouncedUpdate(updated);
            }}
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
              value={(nodeConfig.input_template as string) || ""}
              onChange={(e) => {
                const input_template = e.target.value;
                const updated = { ...nodeData, config: { ...nodeConfig, input_template } };
                debouncedUpdate(updated);
              }}
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
                value={(nodeConfig.model_override as string) || ""}
                onChange={(e) => {
                  const model_override = e.target.value;
                  const updated = { ...nodeData, config: { ...nodeConfig, model_override } };
                  debouncedUpdate(updated);
                }}
                placeholder="Leave blank for agent default"
              />
            </div>
            <div>
              <label className="node-properties-label">Max Iterations</label>
              <input
                type="number"
                className="settings-input node-properties-input"
                value={(nodeConfig.max_iterations_override as number) || ""}
                onChange={(e) => {
                  const val = e.target.value ? Number(e.target.value) : undefined;
                  const updated = { ...nodeData, config: { ...nodeConfig, max_iterations_override: val } };
                  debouncedUpdate(updated);
                }}
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
                    value={(nodeConfig.response_schema as string) || ""}
                    onChange={(e) => {
                      const response_schema = e.target.value;
                      const updated = { ...nodeData, config: { ...nodeConfig, response_schema } };
                      debouncedUpdate(updated);
                    }}
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
        <RouterEditor
          router={router}
          downstreamNodes={downstream}
          onChange={(newRouter) => {
            onUpdateNode(selectedNode.id, { ...nodeData, config: { ...nodeConfig, router: newRouter } });
          }}
        />
      </div>

      <div className="node-properties-footer">
        <button className="btn-secondary node-properties-delete-btn" onClick={() => onDeleteNode(selectedNode.id)}>
          Delete Node
        </button>
      </div>
    </div>
  );
}
