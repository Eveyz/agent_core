import { useState, useCallback, useRef, useEffect, useMemo } from 'react';
import { useSelector } from 'react-redux';
import { RootState } from '../../store';
import { switchModel } from '../../features/settings/settingsSlice';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import ChevronDownIcon from 'lucide-react/dist/esm/icons/chevron-down.mjs';
import SearchIcon from 'lucide-react/dist/esm/icons/search.mjs';
import StarIcon from 'lucide-react/dist/esm/icons/star.mjs';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';

interface ModelItem {
  key: string;
  displayName: string;
  providerName: string;
}

export function ModelSelector({
  currentModel,
  onModelChange,
}: {
  currentModel: string;
  onModelChange?: (key: string) => void;
}) {
  const dispatch = useAppDispatch();
  const configFromStore = useSelector((state: RootState) => state.settings.config);
  const [open, setOpen] = useState(false);
  const [search, setSearch] = useState('');
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const saving = useSelector((state: RootState) => state.settings.saving);

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
    const config = configFromStore;
    if (!config) return new Map<string, ModelItem[]>();
    const providers = config.providers;
    const map = new Map<string, ModelItem[]>();
    Object.entries(providers).forEach(([providerKey, provider]) => {
      const items: ModelItem[] = [];
      Object.entries(provider.models).forEach(([modelKey]) => {
        items.push({
          key: `${providerKey}/${modelKey}`,
          displayName: modelKey,
          providerName: provider.name || providerKey,
        });
      });
      if (items.length > 0) {
        map.set(provider.name || providerKey, items);
      }
    });
    return map;
  }, [configFromStore]);

  const filteredGroups = useMemo(() => {
    if (!search.trim()) return groupedModels;
    const q = search.toLowerCase();
    const result = new Map<string, ModelItem[]>();
    groupedModels.forEach((models, provider) => {
      const matched = models.filter((m) => m.displayName.toLowerCase().includes(q));
      if (matched.length > 0) result.set(provider, matched);
    });
    return result;
  }, [groupedModels, search]);

  const handleSelect = useCallback(
    async (key: string) => {
      setOpen(false);
      setSearch('');
      onModelChange?.(key);
      if (configFromStore) {
        dispatch(switchModel({ modelKey: key, currentConfig: configFromStore }));
      }
    },
    [dispatch, configFromStore, onModelChange]
  );

  const config = configFromStore;

  const currentKey = currentModel || config?.default_model || '';
  const currentDisplay = currentKey.includes('/')
    ? currentKey.slice(currentKey.lastIndexOf('/') + 1)
    : currentKey || 'Select model';

  return (
    <div className="model-selector-wrapper" ref={dropdownRef}>
      <button className="model-selector" onClick={() => setOpen(!open)}>
        <span className="model-selector-text">
          <span className="model-selector-name">{currentDisplay}</span>
        </span>
        <ChevronDownIcon size={12} className={`model-selector-chevron ${open ? 'open' : ''}`} />
      </button>

      {open && (
        <div className="model-dropdown">
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
            {filteredGroups.size === 0 && <div className="model-dropdown-empty">No models found</div>}

            {Array.from(filteredGroups.entries()).map(([provider, models]) => (
              <div key={provider} className="model-dropdown-group">
                <div className="model-dropdown-group-header">
                  <ServerIcon size={12} />
                  <span>{provider}</span>
                </div>
                {models.map((model) => (
                  <button
                    key={model.key}
                    className={`model-dropdown-item ${model.key === currentKey ? 'selected' : ''}`}
                    onClick={() => handleSelect(model.key)}
                  >
                    <span className="model-dropdown-item-key">{model.displayName}</span>
                    {model.key === currentKey && <StarIcon size={12} className="model-dropdown-item-star" />}
                  </button>
                ))}
              </div>
            ))}
          </div>
        </div>
      )}

      {saving && <span className="model-save-indicator">Saving...</span>}
    </div>
  );
}
