import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { useSelector } from 'react-redux';
import { useTranslation } from 'react-i18next';
import { RootState } from '../../store';
import {
  switchModel,
  updateModelSettings,
  type ProviderModelEntry,
  type AppConfig,
} from '../../features/settings/settingsSlice';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import ChevronRightIcon from 'lucide-react/dist/esm/icons/chevron-right.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import CheckIcon from 'lucide-react/dist/esm/icons/check.mjs';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import {
  lookupCapabilities,
  formatContextLabel,
  effectiveContextTokens,
  type ModelCapabilities,
} from '../../utils/modelCapabilities';

interface ModelItem {
  key: string;
  displayName: string;
  providerKey: string;
  providerName: string;
  modelId: string;
  entry: ProviderModelEntry;
  providerDefaultContext: number;
  caps: ModelCapabilities;
  contextTokens: number;
}

function parseModelKey(key: string): { providerKey: string; entryKey: string } | null {
  const slash = key.indexOf('/');
  if (slash < 0) return null;
  return { providerKey: key.slice(0, slash), entryKey: key.slice(slash + 1) };
}

function buildItems(config: AppConfig): Map<string, ModelItem[]> {
  const map = new Map<string, ModelItem[]>();
  Object.entries(config.providers).forEach(([providerKey, provider]) => {
    const items: ModelItem[] = [];
    Object.entries(provider.models).forEach(([modelKey, entry]) => {
      const caps = lookupCapabilities(entry.model_id);
      const contextTokens = effectiveContextTokens(
        entry.model_id,
        entry.max_context_tokens,
        provider.max_context_tokens
      );
      items.push({
        key: `${providerKey}/${modelKey}`,
        displayName: modelKey,
        providerKey,
        providerName: provider.name || providerKey,
        modelId: entry.model_id,
        entry,
        providerDefaultContext: provider.max_context_tokens,
        caps,
        contextTokens,
      });
    });
    if (items.length > 0) {
      map.set(provider.name || providerKey, items);
    }
  });
  return map;
}

function modelTriggerLabel(item: ModelItem | null, fallback: string): string {
  if (!item) return fallback;
  return item.displayName;
}

function Toggle({
  on,
  onChange,
  disabled,
}: {
  on: boolean;
  onChange: (v: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      className={`model-toggle ${on ? 'on' : ''} ${disabled ? 'disabled' : ''}`}
      role="switch"
      aria-checked={on}
      disabled={disabled}
      onClick={(e) => {
        e.stopPropagation();
        onChange(!on);
      }}
    >
      <span className="model-toggle-knob" />
    </button>
  );
}

function ModelSubmenu({
  item,
  onPatch,
}: {
  item: ModelItem;
  onPatch: (patch: {
    thinking_enabled?: boolean;
    reasoning_effort?: string | null;
    max_context_tokens?: number;
  }) => void;
}) {
  const { t } = useTranslation();
  const { caps, entry, contextTokens } = item;
  const showThinking = caps.supports_thinking;
  const showFast = caps.supports_fast;
  const showContext = caps.context_presets.length > 0;
  const showEffort = caps.effort_levels.length > 0;
  const fastOn = entry.reasoning_effort === 'low' && showFast;

  const effortLabel = (level: string) =>
    t(`chat.modelSelector.effortLevels.${level}`, {
      defaultValue: level,
    });

  if (!showThinking && !showFast && !showContext && !showEffort) {
    return (
      <div className="model-submenu">
        <div className="model-submenu-empty">{t('chat.modelSelector.noOptions')}</div>
      </div>
    );
  }

  return (
    <div className="model-submenu" onClick={(e) => e.stopPropagation()}>
      {(showThinking || showFast) && (
        <div className="model-submenu-section">
          <div className="model-submenu-section-title">{t('chat.modelSelector.options')}</div>
          {showThinking && (
            <div className="model-submenu-row">
              <span>{t('chat.modelSelector.thinking')}</span>
              <Toggle
                on={!!entry.thinking_enabled}
                onChange={(v) => onPatch({ thinking_enabled: v })}
              />
            </div>
          )}
          {showFast && (
            <div className="model-submenu-row">
              <span>{t('chat.modelSelector.fast')}</span>
              <Toggle
                on={fastOn}
                onChange={(v) =>
                  onPatch({
                    reasoning_effort: v ? 'low' : 'medium',
                    thinking_enabled: entry.thinking_enabled,
                  })
                }
              />
            </div>
          )}
        </div>
      )}

      {showContext && (
        <div className="model-submenu-section">
          <div className="model-submenu-section-title">{t('chat.modelSelector.context')}</div>
          {caps.context_presets.map((preset) => (
            <button
              key={preset}
              type="button"
              className={`model-submenu-choice ${contextTokens === preset ? 'selected' : ''}`}
              onClick={() => onPatch({ max_context_tokens: preset })}
            >
              <span>{formatContextLabel(preset)}</span>
              {contextTokens === preset && <CheckIcon size={14} />}
            </button>
          ))}
        </div>
      )}

      {showEffort && (
        <div className="model-submenu-section">
          <div className="model-submenu-section-title">{t('chat.modelSelector.effort')}</div>
          {caps.effort_levels.map((level) => {
            const selected = (entry.reasoning_effort || '').toLowerCase() === level;
            return (
              <button
                key={level}
                type="button"
                className={`model-submenu-choice ${selected ? 'selected' : ''}`}
                onClick={() =>
                  onPatch({
                    reasoning_effort: level,
                    thinking_enabled: true,
                  })
                }
              >
                <span>{effortLabel(level)}</span>
                {selected && <CheckIcon size={14} />}
              </button>
            );
          })}
        </div>
      )}
    </div>
  );
}

export function ModelSelector({
  currentModel,
}: {
  currentModel: string;
}) {
  const { t } = useTranslation();
  const dispatch = useAppDispatch();
  const configFromStore = useSelector((state: RootState) => state.settings.config);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const [hoveredKey, setHoveredKey] = useState<string | null>(null);
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const saving = useSelector((state: RootState) => state.settings.saving);

  const effortLabel = useCallback(
    (level: string) =>
      t(`chat.modelSelector.effortLevels.${level}`, {
        defaultValue: level,
      }),
    [t]
  );

  useEffect(() => {
    if (!open) return;
    const handler = (e: MouseEvent) => {
      if (dropdownRef.current && !dropdownRef.current.contains(e.target as Node)) {
        setOpen(false);
        setSearch('');
        setHoveredKey(null);
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

  useEffect(() => {
    return () => {
      if (hoverTimer.current) clearTimeout(hoverTimer.current);
    };
  }, []);

  const groupedModels = useMemo(() => {
    if (!configFromStore) return new Map<string, ModelItem[]>();
    return buildItems(configFromStore);
  }, [configFromStore]);

  const filteredGroups = useMemo(() => {
    if (!search.trim()) return groupedModels;
    const q = search.toLowerCase();
    const result = new Map<string, ModelItem[]>();
    groupedModels.forEach((models, provider) => {
      const matched = models.filter(
        (m) =>
          m.displayName.toLowerCase().includes(q) ||
          m.modelId.toLowerCase().includes(q)
      );
      if (matched.length > 0) result.set(provider, matched);
    });
    return result;
  }, [groupedModels, search]);

  const flatItems = useMemo(() => {
    const all: ModelItem[] = [];
    groupedModels.forEach((models) => all.push(...models));
    return all;
  }, [groupedModels]);

  const currentKey = currentModel || configFromStore?.default_model || '';
  const currentItem = flatItems.find((m) => m.key === currentKey) ?? null;
  const currentDisplay = modelTriggerLabel(
    currentItem,
    currentKey.includes('/')
      ? currentKey.slice(currentKey.lastIndexOf('/') + 1)
      : currentKey || t('chat.modelSelector.selectModel')
  );

  const hoveredItem = hoveredKey
    ? flatItems.find((m) => m.key === hoveredKey) ?? null
    : null;

  const handleSelect = useCallback(
    async (key: string) => {
      setOpen(false);
      setSearch('');
      setHoveredKey(null);
      if (configFromStore) {
        dispatch(switchModel({ modelKey: key, currentConfig: configFromStore }));
      }
    },
    [dispatch, configFromStore]
  );

  const handleHoverEnter = useCallback((key: string) => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    hoverTimer.current = setTimeout(() => setHoveredKey(key), 120);
  }, []);

  const handleHoverLeaveList = useCallback(() => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    hoverTimer.current = setTimeout(() => setHoveredKey(null), 200);
  }, []);

  const handleSubmenuEnter = useCallback(() => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
  }, []);

  const handlePatch = useCallback(
    (
      modelKey: string,
      patch: {
        thinking_enabled?: boolean;
        reasoning_effort?: string | null;
        max_context_tokens?: number;
      }
    ) => {
      if (!configFromStore) return;
      const parsed = parseModelKey(modelKey);
      if (!parsed) return;
      dispatch(
        updateModelSettings({
          modelKey,
          patch,
          currentConfig: configFromStore,
          alsoSwitch: modelKey !== configFromStore.default_model,
        })
      );
    },
    [dispatch, configFromStore]
  );

  return (
    <div className="model-selector-wrapper" ref={dropdownRef}>
      <button
        type="button"
        className="model-selector"
        onClick={() => {
          setOpen(!open);
          if (open) setHoveredKey(null);
        }}
      >
        <span className="model-selector-text">
          <span className="model-selector-name">{currentDisplay}</span>
        </span>
        <ChevronDownIcon size={12} className={`model-selector-chevron ${open ? 'open' : ''}`} />
      </button>

      {open && (
        <div className="model-dropdown-shell">
          <div className="model-dropdown">
            <div className="model-dropdown-search">
              <SearchIcon size={14} />
              <input
                ref={searchInputRef}
                className="model-dropdown-search-input"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t('chat.modelSelector.searchPlaceholder')}
              />
            </div>

            <div className="model-dropdown-list" onMouseLeave={handleHoverLeaveList}>
              {filteredGroups.size === 0 && (
                <div className="model-dropdown-empty">{t('chat.modelSelector.noModels')}</div>
              )}

              {Array.from(filteredGroups.entries()).map(([provider, models]) => (
                <div key={provider} className="model-dropdown-group">
                  <div className="model-dropdown-group-header">
                    <ServerIcon size={12} />
                    <span>{provider}</span>
                  </div>
                  {models.map((model) => {
                    const hasOptions =
                      model.caps.supports_thinking ||
                      model.caps.supports_fast ||
                      model.caps.context_presets.length > 1 ||
                      model.caps.effort_levels.length > 0;
                    return (
                      <button
                        key={model.key}
                        type="button"
                        className={`model-dropdown-item ${model.key === currentKey ? 'selected' : ''} ${hoveredKey === model.key ? 'hovered' : ''}`}
                        onClick={() => handleSelect(model.key)}
                        onMouseEnter={() => handleHoverEnter(model.key)}
                      >
                        <span className="model-dropdown-item-main">
                          <span className="model-dropdown-item-key">{model.displayName}</span>
                          <span className="model-dropdown-item-badges">
                            <span className="model-badge">
                              {formatContextLabel(model.contextTokens)}
                            </span>
                            {model.entry.reasoning_effort && (
                              <span className="model-badge">
                                {effortLabel(model.entry.reasoning_effort)}
                              </span>
                            )}
                            {model.entry.thinking_enabled && (
                              <span className="model-badge model-badge-think">
                                {t('chat.modelSelector.thinkBadge')}
                              </span>
                            )}
                          </span>
                        </span>
                        <span className="model-dropdown-item-trailing">
                          {model.key === currentKey && (
                            <CheckIcon size={14} className="model-dropdown-item-check" />
                          )}
                          {hasOptions && (
                            <ChevronRightIcon size={14} className="model-dropdown-item-chevron" />
                          )}
                        </span>
                      </button>
                    );
                  })}
                </div>
              ))}
            </div>
          </div>

          {hoveredItem && (
            <div
              className="model-submenu-flyout"
              onMouseEnter={handleSubmenuEnter}
              onMouseLeave={handleHoverLeaveList}
            >
              <ModelSubmenu
                item={hoveredItem}
                onPatch={(patch) => handlePatch(hoveredItem.key, patch)}
              />
            </div>
          )}
        </div>
      )}

      {saving && <span className="model-save-indicator">{t('chat.modelSelector.saving')}</span>}
    </div>
  );
}
