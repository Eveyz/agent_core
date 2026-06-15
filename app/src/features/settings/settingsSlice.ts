import { createSlice, createAsyncThunk, PayloadAction } from '@reduxjs/toolkit';
import { invoke } from '@tauri-apps/api/core';

export interface ProviderModelEntry {
  model_id: string;
  temperature?: number;
  max_tokens?: number;
  system_prompt?: string;
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

export interface MemoryConfig {
  db_path: string;
  embedding_model: string;
  max_core_blocks: number;
  default_block_max_chars: number;
  consolidation_enabled: boolean;
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

interface SettingsState {
  isOpen: boolean;
  activeTab: 'general' | 'provider' | 'memory' | 'mcp' | 'skills';
  config: AppConfig | null;
  loading: boolean;
  saving: boolean;
  error: string | null;
}

const initialState: SettingsState = {
  isOpen: false,
  activeTab: 'general',
  config: null,
  loading: false,
  saving: false,
  error: null,
};

function normalizeProviderModel(raw: any): ProviderModelEntry {
  return {
    model_id: raw?.model_id ?? '',
    temperature: raw?.temperature ?? undefined,
    max_tokens: raw?.max_tokens ?? undefined,
    system_prompt: raw?.system_prompt ?? undefined,
  };
}

function normalizeProvider(raw: any): ProviderConfig {
  const rawModels = raw?.models ?? {};
  const models: Record<string, ProviderModelEntry> = {};
  for (const [key, value] of Object.entries(rawModels)) {
    models[key] = normalizeProviderModel(value);
  }
  return {
    name: raw?.name ?? '',
    base_url: raw?.base_url ?? '',
    api_key: raw?.api_key ?? '',
    max_context_tokens: raw?.max_context_tokens ?? 32768,
    temperature: raw?.temperature ?? undefined,
    max_tokens: raw?.max_tokens ?? undefined,
    react_enabled: raw?.react_enabled ?? true,
    system_prompt: raw?.system_prompt ?? undefined,
    max_iterations: raw?.max_iterations ?? 10,
    request_timeout_secs: raw?.request_timeout_secs ?? 60,
    models,
  };
}

function normalizeMemory(raw: any): MemoryConfig {
  return {
    db_path: raw?.db_path ?? '~/.agent_core/memory.db',
    embedding_model: raw?.embedding_model ?? 'BAAI/bge-small-en-v1.5',
    max_core_blocks: raw?.max_core_blocks ?? 5,
    default_block_max_chars: raw?.default_block_max_chars ?? 2000,
    consolidation_enabled: raw?.consolidation_enabled ?? true,
  };
}

function normalizeConfig(raw: any): AppConfig {
  const rawProviders = raw?.providers ?? {};
  const providers: Record<string, ProviderConfig> = {};
  for (const [key, value] of Object.entries(rawProviders)) {
    providers[key] = normalizeProvider(value);
  }
  return {
    default_model: raw?.default_model ?? '',
    providers,
    memory: raw?.memory ? normalizeMemory(raw.memory) : undefined,
    permissions: {
      mode: raw?.permissions?.mode ?? 'standard',
      auto_allow_up_to: raw?.permissions?.auto_allow_up_to ?? undefined,
      rules: raw?.permissions?.rules ?? [],
      whitelist: raw?.permissions?.whitelist ?? [],
      blacklist: raw?.permissions?.blacklist ?? [],
    },
    mcp: {
      servers: raw?.mcp?.servers ?? [],
    },
  };
}

export const fetchConfig = createAsyncThunk('settings/fetchConfig', async (_, { rejectWithValue }) => {
  try {
    const raw = await invoke<any>('get_config');
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

export const settingsSlice = createSlice({
  name: 'settings',
  initialState,
  reducers: {
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
      });
  },
});

export const { openSettings, closeSettings, setActiveTab, upsertProvider, deleteProvider, setDefaultModel } = settingsSlice.actions;
export default settingsSlice.reducer;
