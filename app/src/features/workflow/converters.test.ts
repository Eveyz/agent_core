import { describe, expect, it } from 'vitest';
import type { EdgeDef } from './types';
import { edgeDefToRF, rfToEdgeDef } from './converters';

describe('workflow edge converters', () => {
  it('preserves handles, conditions, and data mappings across an editor round trip', () => {
    const edge: EdgeDef = {
      id: 'edge-1',
      workflow_id: 'workflow-1',
      source_node_id: 'source',
      target_node_id: 'target',
      source_handle: 'success',
      target_handle: 'input',
      label: 'result',
      condition: '$.approved == true',
      data_mapping: {
        pass_through: false,
        source_field: 'answer',
        target_field: 'review',
      },
      created_at: '',
    };

    const roundTripped = rfToEdgeDef(edgeDefToRF(edge), edge.workflow_id);

    expect(roundTripped).toMatchObject({
      source_handle: edge.source_handle,
      target_handle: edge.target_handle,
      condition: edge.condition,
      data_mapping: edge.data_mapping,
    });
  });
});
