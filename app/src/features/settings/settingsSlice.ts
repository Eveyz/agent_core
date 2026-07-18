import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';

export interface ProviderModelEntry {
  model_id: string;
  temperature?: number;
  max_tokens?: number;
  system_prompt?: string;
  max_context_tokens?: number;
  reasoning_effort?: string;
  thinking_enabled?: boolean;
}

export interface ProviderConfig {
  name: string;
  base_url: string;
  api_key: string;
  max_context_tokens: number;
  temperature?: number;
  max_tokens?: number;
  react_enabled: boolean;
  system_prompt?: string;
  max_iterations: number;
  request_timeout_secs: number;
  models: Record<string, ProviderModelEntry>;
}

export interface ReflectionConfig {
  trigger_message_count?: number;
  reflection_model?: string;
}

export interface MemoryConfig {
  db_path: string;
  embedding_model: string;
  max_core_blocks: number;
  default_block_max_chars: number;
  consolidation_enabled: boolean;
  embedding_enabled?: boolean;
  mode?: string;
  reflection?: ReflectionConfig;
}

export interface PermissionRule {
  tool_pattern: string;
  level: string;
}

export interface WhitelistEntry {
  tool_pattern: string;
  commands?: string[];
}

export interface BlacklistEntry {
  tool_pattern: string;
  hosts?: string[];
}

export interface PermissionConfig {
  mode: string;
  auto_allow_up_to?: string;
  rules: PermissionRule[];
  whitelist: WhitelistEntry[];
  blacklist: BlacklistEntry[];
}

export interface McpServerConfig {
  name: string;
  transport?: string;
  command?: string;
  args?: string[];
  env?: Record<string, string>;
  url?: string;
  enabled?: boolean;
}

export interface McpConfig {
  servers: McpServerConfig[];
}

export interface AppConfig {
  default_model: string;
  providers: Record<string, ProviderConfig>;
  memory?: MemoryConfig;
  permissions: PermissionConfig;
  mcp: McpConfig;
}

/** UI-only: whether to render thinking blocks. Does not affect storage or model APIs. */
export type AgentTraceMode = 'concise' | 'verbose';

function loadAgentTrace(): AgentTraceMode {
  const raw = localStorage.getItem('agent_core_agent_trace');
  return raw === 'verbose' ? 'verbose' : 'concise';
}

interface SettingsState {
  isOpen: boolean;
  activeTab: 'general' | 'provider' | 'memory' | 'mcp' | 'skills' | 'permissions';
  config: AppConfig | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
  appearance: 'system' | 'dark' | 'light';
  agentTrace: AgentTraceMode;
}

const initialState: SettingsState = {
  isOpen: false,
  activeTab: 'general',
  config: null,
  loading: false,
  saving: false,
  error: null,
  appearance: (localStorage.getItem('agent_core_appearance') as 'system' | 'dark' | 'light') || 'system',
  agentTrace: loadAgentTrace(),
};

function normalizeProviderModel(raw: Record<string, unknown>): ProviderModelEntry {
  return {
    model_id: (raw.model_id as string) ?? '',
    temperature: (raw.temperature as number) ?? undefined,
    max_tokens: (raw.max_tokens as number) ?? undefined,
    system_prompt: (raw.system_prompt as string) ?? undefined,
    max_context_tokens: (raw.max_context_tokens as number) ?? undefined,
    reasoning_effort: (raw.reasoning_effort as string) ?? undefined,
    thinking_enabled: (raw.thinking_enabled as boolean) ?? false,
  };
}

function normalizeProvider(raw: Record<string, unknown>): ProviderConfig {
  const rawModels = (raw.models as Record<string, unknown>) ?? {};
  const models: Record<string, ProviderModelEntry> = {};
  for (const [key, value] of Object.entries(rawModels)) {
    models[key] = normalizeProviderModel(value as Record<string, unknown>);
  }
  return {
    name: (raw.name as string) ?? '',
    base_url: (raw.base_url as string) ?? '',
    api_key: (raw.api_key as string) ?? '',
    max_context_tokens: (raw.max_context_tokens as number) ?? 32768,
    temperature: (raw.temperature as number) ?? undefined,
    max_tokens: (raw.max_tokens as number) ?? undefined,
    react_enabled: (raw.react_enabled as boolean) ?? true,
    system_prompt: (raw.system_prompt as string) ?? undefined,
    max_iterations: (raw.max_iterations as number) ?? 100,
    request_timeout_secs: (raw.request_timeout_secs as number) ?? 60,
    models,
  };
}

function normalizeMemory(raw: Record<string, unknown>): MemoryConfig {
  const reflection = (raw.reflection as Record<string, unknown>) ?? {};
  return {
    db_path: (raw.db_path as string) ?? '~/.agent_core/memory.db',
    embedding_model: (raw.embedding_model as string) ?? 'BAAI/bge-small-en-v1.5',
    max_core_blocks: (raw.max_core_blocks as number) ?? 5,
    default_block_max_chars: (raw.default_block_max_chars as number) ?? 2000,
    consolidation_enabled: (raw.consolidation_enabled as boolean) ?? true,
    embedding_enabled: (raw.embedding_enabled as boolean) ?? false,
    mode: (raw.mode as string) ?? 'standard',
    reflection: {
      trigger_message_count: (reflection.trigger_message_count as number) ?? 20,
      reflection_model: (reflection.reflection_model as string) ?? undefined,
    },
  };
}

function normalizeConfig(raw: Record<string, unknown>): AppConfig {
  const rawProviders = (raw.providers as Record<string, unknown>) ?? {};
  const providers: Record<string, ProviderConfig> = {};
  for (const [key, value] of Object.entries(rawProviders)) {
    providers[key] = normalizeProvider(value as Record<string, unknown>);
  }
  const rawPerms = (raw.permissions as Record<string, unknown>) ?? {};
  const rawMcp = (raw.mcp as Record<string, unknown>) ?? {};
  return {
    default_model: (raw.default_model as string) ?? '',
    providers,
    memory: raw.memory ? normalizeMemory(raw.memory as Record<string, unknown>) : undefined,
    permissions: {
      mode: (rawPerms.mode as string) ?? 'standard',
      auto_allow_up_to: (rawPerms.auto_allow_up_to as string) ?? undefined,
      rules: (rawPerms.rules as PermissionRule[]) ?? [],
      whitelist: (rawPerms.whitelist as WhitelistEntry[]) ?? [],
      blacklist: (rawPerms.blacklist as BlacklistEntry[]) ?? [],
    },
    mcp: {
      servers: (rawMcp.servers as McpServerConfig[]) ?? [],
    },
  };
}

export const fetchConfig = createAsyncThunk('settings/fetchConfig', async (_, { rejectWithValue }) => {
  try {
    const raw = await invoke<Record<string, unknown>>('get_config');
    return normalizeConfig(raw);
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const saveConfig = createAsyncThunk('settings/saveConfig', async (config: AppConfig, { rejectWithValue }) => {
  try {
    await invoke('save_config', { config });
    return config;
  } catch (e) {
    return rejectWithValue(String(e));
  }
});

export const switchModel = createAsyncThunk(
  'settings/switchModel',
  async (
    { modelKey, currentConfig }: { modelKey: string; currentConfig: AppConfig },
    { dispatch, rejectWithValue }
  ) => {
    dispatch(setDefaultModel(modelKey));
    const newConfig = { ...currentConfig, default_model: modelKey };
    // Switch the runtime model first; if that succeeds, persist to config.
    // This avoids the config/runtime inconsistency where save_config succeeds
    // but switch_model fails (P2-2).
    try {
      await invoke('switch_model', { name: modelKey });
    } catch (e) {
      dispatch(setDefaultModel(currentConfig.default_model));
      return rejectWithValue(String(e));
    }
    try {
      await invoke('save_config', { config: newConfig });
    } catch (e) {
      // Runtime switched but config not saved — not critical, log and proceed.
      console.warn('Model switched but config save failed:', e);
    }
    return newConfig;
  }
);

export type ModelSettingPatch = {
  thinking_enabled?: boolean;
  reasoning_effort?: string | null;
  max_context_tokens?: number;
  max_tokens?: number;
};

/** Patch a provider model entry and persist to config (chat menu / settings). */
export const updateModelSettings = createAsyncThunk(
  'settings/updateModelSettings',
  async (
    {
      modelKey,
      patch,
      currentConfig,
      alsoSwitch = false,
    }: {
      modelKey: string;
      patch: ModelSettingPatch;
      currentConfig: AppConfig;
      alsoSwitch?: boolean;
    },
    { rejectWithValue }
  ) => {
    const slash = modelKey.indexOf('/');
    if (slash < 0) return rejectWithValue('Invalid model key');
    const providerKey = modelKey.slice(0, slash);
    const entryKey = modelKey.slice(slash + 1);
    const provider = currentConfig.providers[providerKey];
    if (!provider?.models[entryKey]) return rejectWithValue('Model not found');

    const prev = provider.models[entryKey];
    const nextEntry: ProviderModelEntry = {
      ...prev,
      ...(patch.thinking_enabled !== undefined
        ? { thinking_enabled: patch.thinking_enabled }
        : {}),
      ...(patch.reasoning_effort !== undefined
        ? { reasoning_effort: patch.reasoning_effort ?? undefined }
        : {}),
      ...(patch.max_context_tokens !== undefined
        ? { max_context_tokens: patch.max_context_tokens }
        : {}),
      ...(patch.max_tokens !== undefined ? { max_tokens: patch.max_tokens } : {}),
    };

    const newConfig: AppConfig = {
      ...currentConfig,
      default_model: alsoSwitch ? modelKey : currentConfig.default_model,
      providers: {
        ...currentConfig.providers,
        [providerKey]: {
          ...provider,
          models: {
            ...provider.models,
            [entryKey]: nextEntry,
          },
        },
      },
    };

    try {
      await invoke('save_config', { config: newConfig });
      if (alsoSwitch || modelKey === currentConfig.default_model) {
        await invoke('switch_model', { name: modelKey });
      }
      return newConfig;
    } catch (e) {
      return rejectWithValue(String(e));
    }
  }
);

export const settingsSlice = createSlice({
  name: 'settings',
  initialState,
  reducers: {
    setAppearance: (state, action: PayloadAction<'system' | 'dark' | 'light'>) => {
      state.appearance = action.payload;
      localStorage.setItem('agent_core_appearance', action.payload);
    },
    setAgentTrace: (state, action: PayloadAction<AgentTraceMode>) => {
      state.agentTrace = action.payload;
      localStorage.setItem('agent_core_agent_trace', action.payload);
    },
    openSettings: (state) => {
      state.isOpen = true;
    },
    closeSettings: (state) => {
      state.isOpen = false;
    },
    setActiveTab: (state, action: PayloadAction<SettingsState['activeTab']>) => {
      state.activeTab = action.payload;
    },
    upsertProvider: (state, action: PayloadAction<{ key: string; provider: ProviderConfig }>) => {
      if (!state.config) return;
      state.config.providers[action.payload.key] = action.payload.provider;
    },
    deleteProvider: (state, action: PayloadAction<string>) => {
      if (!state.config) return;
      const providerKey = action.payload;
      delete state.config.providers[providerKey];
      if (state.config.default_model.startsWith(providerKey + '/')) {
        const remaining = Object.entries(state.config.providers);
        if (remaining.length > 0) {
          const [firstKey, firstProvider] = remaining[0];
          const firstModelKey = Object.keys(firstProvider.models)[0] ?? '';
          state.config.default_model = firstModelKey ? `${firstKey}/${firstModelKey}` : '';
        } else {
          state.config.default_model = '';
        }
      }
    },
    setDefaultModel: (state, action: PayloadAction<string>) => {
      if (!state.config) return;
      state.config.default_model = action.payload;
    },
    updateProvider: (state, action: PayloadAction<{ oldKey: string; newKey: string; provider: ProviderConfig }>) => {
      if (!state.config) return;
      const { oldKey, newKey, provider } = action.payload;
      if (oldKey !== newKey) {
        delete state.config.providers[oldKey];
        if (state.config.default_model.startsWith(oldKey + '/')) {
          state.config.default_model = state.config.default_model.replace(oldKey + '/', newKey + '/');
        }
      }
      state.config.providers[newKey] = provider;
    },
    upsertMcpServer: (state, action: PayloadAction<{ oldName?: string; server: McpServerConfig }>) => {
      if (!state.config) return;
      const { oldName, server } = action.payload;
      const servers = state.config.mcp.servers;
      const idx = oldName
        ? servers.findIndex((s) => s.name === oldName)
        : servers.findIndex((s) => s.name === server.name);
      if (idx >= 0) {
        servers[idx] = { ...server };
      } else {
        servers.push({ ...server });
      }
    },
    deleteMcpServer: (state, action: PayloadAction<string>) => {
      if (!state.config) return;
      state.config.mcp.servers = state.config.mcp.servers.filter((s) => s.name !== action.payload);
    },
    toggleMcpServer: (state, action: PayloadAction<string>) => {
      if (!state.config) return;
      const server = state.config.mcp.servers.find((s) => s.name === action.payload);
      if (server) {
        server.enabled = server.enabled === false ? true : false;
      }
    },
  },
  extraReducers: (builder) => {
    builder
      .addCase(fetchConfig.pending, (state) => {
        state.loading = true;
        state.error = null;
      })
      .addCase(fetchConfig.fulfilled, (state, action) => {
        state.loading = false;
        state.config = action.payload;
      })
      .addCase(fetchConfig.rejected, (state, action) => {
        state.loading = false;
        state.error = action.payload as string;
      })
      .addCase(saveConfig.pending, (state) => {
        state.saving = true;
        state.error = null;
      })
      .addCase(saveConfig.fulfilled, (state, action) => {
        state.saving = false;
        state.config = action.payload;
      })
      .addCase(saveConfig.rejected, (state, action) => {
        state.saving = false;
        state.error = action.payload as string;
      })
      .addCase(switchModel.fulfilled, (state, action) => {
        state.config = action.payload;
      })
      .addCase(updateModelSettings.pending, (state) => {
        state.saving = true;
        state.error = null;
      })
      .addCase(updateModelSettings.fulfilled, (state, action) => {
        state.saving = false;
        state.config = action.payload;
      })
      .addCase(updateModelSettings.rejected, (state, action) => {
        state.saving = false;
        state.error = action.payload as string;
      });
  },
});

export const { setAppearance, setAgentTrace, openSettings, closeSettings, setActiveTab, upsertProvider, deleteProvider, setDefaultModel, updateProvider, upsertMcpServer, deleteMcpServer, toggleMcpServer } = settingsSlice.actions;
export default settingsSlice.reducer;
