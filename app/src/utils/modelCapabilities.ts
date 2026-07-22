/**
 * Frontend mirror of core/src/model_capabilities.rs.
 * Keep patterns in sync when adding models; Rust remains source of truth for runtime.
 *
 * Sources (Jul 2026):
 * - NVIDIA NIM: docs.api.nvidia.com/nim/reference/llm-apis
 * - Kimi: platform.kimi.ai/docs/models
 * - MiniMax: platform.minimax.io/docs/guides/text-generation
 * - Hunyuan: cloud.tencent.com TokenHub model list
 * - GLM: docs.bigmodel.cn/cn/guide/start/model-overview
 */

export interface ModelCapabilities {
  context_tokens: number;
  max_output_tokens: number | null;
  supports_thinking: boolean;
  effort_levels: string[];
  context_presets: number[];
  supports_fast: boolean;
  supports_images: boolean;
}

export const DEFAULT_CONTEXT_TOKENS = 128_000;

const EFFORT_STANDARD = ['low', 'medium', 'high'];
const EFFORT_EXTENDED = ['low', 'medium', 'high', 'xhigh', 'max'];
const EFFORT_OPENAI = ['low', 'medium', 'high', 'xhigh'];
const EFFORT_HY3 = ['low', 'high'];
const EFFORT_KIMI_K3 = ['max'];

function caps(
  context_tokens: number,
  max_output_tokens: number | null,
  supports_thinking: boolean,
  effort_levels: string[],
  context_presets: number[],
  supports_fast = false
): ModelCapabilities {
  return {
    context_tokens,
    max_output_tokens,
    supports_thinking,
    effort_levels,
    context_presets,
    supports_fast,
    supports_images: false,
  };
}

const PRESETS_128K = [128_000];
const PRESETS_200K = [128_000, 200_000];
const PRESETS_204K = [128_000, 204_800];
const PRESETS_256K = [128_000, 256_000];
const PRESETS_NEMOTRON3 = [128_000, 256_000, 1_000_000];
const PRESETS_1M = [128_000, 200_000, 1_000_000];
const PRESETS_GEMINI = [128_000, 1_000_000, 2_000_000];
const PRESETS_64K = [64_000];
const PRESETS_32K = [32_768];
const PRESETS_16K = [16_384];
const PRESETS_8K = [8_192];
const PRESETS_4K = [4_096];

function claude200k(): ModelCapabilities {
  return caps(200_000, 64_000, true, EFFORT_EXTENDED, PRESETS_200K);
}

function nemotron3(supportsFast = false): ModelCapabilities {
  return caps(1_000_000, 65_536, true, EFFORT_STANDARD, PRESETS_NEMOTRON3, supportsFast);
}

function openaiReasoning(context: number, presets: number[]): ModelCapabilities {
  return caps(context, 100_000, true, EFFORT_OPENAI, presets);
}

function conservativeDefault(): ModelCapabilities {
  return caps(DEFAULT_CONTEXT_TOKENS, null, false, [], PRESETS_128K);
}

function normalizeModelId(modelId: string): string {
  const id = modelId.trim().toLowerCase();
  const slash = id.lastIndexOf('/');
  return slash >= 0 ? id.slice(slash + 1) : id;
}

function exactMatch(id: string): ModelCapabilities | null {
  const map: Record<string, ModelCapabilities> = {
    // DeepSeek
    'deepseek-chat': caps(64_000, 8_192, false, [], PRESETS_64K),
    'deepseek-v3': caps(64_000, 8_192, false, [], PRESETS_64K),
    'deepseek-reasoner': caps(64_000, 8_192, true, EFFORT_STANDARD, PRESETS_64K),
    'deepseek-r1': caps(64_000, 8_192, true, EFFORT_STANDARD, PRESETS_64K),
    'deepseek-v4-flash': caps(1_000_000, 384_000, true, EFFORT_STANDARD, PRESETS_1M, true),
    'deepseek-v4': caps(1_000_000, 384_000, true, EFFORT_STANDARD, PRESETS_1M, true),
    'deepseek-v4-pro': caps(1_000_000, 384_000, true, EFFORT_STANDARD, PRESETS_1M),

    // NVIDIA Nemotron
    'nemotron-3-nano-30b-a3b': nemotron3(true),
    'nemotron-3-super-120b-a12b': nemotron3(),
    'nemotron-3-ultra-550b-a55b': nemotron3(),
    'llama-3.3-nemotron-super-49b-v1': caps(131_072, 16_384, true, EFFORT_STANDARD, PRESETS_128K),
    'llama-3.3-nemotron-super-49b-v1.5': caps(131_072, 16_384, true, EFFORT_STANDARD, PRESETS_128K),
    'llama-3.1-nemotron-ultra-253b-v1': caps(131_072, 16_384, true, EFFORT_STANDARD, PRESETS_128K),
    'llama-3.1-nemotron-nano-8b-v1': caps(128_000, 8_192, true, EFFORT_STANDARD, PRESETS_128K, true),
    'nvidia-nemotron-nano-9b-v2': caps(128_000, 8_192, true, EFFORT_STANDARD, PRESETS_128K, true),
    'nemotron-mini-4b-instruct': caps(128_000, 8_192, true, EFFORT_STANDARD, PRESETS_128K, true),

    // Kimi
    'kimi-k3': caps(1_000_000, 128_000, true, EFFORT_KIMI_K3, PRESETS_1M),
    'kimi-k2.7-code': caps(256_000, 256_000, true, EFFORT_STANDARD, PRESETS_256K, true),
    'kimi-k2.7-code-highspeed': caps(256_000, 256_000, true, EFFORT_STANDARD, PRESETS_256K, true),
    'kimi-k2.6': caps(256_000, 32_768, true, EFFORT_STANDARD, PRESETS_256K),
    'kimi-k2.5': caps(256_000, 32_768, true, EFFORT_STANDARD, PRESETS_256K),
    'kimi-k2-instruct': caps(128_000, 8_192, false, [], PRESETS_128K),
    'kimi-k2-thinking': caps(256_000, 32_768, true, EFFORT_STANDARD, PRESETS_256K),

    // MiniMax
    'minimax-m3': caps(1_000_000, 128_000, true, EFFORT_STANDARD, PRESETS_1M),
    'minimax-m2.7': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K),
    'minimax-m2.7-highspeed': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K, true),
    'minimax-m2.5': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K),
    'minimax-m2.5-highspeed': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K, true),
    'minimax-m2.1': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K),
    'minimax-m2': caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K),
    'm2-her': caps(64_000, 8_192, false, [], PRESETS_64K),

    // Hunyuan
    hy3: caps(256_000, 128_000, true, EFFORT_HY3, PRESETS_256K),
    'hy3-preview': caps(256_000, 128_000, true, EFFORT_HY3, PRESETS_256K),
    'hunyuan-role-latest': caps(32_000, 4_096, false, [], PRESETS_32K),
    'hy-role': caps(32_000, 4_096, false, [], PRESETS_32K),

    // GLM
    'glm-5.2': caps(1_000_000, 128_000, true, EFFORT_STANDARD, PRESETS_1M),
    'glm5.2': caps(1_000_000, 128_000, true, EFFORT_STANDARD, PRESETS_1M),
    'glm-5.1': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm5.1': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm-5': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm-5-turbo': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm-4.7': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm4.7': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm-4.6': caps(200_000, 128_000, true, EFFORT_STANDARD, PRESETS_200K),
    'glm-4.5-air': caps(128_000, 96_000, true, EFFORT_STANDARD, PRESETS_128K),
    'glm-4-long': caps(1_000_000, 4_096, false, [], PRESETS_1M),

    // Claude / OpenAI / Gemini / Llama (unchanged)
    'claude-3-5-sonnet': claude200k(),
    'claude-3-5-haiku': claude200k(),
    'claude-sonnet-4': claude200k(),
    'claude-sonnet-4-5': claude200k(),
    'claude-opus-4': claude200k(),
    'claude-opus-4-5': claude200k(),
    'claude-opus-4-6': claude200k(),
    'claude-haiku-4-5': claude200k(),
    'gpt-4o': caps(128_000, 16_384, false, [], PRESETS_128K),
    'gpt-4o-mini': caps(128_000, 16_384, false, [], PRESETS_128K),
    'gpt-4.1': caps(1_000_000, 32_768, false, [], PRESETS_1M),
    'gpt-4.1-mini': caps(1_000_000, 32_768, false, [], PRESETS_1M),
    'gpt-5': openaiReasoning(400_000, PRESETS_1M),
    'gpt-5-mini': openaiReasoning(400_000, PRESETS_1M),
    o1: openaiReasoning(200_000, PRESETS_200K),
    'o1-mini': openaiReasoning(200_000, PRESETS_200K),
    o3: openaiReasoning(200_000, PRESETS_200K),
    'o3-mini': openaiReasoning(200_000, PRESETS_200K),
    'o4-mini': openaiReasoning(200_000, PRESETS_200K),
    'gemini-1.5-flash': caps(1_000_000, 8_192, false, [], PRESETS_GEMINI, true),
    'gemini-1.5-pro': caps(2_000_000, 8_192, false, [], PRESETS_GEMINI),
    'gemini-2.0-flash': caps(1_000_000, 65_536, true, EFFORT_STANDARD, PRESETS_GEMINI, true),
    'gemini-2.5-flash': caps(1_000_000, 65_536, true, EFFORT_STANDARD, PRESETS_GEMINI, true),
    'gemini-2.5-pro': caps(1_000_000, 65_536, true, EFFORT_STANDARD, PRESETS_GEMINI),
    'llama3-8b': caps(8_192, 4_096, false, [], PRESETS_8K),
    'llama-3.1-70b': caps(128_000, 4_096, false, [], PRESETS_128K),
  };
  return map[id] ?? null;
}

function familyMatch(id: string): ModelCapabilities | null {
  if (id.includes('deepseek')) {
    if (id.includes('v4')) {
      return caps(1_000_000, 384_000, true, EFFORT_STANDARD, PRESETS_1M, id.includes('flash'));
    }
    if (id.includes('reasoner') || id.includes('r1')) {
      return caps(64_000, 8_192, true, EFFORT_STANDARD, PRESETS_64K);
    }
    if (id.includes('flash')) {
      return caps(128_000, 8_192, true, EFFORT_STANDARD, PRESETS_128K, true);
    }
    return caps(64_000, 8_192, false, [], PRESETS_64K);
  }

  if (id.includes('nemotron')) {
    if (id.includes('nemotron-3') || id.includes('nemotron_3')) {
      return nemotron3(id.includes('nano'));
    }
    return caps(
      128_000,
      16_384,
      true,
      EFFORT_STANDARD,
      PRESETS_128K,
      id.includes('nano') || id.includes('mini')
    );
  }

  if (id.includes('kimi') || id.startsWith('moonshot')) {
    if (id.includes('k3')) return caps(1_000_000, 128_000, true, EFFORT_KIMI_K3, PRESETS_1M);
    if (id.includes('k2.7') || id.includes('k2-7')) {
      return caps(
        256_000,
        256_000,
        true,
        EFFORT_STANDARD,
        PRESETS_256K,
        id.includes('highspeed') || id.includes('turbo')
      );
    }
    if (id.includes('thinking')) {
      return caps(256_000, 32_768, true, EFFORT_STANDARD, PRESETS_256K);
    }
    if (id.includes('k2.6') || id.includes('k2.5') || id.includes('k2-0905') || id.includes('k2-turbo')) {
      return caps(256_000, 32_768, true, EFFORT_STANDARD, PRESETS_256K);
    }
    return caps(128_000, 8_192, false, [], PRESETS_128K);
  }

  if (id.includes('minimax') || id.startsWith('m2-her')) {
    if (id.includes('m3')) return caps(1_000_000, 128_000, true, EFFORT_STANDARD, PRESETS_1M);
    if (id.includes('her')) return caps(64_000, 8_192, false, [], PRESETS_64K);
    return caps(204_800, 128_000, true, EFFORT_STANDARD, PRESETS_204K, id.includes('highspeed'));
  }

  if (
    id.startsWith('hy3') ||
    id.includes('hunyuan') ||
    id.startsWith('hy-') ||
    id.startsWith('hy_')
  ) {
    if (id.startsWith('hy3')) return caps(256_000, 128_000, true, EFFORT_HY3, PRESETS_256K);
    if (id.includes('role')) return caps(32_000, 4_096, false, [], PRESETS_32K);
    if (id.includes('t1') || id.includes('turbos') || id.includes('thinking')) {
      return caps(128_000, 16_384, true, EFFORT_STANDARD, PRESETS_128K);
    }
    return caps(128_000, 16_384, false, [], PRESETS_128K);
  }

  if (id.includes('glm')) {
    if (id.includes('5.2') || id.includes('glm5.2')) {
      return caps(1_000_000, 128_000, true, EFFORT_STANDARD, PRESETS_1M);
    }
    if (
      id.includes('5.1') ||
      id.includes('glm5.1') ||
      id.includes('glm-5') ||
      id.includes('glm5') ||
      id.includes('4.7') ||
      id.includes('glm4.7') ||
      id.includes('4.6')
    ) {
      return caps(
        200_000,
        128_000,
        true,
        EFFORT_STANDARD,
        PRESETS_200K,
        id.includes('flash') || id.includes('turbo')
      );
    }
    if (id.includes('4-long') || id.includes('4_long')) {
      return caps(1_000_000, 4_096, false, [], PRESETS_1M);
    }
    if (id.includes('4.5') || id.includes('4-flash')) {
      return caps(
        128_000,
        96_000,
        true,
        EFFORT_STANDARD,
        PRESETS_128K,
        id.includes('flash') || id.includes('airx')
      );
    }
    return caps(128_000, 16_384, false, [], PRESETS_128K);
  }

  if (id.includes('claude')) return claude200k();
  if (id.startsWith('o1') || id.startsWith('o3') || id.startsWith('o4') || id.includes('codex')) {
    return openaiReasoning(200_000, PRESETS_200K);
  }
  if (id.startsWith('gpt-5')) return openaiReasoning(400_000, PRESETS_1M);
  if (id.startsWith('gpt-4.1')) return caps(1_000_000, 32_768, false, [], PRESETS_1M);
  if (
    id.includes('gpt-4o') ||
    id.includes('gpt-4-turbo') ||
    id.includes('gpt-4-1106') ||
    id.includes('gpt-4-0125')
  ) {
    return caps(128_000, 16_384, false, [], PRESETS_128K);
  }
  if (id.includes('gpt-4-32k')) return caps(32_768, 8_192, false, [], PRESETS_32K);
  if (id.includes('gpt-4')) return caps(8_192, 8_192, false, [], PRESETS_8K);
  if (id.includes('gpt-3.5')) {
    return id.includes('16k')
      ? caps(16_384, 4_096, false, [], PRESETS_16K)
      : caps(4_096, 4_096, false, [], PRESETS_4K);
  }
  if (id.includes('gemini')) {
    if (id.includes('pro')) {
      const thinking = id.includes('2.5') || id.includes('2.0');
      return caps(
        id.includes('1.5-pro') ? 2_000_000 : 1_000_000,
        65_536,
        thinking,
        thinking ? EFFORT_STANDARD : [],
        PRESETS_GEMINI
      );
    }
    if (id.includes('flash')) {
      const thinking = id.includes('2.5') || id.includes('2.0');
      return caps(
        1_000_000,
        8_192,
        thinking,
        thinking ? EFFORT_STANDARD : [],
        PRESETS_GEMINI,
        true
      );
    }
    return caps(1_000_000, 8_192, false, [], PRESETS_GEMINI);
  }
  if (
    id.includes('llama-3.1') ||
    id.includes('llama3.1') ||
    id.includes('llama-3.2') ||
    id.includes('llama-3.3') ||
    id.includes('llama-3-')
  ) {
    return caps(128_000, 4_096, false, [], PRESETS_128K);
  }
  if (id.includes('llama-3') || id.includes('llama3')) {
    return caps(8_192, 4_096, false, [], PRESETS_8K);
  }
  if (id.includes('llama-2') || id.includes('llama2')) {
    return caps(4_096, 2_048, false, [], PRESETS_4K);
  }
  if (id.includes('qwen')) {
    if (id.includes('qwen3') || id.includes('qwen2.5')) {
      const thinking = id.includes('think') || id.includes('reason');
      return caps(
        128_000,
        8_192,
        thinking,
        thinking ? EFFORT_STANDARD : [],
        PRESETS_128K,
        id.includes('flash') || id.includes('turbo')
      );
    }
    return caps(32_768, 8_192, false, [], PRESETS_32K);
  }
  if (id.includes('grok')) {
    return caps(256_000, 16_384, true, EFFORT_EXTENDED, [128_000, 256_000], id.includes('fast'));
  }
  return null;
}

function modelSupportsImages(id: string): boolean {
  // OpenAI / Anthropic / Google
  if (
    id.startsWith('claude') ||
    id.includes('gpt-4o') ||
    id.includes('gpt-4.1') ||
    id.includes('gpt-4-turbo') ||
    id.includes('gpt-4-vision') ||
    id.startsWith('o3') ||
    id.startsWith('o4-') ||
    id.includes('gemini')
  ) {
    return true;
  }

  // Explicit vision / VL / Omni markers
  if (id.includes('vision') || id.includes('-vl') || id.includes('_vl') || id.includes('omni')) {
    return true;
  }

  // Kimi / Moonshot vision models:
  // kimi-k3, kimi-k2.5, kimi-k2.6, kimi-k2.7-code(+highspeed). Older kimi-k2 /
  // kimi-k2-thinking / kimi-k2-instruct are text-only.
  if (
    id.startsWith('kimi-k3') ||
    id.startsWith('kimi-k2.5') ||
    id.startsWith('kimi-k2.6') ||
    id.startsWith('kimi-k2.7') ||
    (id.startsWith('moonshot') && id.includes('vision'))
  ) {
    return true;
  }

  // NVIDIA: only Omni / VL variants are multimodal. Base Nemotron 3 Nano /
  // Super / Ultra are text-only per NIM docs.
  if (id.includes('nemotron') && (id.includes('omni') || id.includes('vl'))) {
    return true;
  }

  return false;
}

export function lookupCapabilities(modelId: string): ModelCapabilities {
  const id = normalizeModelId(modelId);
  const found = exactMatch(id) ?? familyMatch(id) ?? conservativeDefault();
  if (modelSupportsImages(id)) {
    return { ...found, supports_images: true };
  }
  return found;
}

/** Resolve vision support: config override wins; else curated registry (unknown → false). */
export function resolveSupportsImages(
  modelId: string,
  overrideFlag?: boolean | null
): boolean {
  if (typeof overrideFlag === 'boolean') return overrideFlag;
  return modelSupportsImages(normalizeModelId(modelId));
}

export function resolveContextTokens(modelId: string, configured: number): number {
  if (configured !== DEFAULT_CONTEXT_TOKENS) return configured;
  return lookupCapabilities(modelId).context_tokens;
}

export function formatContextLabel(tokens: number): string {
  if (tokens >= 1_000_000 && tokens % 1_000_000 === 0) {
    return `${tokens / 1_000_000}M`;
  }
  if (tokens >= 1_000) {
    const k = tokens / 1_000;
    if (Math.abs(k - Math.round(k)) < 0.05) return `${Math.round(k)}K`;
    return `${k.toFixed(1)}K`;
  }
  return String(tokens);
}

export function formatEffortLabel(effort: string | undefined | null): string {
  if (!effort) return '';
  const map: Record<string, string> = {
    low: 'Low',
    medium: 'Medium',
    high: 'High',
    xhigh: 'Extra High',
    max: 'Max',
  };
  return map[effort.toLowerCase()] ?? effort;
}

/** Closest preset at or below configured, else registry default. */
export function effectiveContextTokens(
  modelId: string,
  configured: number | undefined,
  providerDefault: number
): number {
  const raw = configured ?? providerDefault;
  return resolveContextTokens(modelId, raw);
}
