import { useState, useEffect } from "react";
import type { Edge } from "@xyflow/react";
import XIcon from "lucide-react/dist/esm/icons/x.mjs";

export interface RouterRule {
  field: string;
  op: string;
  value: string;
  targets: string[];
}

export interface RouterConfig {
  rules: RouterRule[];
  default: string[];
}

const OPERATORS = ["==", "!=", ">", ">=", "<", "<=", "contains"] as const;

/**
 * EdgeConfigPanel — configures a selected edge's data mapping and label.
 *
 * Also doubles as a node-level router editor when a node is selected
 * (router rules are stored in the node's `config.router`).
 */
export function EdgeConfigPanel({
  edge,
  onClose,
  onUpdate,
}: {
  edge: Edge;
  onClose: () => void;
  onUpdate: (updates: Partial<Edge>) => void;
}) {
  const [label, setLabel] = useState((edge.label as string) ?? "");
  const [passThrough, setPassThrough] = useState(true);
  const [sourceField, setSourceField] = useState("");
  const [targetField, setTargetField] = useState("");

  useEffect(() => {
    setLabel((edge.label as string) ?? "");
    // Parse existing data_mapping from edge data if present
    const dm = (edge.data as Record<string, unknown> | undefined)?.data_mapping as Record<string, unknown> | undefined;
    if (dm) {
      setPassThrough(dm.pass_through !== false);
      setSourceField((dm.source_field as string) ?? "");
      setTargetField((dm.target_field as string) ?? "");
    }
  }, [edge]);

  const applyLabel = () => {
    onUpdate({ label });
  };

  return (
    <div style={{ width: "280px", borderLeft: "1px solid var(--border-color)", padding: "12px", overflowY: "auto" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "12px" }}>
        <span style={{ fontSize: "12px", fontWeight: 600, textTransform: "uppercase", color: "var(--text-muted)" }}>Edge Config</span>
        <button className="icon-btn" onClick={onClose}><XIcon size={14} /></button>
      </div>

      <div style={{ marginBottom: "12px" }}>
        <label style={{ fontSize: "12px", display: "block", marginBottom: "4px" }}>Label</label>
        <input
          className="settings-input"
          value={label}
          onChange={(e) => setLabel(e.target.value)}
          onBlur={applyLabel}
          placeholder="Edge label (used as input key)"
          style={{ width: "100%", fontSize: "12px" }}
        />
      </div>

      <div style={{ fontSize: "11px", fontWeight: 600, color: "var(--text-muted)", marginBottom: "6px", textTransform: "uppercase" }}>Data Mapping</div>
      <div style={{ marginBottom: "8px" }}>
        <label style={{ display: "flex", alignItems: "center", gap: "6px", fontSize: "12px", cursor: "pointer" }}>
          <input
            type="checkbox"
            checked={passThrough}
            onChange={(e) => setPassThrough(e.target.checked)}
          />
          Pass through entire output
        </label>
      </div>
      {!passThrough && (
        <div style={{ display: "flex", flexDirection: "column", gap: "8px" }}>
          <div>
            <label style={{ fontSize: "12px", display: "block", marginBottom: "4px" }}>Source Field</label>
            <input
              className="settings-input"
              value={sourceField}
              onChange={(e) => setSourceField(e.target.value)}
              placeholder="e.g. result"
              style={{ width: "100%", fontSize: "12px" }}
            />
          </div>
          <div>
            <label style={{ fontSize: "12px", display: "block", marginBottom: "4px" }}>Target Field</label>
            <input
              className="settings-input"
              value={targetField}
              onChange={(e) => setTargetField(e.target.value)}
              placeholder="(defaults to source)"
              style={{ width: "100%", fontSize: "12px" }}
            />
          </div>
        </div>
      )}
      <div style={{ fontSize: "11px", color: "var(--text-muted)", marginTop: "8px" }}>
        {passThrough
          ? "The entire upstream output is passed to this node, keyed by the edge label."
          : `Extracts "${sourceField || "…"}" from upstream output and maps it to "${targetField || sourceField || "…"}".`}
      </div>
    </div>
  );
}

/**
 * RouterEditor — edits a node's conditional routing rules.
 * Stored in node.config.router.
 */
export function RouterEditor({
  router,
  downstreamNodes,
  onChange,
}: {
  router: RouterConfig | null;
  downstreamNodes: { id: string; label: string }[];
  onChange: (router: RouterConfig) => void;
}) {
  const cfg: RouterConfig = router ?? { rules: [], default: [] };

  const updateRule = (idx: number, field: keyof RouterRule, value: string) => {
    const rules = [...cfg.rules];
    rules[idx] = { ...rules[idx], [field]: value };
    onChange({ ...cfg, rules });
  };

  const updateRuleTargets = (idx: number, targetId: string) => {
    const rules = [...cfg.rules];
    const current = new Set(rules[idx].targets);
    if (current.has(targetId)) current.delete(targetId);
    else current.add(targetId);
    rules[idx] = { ...rules[idx], targets: [...current] };
    onChange({ ...cfg, rules });
  };

  const addRule = () => {
    onChange({
      ...cfg,
      rules: [...cfg.rules, { field: "success", op: "==", value: "true", targets: [] }],
    });
  };

  const removeRule = (idx: number) => {
    const rules = cfg.rules.filter((_, i) => i !== idx);
    onChange({ ...cfg, rules });
  };

  const toggleDefault = (targetId: string) => {
    const current = new Set(cfg.default);
    if (current.has(targetId)) current.delete(targetId);
    else current.add(targetId);
    onChange({ ...cfg, default: [...current] });
  };

  if (downstreamNodes.length === 0) {
    return (
      <div style={{ fontSize: "11px", color: "var(--text-muted)", marginTop: "8px" }}>
        Connect downstream nodes to configure routing.
      </div>
    );
  }

  return (
    <div style={{ marginTop: "12px" }}>
      <div style={{ display: "flex", justifyContent: "space-between", alignItems: "center", marginBottom: "8px" }}>
        <span style={{ fontSize: "11px", fontWeight: 600, color: "var(--text-muted)", textTransform: "uppercase" }}>Router (Conditional)</span>
        <button className="btn-secondary" style={{ fontSize: "11px", padding: "2px 6px" }} onClick={addRule}>+ Rule</button>
      </div>

      {cfg.rules.map((rule, idx) => (
        <div key={idx} style={{ border: "1px solid var(--border-color)", borderRadius: "6px", padding: "8px", marginBottom: "6px" }}>
          <div style={{ display: "flex", gap: "4px", marginBottom: "6px" }}>
            <input
              className="settings-input"
              value={rule.field}
              onChange={(e) => updateRule(idx, "field", e.target.value)}
              placeholder="field"
              style={{ flex: 1, fontSize: "11px" }}
            />
            <select
              className="settings-input"
              value={rule.op}
              onChange={(e) => updateRule(idx, "op", e.target.value)}
              style={{ width: "70px", fontSize: "11px" }}
            >
              {OPERATORS.map(op => <option key={op} value={op}>{op}</option>)}
            </select>
            <input
              className="settings-input"
              value={rule.value}
              onChange={(e) => updateRule(idx, "value", e.target.value)}
              placeholder="value"
              style={{ flex: 1, fontSize: "11px" }}
            />
            <button className="icon-btn" style={{ padding: "2px" }} onClick={() => removeRule(idx)}>
              <XIcon size={12} />
            </button>
          </div>
          <div style={{ fontSize: "10px", color: "var(--text-muted)", marginBottom: "4px" }}>→ Targets:</div>
          <div style={{ display: "flex", flexDirection: "column", gap: "2px" }}>
            {downstreamNodes.map(n => {
              const checked = rule.targets.includes(n.id);
              return (
                <label key={n.id} style={{ display: "flex", alignItems: "center", gap: "4px", fontSize: "11px", cursor: "pointer" }}>
                  <input type="checkbox" checked={checked} onChange={() => updateRuleTargets(idx, n.id)} />
                  {n.label}
                </label>
              );
            })}
          </div>
        </div>
      ))}

      <div style={{ fontSize: "10px", color: "var(--text-muted)", marginTop: "8px", marginBottom: "4px" }}>Default targets (when no rule matches):</div>
      <div style={{ display: "flex", flexDirection: "column", gap: "2px" }}>
        {downstreamNodes.map(n => {
          const checked = cfg.default.includes(n.id);
          return (
            <label key={n.id} style={{ display: "flex", alignItems: "center", gap: "4px", fontSize: "11px", cursor: "pointer" }}>
              <input type="checkbox" checked={checked} onChange={() => toggleDefault(n.id)} />
              {n.label}
            </label>
          );
        })}
      </div>
    </div>
  );
}
