import { useState, useEffect } from 'react';
import { useSelector } from 'react-redux';
import { invoke } from '@tauri-apps/api/core';
import { RootState } from '../../store';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import LayersIcon from 'lucide-react/dist/esm/icons/layers.mjs';
import TypeIcon from 'lucide-react/dist/esm/icons/type.mjs';
import MergeIcon from 'lucide-react/dist/esm/icons/merge.mjs';

interface MemoryBlock {
  id: string;
  label: string;
  content: string;
  max_chars: number;
  updated_at: string;
}

export default function MemoryTab() {
  const config = useSelector((state: RootState) => state.settings.config);
  const [blocks, setBlocks] = useState<MemoryBlock[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    async function loadBlocks() {
      try {
        const data = await invoke<MemoryBlock[]>('get_memory_blocks');
        setBlocks(data);
      } catch (err) {
        console.error('Failed to load memory blocks', err);
      } finally {
        setLoading(false);
      }
    }
    loadBlocks();
  }, []);

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const memory = config.memory;

  if (!memory) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">Memory is not configured. Add a [memory] section to config.toml to enable it.</div>
      </div>
    );
  }

  return (
    <div className="settings-tab-content">
      <div className="settings-section">
        <h3 className="settings-section-title">
          <BrainIcon size={14} /> Memory Configuration
        </h3>

        <div className="settings-field">
          <DatabaseIcon size={12} />
          <label className="settings-label">Database Path</label>
          <div className="settings-value">{memory.db_path}</div>
        </div>

        <div className="settings-field">
          <BrainIcon size={12} />
          <label className="settings-label">Embedding Model</label>
          <div className="settings-value">{memory.embedding_model}</div>
        </div>

        <div className="settings-field">
          <LayersIcon size={12} />
          <label className="settings-label">Max Core Blocks</label>
          <div className="settings-value">{memory.max_core_blocks}</div>
        </div>

        <div className="settings-field">
          <TypeIcon size={12} />
          <label className="settings-label">Block Max Chars</label>
          <div className="settings-value">{(memory.default_block_max_chars ?? 0).toLocaleString()}</div>
        </div>

        <div className="settings-field">
          <MergeIcon size={12} />
          <label className="settings-label">Consolidation</label>
          <div className="settings-value">
            <span className={`badge badge-${memory.consolidation_enabled ? 'enabled' : 'disabled'}`}>
              {memory.consolidation_enabled ? 'Enabled' : 'Disabled'}
            </span>
          </div>
        </div>
      </div>

      <div className="settings-section" style={{ marginTop: '24px' }}>
        <h3 className="settings-section-title">
          <LayersIcon size={14} /> Core Memory Blocks
        </h3>
        {loading ? (
          <div className="settings-empty" style={{ padding: '20px 0', fontSize: '13px' }}>Loading memory blocks...</div>
        ) : blocks.length === 0 ? (
          <div className="settings-empty" style={{ padding: '20px 0', fontSize: '13px' }}>No memory blocks found.</div>
        ) : (
          <div style={{ display: 'flex', flexDirection: 'column', gap: '12px', marginTop: '12px' }}>
            {blocks.map(block => (
              <div key={block.id} style={{ 
                background: 'var(--bg-tertiary)', 
                border: '1px solid var(--border-color)', 
                borderRadius: '8px', 
                padding: '12px',
                display: 'flex',
                flexDirection: 'column',
                gap: '8px'
              }}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
                  <div style={{ fontWeight: 500, fontSize: '13px', color: 'var(--text-primary)' }}>
                    {block.label} <span style={{ opacity: 0.5, fontSize: '11px', marginLeft: '4px' }}>({block.id})</span>
                  </div>
                  <div style={{ fontSize: '11px', color: 'var(--text-secondary)' }}>
                    {new Date(block.updated_at).toLocaleString()}
                  </div>
                </div>
                <div style={{ 
                  fontSize: '13px', 
                  color: 'var(--text-secondary)',
                  whiteSpace: 'pre-wrap',
                  lineHeight: '1.4'
                }}>
                  {block.content || <span style={{ opacity: 0.5, fontStyle: 'italic' }}>Empty</span>}
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
