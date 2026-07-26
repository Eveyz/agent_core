import type { Node, Edge } from '@xyflow/react';
import type { NodeDef, EdgeDef, NodeType } from './types';

export function nodeDefToRF(n: NodeDef): Node {
  return {
    id: n.id,
    type: n.node_type,
    position: { x: n.position_x, y: n.position_y },
    data: { label: n.label, nodeType: n.node_type, agentId: n.agent_id, config: n.config },
  };
}

export function rfToNodeDef(n: Node, workflowId: string): NodeDef {
  const data = (n.data ?? {}) as Record<string, unknown>;
  return {
    id: n.id,
    workflow_id: workflowId,
    node_type: (n.type ?? 'transform') as NodeType,
    label: (data.label as string) ?? '',
    agent_id: (data.agentId as string) ?? '',
    config: (data.config as Record<string, unknown>) ?? {},
    position_x: n.position.x,
    position_y: n.position.y,
    created_at: '',
  };
}

export function edgeDefToRF(e: EdgeDef): Edge {
  return {
    id: e.id,
    source: e.source_node_id,
    target: e.target_node_id,
    sourceHandle: e.source_handle || undefined,
    targetHandle: e.target_handle || undefined,
    label: e.label || undefined,
    data: {
      condition: e.condition,
      data_mapping: e.data_mapping,
    },
    animated: true,
  };
}

export function rfToEdgeDef(e: Edge, workflowId: string): EdgeDef {
  return {
    id: e.id,
    workflow_id: workflowId,
    source_node_id: e.source,
    target_node_id: e.target,
    source_handle: (e.sourceHandle as string) ?? '',
    target_handle: (e.targetHandle as string) ?? '',
    label: (e.label as string) ?? '',
    condition: ((e.data as Record<string, unknown> | undefined)?.condition as string) ?? '',
    data_mapping:
      ((e.data as Record<string, unknown> | undefined)?.data_mapping as Record<string, unknown>)
      ?? { pass_through: true },
    created_at: '',
  };
}
