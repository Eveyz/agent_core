import { useState, useEffect } from 'react';
import { useSelector } from 'react-redux';
import { useAppDispatch } from '../../hooks/useAppDispatch';
import { RootState } from '../../store';
import { upsertMcpServer, deleteMcpServer, toggleMcpServer } from '../../features/settings/settingsSlice';
import type { McpServerConfig } from '../../features/settings/settingsSlice';
import { useConfirmDialog } from '../ui/DialogManager';
import ServerIcon from 'lucide-react/dist/esm/icons/server.mjs';
import TerminalIcon from 'lucide-react/dist/esm/icons/terminal.mjs';
import LinkIcon from 'lucide-react/dist/esm/icons/link.mjs';
import SettingsIcon from 'lucide-react/dist/esm/icons/settings.mjs';
import CheckCircleIcon from 'lucide-react/dist/esm/icons/check-circle.mjs';
import XCircleIcon from 'lucide-react/dist/esm/icons/x-circle.mjs';
import PlusIcon from 'lucide-react/dist/esm/icons/plus.mjs';
import PencilIcon from 'lucide-react/dist/esm/icons/pencil.mjs';
import TrashIcon from 'lucide-react/dist/esm/icons/trash.mjs';
import XIcon from 'lucide-react/dist/esm/icons/x.mjs';

interface EnvRow {
  key: string;
  value: string;
}

interface McpFormData {
  name: string;
  transport: 'stdio' | 'sse' | 'http';
  command: string;
  argsText: string;
  url: string;
  envRows: EnvRow[];
  enabled: boolean;
}

const EMPTY_FORM: McpFormData = {
  name: '',
  transport: 'stdio',
  command: '',
  argsText: '',
  url: '',
  envRows: [],
  enabled: true,
};

// Common, canonical MCP servers offered as one-click starting templates.
// These only prefill the form — the user can still edit before saving.
const PRESETS: { label: string; build: () => McpFormData }[] = [
  {
    label: 'Filesystem',
    build: () => ({
      ...EMPTY_FORM,
      name: 'filesystem',
      command: 'npx',
      argsText: '-y @modelcontextprotocol/server-filesystem /path/to/folder',
    }),
  },
  {
    label: 'Fetch (web)',
    build: () => ({ ...EMPTY_FORM, name: 'fetch', command: 'uvx', argsText: 'mcp-server-fetch' }),
  },
  {
    label: 'GitHub',
    build: () => ({
      ...EMPTY_FORM,
      name: 'github',
      command: 'npx',
      argsText: '-y @modelcontextprotocol/server-github',
      envRows: [{ key: 'GITHUB_PERSONAL_ACCESS_TOKEN', value: '' }],
    }),
  },
  {
    label: 'Parallel Search',
    build: () => ({
      ...EMPTY_FORM,
      name: 'parallel-search',
      transport: 'http',
      url: 'https://search.parallel.ai/mcp',
    }),
  },
  {
    label: 'Custom URL',
    build: () => ({ ...EMPTY_FORM, name: '', transport: 'http', url: '' }),
  },
];

function formToServer(form: McpFormData): McpServerConfig {
  const args = form.argsText
    .split('\n')
    .map((s) => s.trim())
    .filter(Boolean);
  const env: Record<string, string> = {};
  form.envRows.forEach((r) => {
    if (r.key.trim()) env[r.key.trim()] = r.value;
  });

  const server: McpServerConfig = {
    name: form.name.trim(),
    transport: form.transport,
    enabled: form.enabled,
  };
  if (form.transport === 'stdio') {
    server.command = form.command;
    if (args.length) server.args = args;
  } else {
    server.url = form.url;
  }
  if (Object.keys(env).length) server.env = env;
  return server;
}

function McpServerForm({
  editName,
  initial,
  onCancel,
}: {
  editName: string | null;
  initial?: McpFormData;
  onCancel: () => void;
}) {
  const dispatch = useAppDispatch();
  const config = useSelector((state: RootState) => state.settings.config);
  const [form, setForm] = useState<McpFormData>(EMPTY_FORM);
  const [errors, setErrors] = useState<Record<string, string>>({});

  useEffect(() => {
    if (editName && config?.mcp.servers) {
      const s = config.mcp.servers.find((x) => x.name === editName);
      if (s) {
        setForm({
          name: s.name,
          transport: (s.transport as 'stdio' | 'sse' | 'http') || 'stdio',
          command: s.command || '',
          argsText: (s.args || []).join('\n'),
          url: s.url || '',
          envRows: s.env ? Object.entries(s.env).map(([k, v]) => ({ key: k, value: v })) : [],
          enabled: s.enabled !== false,
        });
        return;
      }
    }
    if (initial) {
      setForm(initial);
      return;
    }
    setForm(EMPTY_FORM);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [editName, initial, config]);

  const updateField = <K extends keyof McpFormData>(field: K, value: McpFormData[K]) => {
    setForm((prev) => ({ ...prev, [field]: value }));
    if (errors[field]) {
      setErrors((prev) => {
        const next = { ...prev };
        delete next[field];
        return next;
      });
    }
  };

  const updateEnvRow = (index: number, patch: Partial<EnvRow>) => {
    setForm((prev) => ({
      ...prev,
      envRows: prev.envRows.map((r, i) => (i === index ? { ...r, ...patch } : r)),
    }));
  };

  const addEnvRow = () => setForm((prev) => ({ ...prev, envRows: [...prev.envRows, { key: '', value: '' }] }));
  const removeEnvRow = (index: number) =>
    setForm((prev) => ({ ...prev, envRows: prev.envRows.filter((_, i) => i !== index) }));

  const validate = (): boolean => {
    const next: Record<string, string> = {};
    if (!form.name.trim()) next.name = 'Server name is required';
    if (form.transport === 'stdio') {
      if (!form.command.trim()) next.command = 'Command is required for stdio servers';
    } else {
      if (!form.url.trim()) next.url = 'URL is required for SSE servers';
    }
    setErrors(next);
    return Object.keys(next).length === 0;
  };

  const handleSave = () => {
    if (!validate()) return;
    const server = formToServer(form);
    dispatch(upsertMcpServer({ oldName: editName ?? undefined, server }));
    onCancel();
  };

  const inputClass = (field: string) => `settings-input ${errors[field] ? 'settings-input-error' : ''}`;

  return (
    <div className="model-form">
      <h4 className="model-form-title">{editName ? 'Edit MCP Server' : 'New MCP Server'}</h4>

      <div className="model-form-grid">
        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Server Name <span className="required">*</span></label>
          <input
            className={inputClass('name')}
            value={form.name}
            onChange={(e) => updateField('name', e.target.value)}
            placeholder="e.g. filesystem, github"
            disabled={!!editName}
          />
          {errors.name && <span className="field-error">{errors.name}</span>}
        </div>

        <div className="model-form-field">
          <label className="model-form-label">Transport</label>
          <select
            className="settings-input"
            value={form.transport}
            onChange={(e) => updateField('transport', e.target.value as 'stdio' | 'sse' | 'http')}
          >
            <option value="stdio">Stdio (local command)</option>
            <option value="sse">SSE (legacy HTTP)</option>
            <option value="http">Streamable HTTP</option>
          </select>
        </div>

        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Enabled</label>
          <label className="mcp-switch-label">
            <button
              type="button"
              className={`mcp-switch ${form.enabled ? 'on' : ''}`}
              onClick={() => updateField('enabled', !form.enabled)}
              aria-pressed={form.enabled}
            >
              <span className="mcp-switch-knob" />
            </button>
            <span>{form.enabled ? 'Enabled' : 'Disabled'}</span>
          </label>
        </div>

        {form.transport === 'stdio' ? (
          <>
            <div className="model-form-field model-form-field-full">
              <label className="model-form-label">Command <span className="required">*</span></label>
              <input
                className={inputClass('command')}
                value={form.command}
                onChange={(e) => updateField('command', e.target.value)}
                placeholder="e.g. npx, uvx, node"
              />
              {errors.command && <span className="field-error">{errors.command}</span>}
            </div>

            <div className="model-form-field model-form-field-full">
              <label className="model-form-label">Args (one per line)</label>
              <textarea
                className="settings-input mcp-args-input"
                value={form.argsText}
                onChange={(e) => updateField('argsText', e.target.value)}
                placeholder={'-y @modelcontextprotocol/server-filesystem\n/path/to/folder'}
                rows={3}
              />
            </div>
          </>
        ) : (
          <div className="model-form-field model-form-field-full">
            <label className="model-form-label">URL <span className="required">*</span></label>
            <input
              className={inputClass('url')}
              value={form.url}
              onChange={(e) => updateField('url', e.target.value)}
              placeholder="https://example.com/mcp/sse"
            />
            {errors.url && <span className="field-error">{errors.url}</span>}
          </div>
        )}

        <div className="model-form-field model-form-field-full">
          <label className="model-form-label">Environment Variables</label>
          <div className="mcp-env-editor">
            {form.envRows.map((row, index) => (
              <div key={index} className="mcp-env-row">
                <input
                  className="settings-input"
                  value={row.key}
                  onChange={(e) => updateEnvRow(index, { key: e.target.value })}
                  placeholder="KEY"
                />
                <span className="mcp-env-eq">=</span>
                <input
                  className="settings-input"
                  value={row.value}
                  onChange={(e) => updateEnvRow(index, { value: e.target.value })}
                  placeholder="value"
                />
                <button
                  type="button"
                  className="model-row-delete"
                  title="Remove variable"
                  onClick={() => removeEnvRow(index)}
                >
                  <TrashIcon size={14} />
                </button>
              </div>
            ))}
            <button type="button" className="btn-add-model" onClick={addEnvRow}>
              <PlusIcon size={14} /> Add Variable
            </button>
          </div>
        </div>
      </div>

      <div className="model-form-actions">
        <button className="btn-secondary" onClick={onCancel}>
          <XIcon size={14} /> Cancel
        </button>
        <button className="btn-primary" onClick={handleSave}>
          <CheckCircleIcon size={14} /> Save
        </button>
      </div>
    </div>
  );
}

export default function McpTab() {
  const dispatch = useAppDispatch();
  const config = useSelector((state: RootState) => state.settings.config);
  const [creating, setCreating] = useState(false);
  const [editingName, setEditingName] = useState<string | null>(null);
  const [presetForm, setPresetForm] = useState<McpFormData | undefined>(undefined);
  const { confirm, dialogElement } = useConfirmDialog();

  if (!config) {
    return (
      <div className="settings-tab-content">
        <div className="settings-empty">No configuration loaded.</div>
      </div>
    );
  }

  const servers = config.mcp.servers;

  if (creating || editingName !== null) {
    return (
      <div className="settings-tab-content">
        <McpServerForm
          editName={creating ? null : editingName}
          initial={creating ? presetForm : undefined}
          onCancel={() => {
            setCreating(false);
            setEditingName(null);
            setPresetForm(undefined);
          }}
        />
      </div>
    );
  }

  const handleDelete = async (name: string) => {
    const ok = await confirm({
      title: 'Delete MCP Server',
      message: `Remove MCP server "${name}"? Its tools will no longer be available to the agent.`,
      confirmLabel: 'Delete',
      cancelLabel: 'Cancel',
      danger: true,
    });
    if (ok) dispatch(deleteMcpServer(name));
  };

  const handleToggle = (name: string) => dispatch(toggleMcpServer(name));

  const handlePreset = (build: () => McpFormData) => {
    setPresetForm(build());
    setCreating(true);
  };

  return (
    <div className="settings-tab-content">
      <div className="settings-section-header">
        <h3 className="settings-section-title">
          <ServerIcon size={14} /> MCP Servers ({servers.length})
        </h3>
        <div className="settings-section-actions">
          <button className="btn-primary" onClick={() => setCreating(true)}>
            <PlusIcon size={14} /> Add Server
          </button>
        </div>
      </div>

      {servers.length === 0 && (
        <div className="settings-empty mcp-empty">
          <p className="mcp-empty-text">
            MCP is ready. Add a server below to expose its tools to the agent.
          </p>
          <div className="mcp-presets">
            <span className="mcp-presets-label">Quick start:</span>
            {PRESETS.map((p) => (
              <button key={p.label} className="mcp-preset-chip" onClick={() => handlePreset(p.build)}>
                + {p.label}
              </button>
            ))}
          </div>
        </div>
      )}

      <div className="mcp-server-list">
        {servers.map((server, idx) => {
          const enabled = server.enabled !== false;
          return (
            <div key={server.name || idx} className="mcp-server-card">
              <div className="mcp-server-header">
                <span className="mcp-server-name">{server.name}</span>
                <div className="mcp-server-controls">
                  <button
                    type="button"
                    className={`mcp-switch ${enabled ? 'on' : ''}`}
                    onClick={() => handleToggle(server.name)}
                    aria-pressed={enabled}
                    title={enabled ? 'Disable server' : 'Enable server'}
                  >
                    <span className="mcp-switch-knob" />
                  </button>
                  <span className={`mcp-server-status ${enabled ? 'status-enabled' : 'status-disabled'}`}>
                    {enabled ? (
                      <>
                        <CheckCircleIcon size={12} /> Enabled
                      </>
                    ) : (
                      <>
                        <XCircleIcon size={12} /> Disabled
                      </>
                    )}
                  </span>
                  <button
                    className="model-action-btn"
                    title="Edit"
                    onClick={() => setEditingName(server.name)}
                  >
                    <PencilIcon size={12} />
                  </button>
                  <button
                    className="model-action-btn model-action-danger"
                    title="Delete server"
                    onClick={() => handleDelete(server.name)}
                  >
                    <TrashIcon size={12} />
                  </button>
                </div>
              </div>
              <div className="mcp-server-body">
                {server.transport && (
                  <div className="mcp-field">
                    <LinkIcon size={12} />
                    <span className="mcp-field-label">Transport</span>
                    <span className="mcp-field-value">{server.transport}</span>
                  </div>
                )}
                {server.command && (
                  <div className="mcp-field">
                    <TerminalIcon size={12} />
                    <span className="mcp-field-label">Command</span>
                    <span className="mcp-field-value">{server.command}</span>
                  </div>
                )}
                {server.args && server.args.length > 0 && (
                  <div className="mcp-field">
                    <SettingsIcon size={12} />
                    <span className="mcp-field-label">Args</span>
                    <span className="mcp-field-value">{server.args.join(' ')}</span>
                  </div>
                )}
                {server.url && (
                  <div className="mcp-field">
                    <LinkIcon size={12} />
                    <span className="mcp-field-label">URL</span>
                    <span className="mcp-field-value">{server.url}</span>
                  </div>
                )}
                {server.env && Object.keys(server.env).length > 0 && (
                  <div className="mcp-field mcp-field-full">
                    <span className="mcp-field-label">Environment</span>
                    <div className="mcp-env-list">
                      {Object.entries(server.env).map(([k, v]) => (
                        <div key={k} className="mcp-env-item">
                          <code>{k}</code> = <code className="mcp-env-value">{v}</code>
                        </div>
                      ))}
                    </div>
                  </div>
                )}
              </div>
            </div>
          );
        })}
      </div>
      {dialogElement}
    </div>
  );
}
