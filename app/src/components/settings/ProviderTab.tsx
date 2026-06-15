import { useState, useEffect, useCallback } from 'react';
import { useDispatch, useSelector } from 'react-redux';
import { RootState } from '../../store';
import { upsertProvider, deleteProvider, setDefaultModel } from '../../features/settings/settingsSlice';
import type { ProviderModelEntry } from '../../features/settings/settingsSlice';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import StarIcon from 'lucide-react/dist/esm/icons/star.mjs';
import SaveIcon from 'lucide-react/dist/esm/icons/save.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';

interface ModelRow {
  key: string;
  model_id: string;
}

interface ProviderFormData {
  provider_key: string;
  provider_name: string;
  base_url: string;
  api_key: string;
  max_context_tokens: number;
  models: ModelRow[];
}

const EMPTY_FORM: ProviderFormData = {
  provider_key: '',
  provider_name: '',
  base_url: '',
  api_key: '',
  max_context_tokens: 32768,
  models: [{ key: '', model_id: '' }],
};

function modelRowToEntry(row: ModelRow): ProviderModelEntry {
  return {
    model_id: row.model_id,
  };
}

function ProviderForm({
  editKey,
  onCancel,
}: {
  editKey: string | null;
  onCancel: () => void;
}) {
  const dispatch = useDispatch();
  const config = useSelector((state: RootState) => state.settings.config);
  const [form, setForm] = useState<ProviderFormData>(EMPTY_FORM);
  const [errors, setErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    if (editKey && config?.providers[editKey]) {
      const provider = config.providers[editKey];
      setForm({
        provider_key: editKey,
        provider_name: provider.name ?? '',
        base_url: provider.base_url,
        api_key: provider.api_key,
        max_context_tokens: provider.max_context_tokens ?? 32768,
        models: Object.entries(provider.models).map(([k, m]) => ({ key: k, model_id: m.model_id })),
      });
    } else {
      setForm(EMPTY_FORM);
    }
  }, [editKey, config]);

  const updateField = useCallback(<K extends keyof ProviderFormData>(field: K, value: ProviderFormData[K]) => {
    setForm((prev) => ({ ...prev, [field]: value }));
    if (errors[field]) {
      setErrors((prev) => {
        const next = { ...prev };
        delete next[field];
        return next;
      });
    }
  }, [errors]);

  const updateModelRow = (index: number, patch: Partial<ModelRow>) => {
    setForm((prev) => ({
      ...prev,
      models: prev.models.map((row, i) => (i === index ? { ...row, ...patch } : row)),
    }));
    if (errors[`model_${index}`]) {
      setErrors((prev) => {
        const next = { ...prev };
        delete next[`model_${index}`];
        return next;
      });
    }
  };

  const addModelRow = () => {
    setForm((prev) => ({
      ...prev,
      models: [...prev.models, { key: '', model_id: '' }],
    }));
  };

  const removeModelRow = (index: number) => {
    setForm((prev) => ({
      ...prev,
      models: prev.models.filter((_, i) => i !== index),
    }));
  };

  const validate = (): boolean => {
    const next: Record<string, string> = {};
    if (!form.provider_key.trim()) next.provider_key = 'Provider key is required';
    if (!form.base_url.trim()) next.base_url = 'Base URL is required';

    if (config) {
      const isNewProvider = !editKey || editKey !== form.provider_key.trim();
      if (isNewProvider && config.providers[form.provider_key.trim()]) {
        next.provider_key = 'A provider with this key already exists';
      }
    }

    form.models.forEach((row, i) => {
      if (!row.key.trim()) next[`model_${i}`] = 'Model key is required';
      if (!row.model_id.trim()) next[`model_${i}_id`] = 'Model ID is required';
    });

    const keys = form.models.map((r) => r.key.trim()).filter(Boolean);
    const dupes = keys.filter((item, idx) => keys.indexOf(item) !== idx);
    if (dupes.length > 0) {
      form.models.forEach((row, i) => {
        if (dupes.includes(row.key.trim())) {
          next[`model_${i}`] = 'Duplicate model key in this provider';
        }
      });
    }

    setErrors(next);
    return Object.keys(next).length === 0;
  };

  const handleSave = () => {
    if (!validate()) return;

    if (editKey && config) {
      dispatch(deleteProvider(editKey));
    }

    const models: Record<string, ProviderModelEntry> = {};
    form.models.forEach((row) => {
      if (row.key.trim()) {
        models[row.key.trim()] = modelRowToEntry(row);
      }
    });

    dispatch(upsertProvider({
      key: form.provider_key.trim(),
      provider: {
        name: form.provider_name,
        base_url: form.base_url,
        api_key: form.api_key,
        max_context_tokens: form.max_context_tokens,
        temperature: undefined,
        max_tokens: undefined,
        react_enabled: true,
        system_prompt: undefined,
        max_iterations: 100,
        request_timeout_secs: 60,
        models,
      },
    }));

    onCancel();
  };

  const inputClass = (field: string) => `settings-input ${errors[field] ? 'settings-input-error' : ''}`;

  return (
    <div className="model-form">
      <h4 className="model-form-title">{editKey ? 'Edit Provider' : 'New Provider'}</h4>

      <div className="model-form-grid">
        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Provider Key <span className="required">*</span></label>
          <input
            className={inputClass('provider_key')}
            value={form.provider_key}
            onChange={(e) => updateField('provider_key', e.target.value)}
            placeholder="e.g. openai, deepseek"
            disabled={!!editKey}
          />
          {errors.provider_key && <span className="field-error">{errors.provider_key}</span>}
        </div>

        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Provider Name</label>
          <input
            className="settings-input"
            value={form.provider_name}
            onChange={(e) => updateField('provider_name', e.target.value)}
            placeholder="e.g. OpenAI, DeepSeek, Ollama"
          />
        </div>

        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Base URL <span className="required">*</span></label>
          <input
            className={inputClass('base_url')}
            value={form.base_url}
            onChange={(e) => updateField('base_url', e.target.value)}
            placeholder="https://api.openai.com/v1"
          />
          {errors.base_url && <span className="field-error">{errors.base_url}</span>}
        </div>

        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">API Key</label>
          <input
            className="settings-input"
            type="password"
            value={form.api_key}
            onChange={(e) => updateField('api_key', e.target.value)}
            placeholder="sk-... or ${ENV_VAR}"
          />
        </div>

        <div className="model-form-field">
          <label className="model-form-label">Max Context Tokens</label>
          <input
            className="settings-input"
            type="number"
            value={form.max_context_tokens}
            onChange={(e) => updateField('max_context_tokens', Number(e.target.value))}
          />
        </div>
      </div>

      {/* Models Section */}
      <div className="model-section-divider" />
      <div className="models-section">
        <div className="models-section-header">
          <h5 className="models-section-title">Models</h5>
        </div>

        <div className="models-table">
          <div className="models-table-header">
            <span>Model Key <span className="required">*</span></span>
            <span>Model ID <span className="required">*</span></span>
            <span style={{ width: '32px' }} />
          </div>
          {form.models.map((row, index) => (
            <div key={index} className="models-table-row">
              <input
                className={inputClass(`model_${index}`)}
                value={row.key}
                onChange={(e) => updateModelRow(index, { key: e.target.value })}
                placeholder="e.g. gpt4o"
                disabled={!!editKey && !!config?.providers[editKey]?.models[row.key]}
              />
              <input
                className={inputClass(`model_${index}_id`)}
                value={row.model_id}
                onChange={(e) => updateModelRow(index, { model_id: e.target.value })}
                placeholder="e.g. gpt-4o"
              />
              <button
                className="model-row-delete"
                title="Remove model"
                onClick={() => removeModelRow(index)}
                disabled={form.models.length <= 1}
              >
                <TrashIcon size={14} />
              </button>
              {(errors[`model_${index}`] || errors[`model_${index}_id`]) && (
                <span className="field-error models-row-error">
                  {errors[`model_${index}`] || errors[`model_${index}_id`]}
                </span>
              )}
            </div>
          ))}
        </div>

        <button className="btn-add-model" onClick={addModelRow}>
          <PlusIcon size={14} /> Add Model
        </button>
      </div>

      <div className="model-form-actions">
        <button className="btn-secondary" onClick={onCancel}>
          <XIcon size={14} /> Cancel
        </button>
        <button className="btn-primary" onClick={handleSave}>
          <SaveIcon size={14} /> Save
        </button>
      </div>
    </div>
  );
}

export default function ProviderTab() {
  const dispatch = useDispatch();
  const config = useSelector((state: RootState) => state.settings.config);
  const [editingKey, setEditingKey] = useState<string | null>(null);
  const [isCreating, setIsCreating] = useState(false);

  const handleDeleteProvider = (key: string) => {
    if (!config) return;
    const provider = config.providers[key];
    if (!provider) return;
    const names = Object.keys(provider.models).map((k) => `"${k}"`).join(', ');
    if (!confirm(`Delete provider "${key}" and all its models (${names})?`)) return;
    dispatch(deleteProvider(key));
  };

  const handleSetDefault = (providerKey: string, modelKey: string) => {
    dispatch(setDefaultModel(`${providerKey}/${modelKey}`));
  };

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const defaultModel = config.default_model;

  if (isCreating || editingKey !== null) {
    return (
      <div className="settings-tab-content">
        <ProviderForm editKey={isCreating ? null : editingKey} onCancel={() => { setIsCreating(false); setEditingKey(null); }} />
      </div>
    );
  }

  return (
    <div className="settings-tab-content">
      <div className="settings-section-header">
        <h3 className="settings-section-title">
          <ServerIcon size={14} /> Model Providers ({Object.keys(config.providers).length})
        </h3>
        <div className="settings-section-actions">
          <button className="btn-primary" onClick={() => setIsCreating(true)}>
            <PlusIcon size={14} /> Add Provider
          </button>
        </div>
      </div>

      {Object.keys(config.providers).length === 0 && (
        <div className="settings-empty">No providers configured. Click &quot;Add Provider&quot; to create one.</div>
      )}

      <div className="model-cards">
        {Object.entries(config.providers).map(([providerKey, provider]) => {
          const modelEntries = Object.entries(provider.models);
          const hasDefault = modelEntries.some(([k]) => `${providerKey}/${k}` === defaultModel);
          const defaultModelKey = modelEntries.find(([k]) => `${providerKey}/${k}` === defaultModel)?.[0] || modelEntries[0]?.[0];

          return (
            <div key={providerKey} className={`model-card ${hasDefault ? 'model-card-default' : ''}`}>
              <div className="model-card-header">
                <div className="model-card-title">
                  <span className="model-name">{provider.name || providerKey}</span>
                  <span className="model-count">{modelEntries.length} model{modelEntries.length > 1 ? 's' : ''}</span>
                  {hasDefault && (
                    <span className="model-default-badge">
                      <StarIcon size={10} /> default: {defaultModelKey}
                    </span>
                  )}
                </div>
                <div className="model-card-actions">
                  {!hasDefault && modelEntries.length > 0 && (
                    <button
                      className="model-action-btn"
                      title="Set as default"
                      onClick={() => handleSetDefault(providerKey, modelEntries[0][0])}
                    >
                      <StarIcon size={12} />
                    </button>
                  )}
                  <button
                    className="model-action-btn"
                    title="Edit"
                    onClick={() => setEditingKey(providerKey)}
                  >
                    <PencilIcon size={12} />
                  </button>
                  <button
                    className="model-action-btn model-action-danger"
                    title="Delete provider"
                    onClick={() => handleDeleteProvider(providerKey)}
                  >
                    <TrashIcon size={12} />
                  </button>
                </div>
              </div>

              <div className="model-card-body">
                <div className="model-field">
                  <span className="model-field-label">Base URL</span>
                  <span className="model-field-value">{provider.base_url}</span>
                </div>
                <div className="model-field">
                  <span className="model-field-label">API Key</span>
                  <span className="model-field-value model-field-masked">
                    {provider.api_key ? '••••••••••••••••' : '—'}
                  </span>
                </div>
                <div className="model-field">
                  <span className="model-field-label">Context</span>
                  <span className="model-field-value">{(provider.max_context_tokens ?? 0).toLocaleString()}</span>
                </div>
              </div>

              <div className="provider-models-list">
                {modelEntries.map(([modelKey, model]) => (
                  <div key={modelKey} className={`provider-model-item ${`${providerKey}/${modelKey}` === defaultModel ? 'provider-model-default' : ''}`}>
                    <span className="provider-model-key">{modelKey}</span>
                    <span className="provider-model-id">{model.model_id}</span>
                    {`${providerKey}/${modelKey}` === defaultModel && <StarIcon size={10} className="provider-model-star" />}
                  </div>
                ))}
              </div>
            </div>
          );
        })}
      </div>
    </div>
  );
}
