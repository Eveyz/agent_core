import { useState, useEffect } from 'react';
import { useSelector, useDispatch } from 'react-redux';
import { invoke } from '@tauri-apps/api/core';
import { RootState, AppDispatch } from '../../store';
import { saveConfig } from '../../features/settings/settingsSlice';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import LayersIcon from 'lucide-react/dist/esm/icons/layers.mjs';
import TypeIcon from 'lucide-react/dist/esm/icons/type.mjs';
import MergeIcon from 'lucide-react/dist/esm/icons/merge.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';

interface MemoryBlock {
  id: string;
  label: string;
  content: string;
  max_chars: number;
  updated_at: string;
}

type MemoryMode = 'stateless' | 'standard' | 'deep';

const MODE_DESCRIPTIONS: Record<MemoryMode, { label: string; desc: string }> = {
  stateless: {
    label: 'Stateless',
    desc: 'No memory. Each conversation starts fresh. No recall, no agverse.md injection.',
  },
  standard: {
    label: 'Standard',
    desc: 'Dual-track: core memory blocks + vector recall + project docs. Agent can proactively manage memory.',
  },
  deep: {
    label: 'Deep',
    desc: 'Standard + proactive recall guidance + background reflection readiness. Best for long-term complex projects.',
  },
};

export default function MemoryTab() {
  const config = useSelector((state: RootState) => state.settings.config);
  const dispatch = useDispatch<AppDispatch>();
  const [blocks, setBlocks] = useState<MemoryBlock[]>([]);
  const [loading, setLoading] = useState(true);
  const [switchingMode, setSwitchingMode] = useState(false);

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
  const currentMode = (memory?.mode as MemoryMode) ?? 'standard';

  const handleEnableMemory = async () => {
    if (!config) return;
    const newConfig = {
      ...config,
      memory: {
        db_path: '~/.agverse/memory.db',
        embedding_model: 'BAAI/bge-small-en-v1.5',
        max_core_blocks: 5,
        default_block_max_chars: 2000,
        consolidation_enabled: true,
        embedding_enabled: false,
        mode: 'standard',
        reflection: {
          trigger_message_count: 20,
          reflection_model: undefined,
        },
      }
    };
    try {
      await dispatch(saveConfig(newConfig));
      const data = await invoke<MemoryBlock[]>('get_memory_blocks');
      setBlocks(data);
    } catch (e) {
      console.error('Failed to enable memory', e);
    }
  };

  const handleModeChange = async (mode: MemoryMode) => {
    if (!config || !memory) return;
    setSwitchingMode(true);
    const newConfig = {
      ...config,
      memory: {
        ...memory,
        mode,
      }
    };
    try {
      await dispatch(saveConfig(newConfig));
    } catch (e) {
      console.error('Failed to switch memory mode', e);
    } finally {
      setSwitchingMode(false);
    }
  };

  const handleReflectionModelChange = async (modelKey: string) => {
    if (!config || !memory) return;
    const newConfig = {
      ...config,
      memory: {
        ...memory,
        reflection: {
          trigger_message_count: memory.reflection?.trigger_message_count ?? 20,
          reflection_model: modelKey || undefined,
        },
      }
    };
    try {
      await dispatch(saveConfig(newConfig));
    } catch (e) {
      console.error('Failed to set reflection model', e);
    }
  };

  // Build list of available models from providers
  const availableModels: { key: string; label: string }[] = [];
  if (config) {
    for (const [providerKey, provider] of Object.entries(config.providers)) {
      for (const [modelKey, model] of Object.entries(provider.models)) {
        const fullKey = `${providerKey}/${modelKey}`;
        availableModels.push({ key: fullKey, label: `${fullKey} (${model.model_id})` });
      }
    }
  }

  if (!memory) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">
          <BrainIcon size={32} style={{ marginBottom: '12px', opacity: 0.5 }} />
          <p>Memory is not configured.</p>
          <p style={{ fontSize: '12px', marginTop: '8px', opacity: 0.6 }}>
            Enable it to give your agent long-term memory.
          </p>
          <button 
            className="btn btn-primary" 
            style={{ marginTop: '16px' }} 
            onClick={handleEnableMemory}
          >
            Enable Memory
          </button>
        </div>
      </div>
    );
  }

  return (
    <div className="settings-tab-content">
      {/* Memory Mode Selector */}
      <div className="settings-section">
        <h3 className="settings-section-title">
          <ZapIcon size={14} /> Memory Mode
        </h3>
        <div style={{ display: 'flex', gap: '8px', marginTop: '12px' }}>
          {(['stateless', 'standard', 'deep'] as MemoryMode[]).map((mode) => (
            <button
              key={mode}
              onClick={() => handleModeChange(mode)}
              disabled={switchingMode}
              style={{
                flex: 1,
                padding: '12px',
                borderRadius: '8px',
                border: `2px solid ${currentMode === mode ? 'var(--accent-color, #6366f1)' : 'var(--border-color)'}`,
                background: currentMode === mode ? 'var(--accent-bg, rgba(99, 102, 241, 0.1))' : 'var(--bg-tertiary)',
                cursor: switchingMode ? 'wait' : 'pointer',
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'flex-start',
                gap: '4px',
                transition: 'all 0.15s ease',
                opacity: switchingMode ? 0.6 : 1,
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                <span style={{ fontWeight: 600, fontSize: '13px' }}>
                  {MODE_DESCRIPTIONS[mode].label}
                </span>
                {currentMode === mode && <CheckIcon size={14} style={{ color: 'var(--accent-color, #6366f1)' }} />}
              </div>
              <span style={{ fontSize: '11px', color: 'var(--text-secondary)', textAlign: 'left', lineHeight: '1.3' }}>
                {MODE_DESCRIPTIONS[mode].desc}
              </span>
            </button>
          ))}
        </div>
        {currentMode !== 'standard' && (
          <p style={{ fontSize: '11px', marginTop: '8px', opacity: 0.6, color: 'var(--text-secondary)' }}>
            Restart required for mode changes to take full effect.
          </p>
        )}
      </div>

      {/* Reflection Model Selector (Deep mode only) */}
      {currentMode === 'deep' && (
        <div className="settings-section" style={{ marginTop: '16px' }}>
          <h3 className="settings-section-title">
            <BrainIcon size={14} /> Reflection Model
          </h3>
          <p style={{ fontSize: '12px', marginTop: '8px', color: 'var(--text-secondary)', lineHeight: '1.4' }}>
            Select a model for background fact extraction. Every {memory.reflection?.trigger_message_count ?? 20} messages, the daemon will use this model to extract durable facts and store them in archival memory. Leave unselected to disable LLM reflection.
          </p>
          <select
            value={memory.reflection?.reflection_model ?? ''}
            onChange={(e) => handleReflectionModelChange(e.target.value)}
            style={{
              marginTop: '8px',
              width: '100%',
              padding: '8px 10px',
              borderRadius: '6px',
              border: '1px solid var(--border-color)',
              background: 'var(--bg-tertiary)',
              color: 'var(--text-primary)',
              fontSize: '13px',
            }}
          >
            <option value="">— Disabled (no LLM reflection) —</option>
            {availableModels.map((m) => (
              <option key={m.key} value={m.key}>{m.label}</option>
            ))}
          </select>
          {memory.reflection?.reflection_model && (
            <p style={{ fontSize: '11px', marginTop: '6px', opacity: 0.6, color: 'var(--text-secondary)' }}>
              Restart required for changes to take effect.
            </p>
          )}
        </div>
      )}

      <div className="settings-section" style={{ marginTop: '24px' }}>
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

        <div className="settings-field">
          <BrainIcon size={12} />
          <label className="settings-label">Vector Embedding</label>
          <div className="settings-value">
            <span className={`badge badge-${memory.embedding_enabled ? 'enabled' : 'disabled'}`}>
              {memory.embedding_enabled ? 'Enabled' : 'Disabled (keyword search)'}
            </span>
          </div>
        </div>
      </div>

      {currentMode !== 'stateless' && (
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
      )}
    </div>
  );
}
