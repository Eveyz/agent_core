import { useState, useEffect, useRef, useMemo, useCallback } from 'react';
import { useSelector, useDispatch } from 'react-redux';
import { invoke } from '@tauri-apps/api/core';
import { RootState, AppDispatch } from '../../store';
import { saveConfig } from '../../features/settings/settingsSlice';
import { parseMarkdown } from '../chat/MarkdownContent';
import DatabaseIcon from 'lucide-react/dist/esm/icons/database.mjs';
import BrainIcon from 'lucide-react/dist/esm/icons/brain.mjs';
import LayersIcon from 'lucide-react/dist/esm/icons/layers.mjs';
import TypeIcon from 'lucide-react/dist/esm/icons/type.mjs';
import MergeIcon from 'lucide-react/dist/esm/icons/merge.mjs';
import ZapIcon from 'lucide-react/dist/esm/icons/zap.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import StarIcon from 'lucide-react/dist/esm/icons/star.mjs';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import FileTextIcon from 'lucide-react/dist/esm/icons/file-text.mjs';
import RefreshIcon from 'lucide-react/dist/esm/icons/refresh-cw.mjs';

type MemoryMode = 'stateless' | 'standard' | 'deep';

const MODE_DESCRIPTIONS: Record<MemoryMode, { label: string; desc: string }> = {
  stateless: {
    label: 'Stateless',
    desc: 'No memory. Each conversation starts fresh.',
  },
  standard: {
    label: 'Standard',
    desc: 'agverse.md core memory + vector recall tools.',
  },
  deep: {
    label: 'Deep',
    desc: 'Standard + background reflection daemon for auto fact extraction.',
  },
};

export default function MemoryTab() {
  const config = useSelector((state: RootState) => state.settings.config);
  const dispatch = useDispatch<AppDispatch>();
  const [agverseContent, setAgverseContent] = useState<string>('');
  const [loadingMd, setLoadingMd] = useState(true);
  const [switchingMode, setSwitchingMode] = useState(false);

  const loadAgverseMd = useCallback(async () => {
    setLoadingMd(true);
    try {
      const content = await invoke<string>('get_agverse_md');
      setAgverseContent(content);
    } catch (err) {
      console.error('Failed to load agverse.md', err);
      setAgverseContent('');
    } finally {
      setLoadingMd(false);
    }
  }, []);

  useEffect(() => {
    loadAgverseMd();
  }, [loadAgverseMd]);

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
        embedding_enabled: true,
        mode: 'standard',
        reflection: {
          trigger_message_count: 20,
          reflection_model: undefined,
        },
      }
    };
    try {
      await dispatch(saveConfig(newConfig));
      await loadAgverseMd();
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
                border: `2px solid ${currentMode === mode ? 'var(--accent)' : 'var(--border-color)'}`,
                background: currentMode === mode ? 'var(--accent-bg, rgba(99, 102, 241, 0.1))' : 'var(--bg-tertiary)',
                cursor: switchingMode ? 'wait' : 'pointer',
                display: 'flex',
                flexDirection: 'column',
                alignItems: 'flex-start',
                gap: '4px',
                transition: 'all 0.15s ease',
                opacity: switchingMode ? 0.6 : 1,
                color: 'var(--text-main)',
              }}
            >
              <div style={{ display: 'flex', alignItems: 'center', gap: '6px' }}>
                <span style={{ fontWeight: 600, fontSize: '13px' }}>
                  {MODE_DESCRIPTIONS[mode].label}
                </span>
                {currentMode === mode && <CheckIcon size={14} style={{ color: 'var(--accent)' }} />}
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
        <ReflectionModelSelector
          config={config}
          currentModel={memory.reflection?.reflection_model ?? ''}
          onChange={handleReflectionModelChange}
        />
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
            <span
              className={`badge badge-${memory.embedding_enabled ? 'enabled' : 'disabled'}`}
              style={{ cursor: 'pointer' }}
              onClick={async () => {
                if (!config || !memory) return;
                const newConfig = {
                  ...config,
                  memory: {
                    ...memory,
                    embedding_enabled: !memory.embedding_enabled,
                  }
                };
                try {
                  await dispatch(saveConfig(newConfig));
                } catch (e) {
                  console.error('Failed to toggle embedding', e);
                }
              }}
            >
              {memory.embedding_enabled ? 'Enabled' : 'Disabled (keyword search)'}
            </span>
          </div>
        </div>
      </div>

      {/* Core Memory (agverse.md) — live markdown view */}
      {currentMode !== 'stateless' && (
        <div className="settings-section" style={{ marginTop: '24px' }}>
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between' }}>
            <h3 className="settings-section-title" style={{ marginBottom: 0 }}>
              <FileTextIcon size={14} /> Core Memory (agverse.md)
            </h3>
            <button
              onClick={loadAgverseMd}
              disabled={loadingMd}
              style={{
                background: 'transparent',
                border: '1px solid var(--border-color)',
                borderRadius: '6px',
                padding: '3px 8px',
                cursor: loadingMd ? 'wait' : 'pointer',
                display: 'flex',
                alignItems: 'center',
                gap: '4px',
                fontSize: '11px',
                color: 'var(--text-secondary)',
              }}
            >
              <RefreshIcon size={12} className={loadingMd ? 'settings-spinner' : ''} />
              Refresh
            </button>
          </div>
          {loadingMd ? (
            <div className="settings-empty" style={{ padding: '20px 0', fontSize: '13px' }}>Loading...</div>
          ) : agverseContent ? (
            <div
              className="agverse-md-view assistant-msg"
              style={{ fontSize: '12px', lineHeight: '1.5', padding: '8px 0' }}
              dangerouslySetInnerHTML={parseMarkdown(agverseContent)}
            />
          ) : (
            <div className="settings-empty" style={{ padding: '20px 0', fontSize: '13px' }}>
              agverse.md not found. It will be created on first conversation.
            </div>
          )}
        </div>
      )}
    </div>
  );
}

// ── Reflection Model Dropdown (reuses model-selector CSS) ────────────

function ReflectionModelSelector({
  config,
  currentModel,
  onChange,
}: {
  config: NonNullable<ReturnType<typeof useSelector<RootState, any>>>;
  currentModel: string;
  onChange: (key: string) => void;
}) {
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
        setSearch('');
      }
    };
    document.addEventListener('mousedown', handler);
    return () => document.removeEventListener('mousedown', handler);
  }, [open]);

  useEffect(() => {
    if (open && searchInputRef.current) {
      searchInputRef.current.focus();
    }
  }, [open]);

  const groupedModels = useMemo(() => {
    const map = new Map<string, { key: string; displayName: string }[]>();
    if (!config) return map;
    Object.entries(config.providers).forEach(([providerKey, provider]: [string, any]) => {
      const items: { key: string; displayName: string }[] = [];
      Object.entries(provider.models).forEach(([modelKey]: [string, any]) => {
        items.push({
          key: `${providerKey}/${modelKey}`,
          displayName: modelKey,
        });
      });
      if (items.length > 0) {
        map.set(provider.name || providerKey, items);
      }
    });
    return map;
  }, [config]);

  const filteredGroups = useMemo(() => {
    if (!search.trim()) return groupedModels;
    const q = search.toLowerCase();
    const result = new Map<string, { key: string; displayName: string }[]>();
    groupedModels.forEach((models, provider) => {
      const matched = models.filter((m) => m.displayName.toLowerCase().includes(q));
      if (matched.length > 0) result.set(provider, matched);
    });
    return result;
  }, [groupedModels, search]);

  const handleSelect = useCallback((key: string) => {
    setOpen(false);
    setSearch('');
    onChange(key);
  }, [onChange]);

  const currentDisplay = currentModel
    ? currentModel.includes('/')
      ? currentModel.slice(currentModel.lastIndexOf('/') + 1)
      : currentModel
    : 'Disabled';

  return (
    <div className="settings-section" style={{ marginTop: '16px' }}>
      <h3 className="settings-section-title">
        <BrainIcon size={14} /> Reflection Model
      </h3>
      <p style={{ fontSize: '12px', marginTop: '8px', color: 'var(--text-secondary)', lineHeight: '1.4' }}>
        Background daemon extracts durable facts and writes them to agverse.md. Leave unselected to disable.
      </p>

      <div ref={dropdownRef} className="model-selector-wrapper" style={{ marginTop: '8px', position: 'relative' }}>
        <button
          className="model-selector"
          onClick={() => setOpen(!open)}
          style={{ width: '100%', justifyContent: 'space-between', padding: '6px 10px' }}
        >
          <span className="model-selector-text">
            {currentModel ? (
              <span className="model-selector-name">{currentDisplay}</span>
            ) : (
              <span style={{ color: 'var(--text-dim)' }}>— Disabled —</span>
            )}
          </span>
          <ChevronDownIcon size={12} className={`model-selector-chevron ${open ? 'open' : ''}`} />
        </button>

        {open && (
          <div className="model-dropdown" style={{ bottom: 'auto', top: 'calc(100% + 6px)', width: '100%' }}>
            <div className="model-dropdown-search">
              <SearchIcon size={14} />
              <input
                ref={searchInputRef}
                className="model-dropdown-search-input"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder="Search models..."
              />
            </div>

            <div className="model-dropdown-list">
              {/* Disabled option */}
              <button
                className={`model-dropdown-item ${!currentModel ? 'selected' : ''}`}
                onClick={() => handleSelect('')}
              >
                <span className="model-dropdown-item-key" style={{ color: 'var(--text-dim)' }}>— Disabled —</span>
                {!currentModel && <StarIcon size={12} className="model-dropdown-item-star" />}
              </button>

              {Array.from(filteredGroups.entries()).map(([provider, models]) => (
                <div key={provider} className="model-dropdown-group">
                  <div className="model-dropdown-group-header">
                    <ServerIcon size={12} />
                    <span>{provider}</span>
                  </div>
                  {models.map((model) => (
                    <button
                      key={model.key}
                      className={`model-dropdown-item ${model.key === currentModel ? 'selected' : ''}`}
                      onClick={() => handleSelect(model.key)}
                    >
                      <span className="model-dropdown-item-key">{model.displayName}</span>
                      {model.key === currentModel && <StarIcon size={12} className="model-dropdown-item-star" />}
                    </button>
                  ))}
                </div>
              ))}
            </div>
          </div>
        )}
      </div>

      {currentModel && (
        <p style={{ fontSize: '11px', marginTop: '6px', opacity: 0.6, color: 'var(--text-secondary)' }}>
          Restart required for changes to take effect.
        </p>
      )}
    </div>
  );
}
