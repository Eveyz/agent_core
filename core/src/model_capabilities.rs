//! Curated model capability registry keyed by `model_id` (provider-agnostic).
//!
//! Same model id (e.g. `deepseek-chat`) gets the same defaults whether it is
//! served via DeepSeek, OpenRouter, or a self-hosted gateway.

use serde::{Deserialize, Serialize};

/// Default context window used when config omits / leaves the provider default.
pub const DEFAULT_CONTEXT_TOKENS: usize = 128_000;

const EFFORT_STANDARD: &[&str] = &["low", "medium", "high"];
const EFFORT_EXTENDED: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const EFFORT_OPENAI: &[&str] = &["low", "medium", "high", "xhigh"];

const PRESETS_128K: &[usize] = &[128_000];
const PRESETS_200K: &[usize] = &[128_000, 200_000];
const PRESETS_204K: &[usize] = &[128_000, 204_800];
const PRESETS_256K: &[usize] = &[128_000, 256_000];
const PRESETS_NEMOTRON3: &[usize] = &[128_000, 256_000, 1_000_000];
const PRESETS_1M: &[usize] = &[128_000, 200_000, 1_000_000];
const PRESETS_GEMINI: &[usize] = &[128_000, 1_000_000, 2_000_000];
const PRESETS_64K: &[usize] = &[64_000];
const PRESETS_32K: &[usize] = &[32_768];
const PRESETS_16K: &[usize] = &[16_384];
const PRESETS_8K: &[usize] = &[8_192];
const PRESETS_4K: &[usize] = &[4_096];

const EFFORT_HY3: &[&str] = &["low", "high"];
const EFFORT_KIMI_K3: &[&str] = &["max"];

/// Capabilities inferred from a model id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelCapabilities {
    pub context_tokens: usize,
    pub max_output_tokens: Option<u32>,
    pub supports_thinking: bool,
    /// Effort levels the model accepts (empty = no effort UI).
    pub effort_levels: Vec<String>,
    /// Context window presets offered in the model menu.
    pub context_presets: Vec<usize>,
    pub supports_fast: bool,
}

impl ModelCapabilities {
    fn owned(
        context_tokens: usize,
        max_output_tokens: Option<u32>,
        supports_thinking: bool,
        effort_levels: &[&str],
        context_presets: &[usize],
        supports_fast: bool,
    ) -> Self {
        Self {
            context_tokens,
            max_output_tokens,
            supports_thinking,
            effort_levels: effort_levels.iter().map(|s| (*s).to_string()).collect(),
            context_presets: context_presets.to_vec(),
            supports_fast,
        }
    }

    fn conservative_default() -> Self {
        Self::owned(
            DEFAULT_CONTEXT_TOKENS,
            None,
            false,
            &[],
            PRESETS_128K,
            false,
        )
    }
}

/// Look up curated capabilities for a model id (case-insensitive).
/// Accepts bare ids (`kimi-k3`) or org-prefixed NIM ids (`nvidia/nemotron-3-nano-30b-a3b`).
pub fn lookup_capabilities(model_id: &str) -> ModelCapabilities {
    let id = normalize_model_id(model_id);

    if let Some(caps) = exact_match(&id) {
        return caps;
    }
    if let Some(caps) = family_match(&id) {
        return caps;
    }
    ModelCapabilities::conservative_default()
}

fn normalize_model_id(model_id: &str) -> String {
    let id = model_id.trim().to_lowercase();
    // NIM / HF style: `org/name` → `name`
    if let Some((_, name)) = id.rsplit_once('/') {
        name.to_string()
    } else {
        id
    }
}

/// Resolve effective context window: registry when config is still at default.
pub fn resolve_context_tokens(model_id: &str, configured: usize) -> usize {
    if configured != DEFAULT_CONTEXT_TOKENS {
        return configured;
    }
    lookup_capabilities(model_id).context_tokens
}

/// Resolve max output: config wins when set, else registry.
pub fn resolve_max_output_tokens(model_id: &str, configured: Option<u32>) -> Option<u32> {
    configured.or_else(|| lookup_capabilities(model_id).max_output_tokens)
}

fn exact_match(id: &str) -> Option<ModelCapabilities> {
    let caps = match id {
        // DeepSeek
        "deepseek-chat" | "deepseek-v3" | "deepseek-v3.1" | "deepseek-v3.2" => {
            ModelCapabilities::owned(64_000, Some(8_192), false, &[], PRESETS_64K, false)
        }
        "deepseek-reasoner" | "deepseek-r1" | "deepseek-r1-0528" => {
            ModelCapabilities::owned(64_000, Some(8_192), true, EFFORT_STANDARD, PRESETS_64K, false)
        }
        "deepseek-v4-flash" | "deepseek-v4" | "deepseek-v4-flash-202605" => {
            // TokenHub / NVIDIA NIM (2026): 1M context
            ModelCapabilities::owned(
                1_000_000,
                Some(384_000),
                true,
                EFFORT_STANDARD,
                PRESETS_1M,
                true,
            )
        }
        "deepseek-v4-pro" | "deepseek-v4-pro-202606" => ModelCapabilities::owned(
            1_000_000,
            Some(384_000),
            true,
            EFFORT_STANDARD,
            PRESETS_1M,
            false,
        ),

        // ── NVIDIA Nemotron (docs.api.nvidia.com, 2026) ──────────────
        "nemotron-3-nano-30b-a3b" | "nvidia-nemotron-3-nano-30b-a3b" => {
            nemotron3_1m(true)
        }
        "nemotron-3-super-120b-a12b" | "nvidia-nemotron-3-super-120b-a12b" => {
            nemotron3_1m(false)
        }
        "nemotron-3-ultra-550b-a55b" | "nvidia-nemotron-3-ultra-550b-a55b" => {
            nemotron3_1m(false)
        }
        "llama-3.3-nemotron-super-49b-v1"
        | "llama-3.3-nemotron-super-49b-v1.5"
        | "nvidia-llama-3.3-nemotron-super-49b-v1" => ModelCapabilities::owned(
            131_072,
            Some(16_384),
            true,
            EFFORT_STANDARD,
            PRESETS_128K,
            false,
        ),
        "llama-3.1-nemotron-ultra-253b-v1"
        | "nvidia-llama-3.1-nemotron-ultra-253b-v1" => ModelCapabilities::owned(
            131_072,
            Some(16_384),
            true,
            EFFORT_STANDARD,
            PRESETS_128K,
            false,
        ),
        "llama-3.1-nemotron-nano-8b-v1"
        | "nvidia-nemotron-nano-9b-v2"
        | "nemotron-mini-4b-instruct" => ModelCapabilities::owned(
            128_000,
            Some(8_192),
            true,
            EFFORT_STANDARD,
            PRESETS_128K,
            true,
        ),

        // ── Moonshot Kimi (platform.kimi.ai, Jul 2026) ───────────────
        "kimi-k3" => ModelCapabilities::owned(
            1_000_000,
            Some(128_000),
            true,
            EFFORT_KIMI_K3,
            PRESETS_1M,
            false,
        ),
        "kimi-k2.7-code" | "kimi-k2.7-code-highspeed" => ModelCapabilities::owned(
            256_000,
            Some(256_000),
            true,
            EFFORT_STANDARD,
            PRESETS_256K,
            true, // highspeed variant sets supports_fast via family
        ),
        "kimi-k2.6" | "kimi-k2.5" => ModelCapabilities::owned(
            256_000,
            Some(32_768),
            true,
            EFFORT_STANDARD,
            PRESETS_256K,
            false,
        ),
        "kimi-k2-instruct" | "kimi-k2" => ModelCapabilities::owned(
            128_000,
            Some(8_192),
            false,
            &[],
            PRESETS_128K,
            false,
        ),
        "kimi-k2-thinking" | "kimi-k2-thinking-turbo" => ModelCapabilities::owned(
            256_000,
            Some(32_768),
            true,
            EFFORT_STANDARD,
            PRESETS_256K,
            false,
        ),

        // ── MiniMax (platform.minimax.io) ────────────────────────────
        "minimax-m3" => ModelCapabilities::owned(
            1_000_000,
            Some(128_000),
            true,
            EFFORT_STANDARD,
            PRESETS_1M,
            false,
        ),
        "minimax-m2.7"
        | "minimax-m2.7-highspeed"
        | "minimax-m2.5"
        | "minimax-m2.5-highspeed"
        | "minimax-m2.1"
        | "minimax-m2.1-highspeed"
        | "minimax-m2" => ModelCapabilities::owned(
            204_800,
            Some(128_000),
            true,
            EFFORT_STANDARD,
            PRESETS_204K,
            false,
        ),
        "m2-her" => ModelCapabilities::owned(64_000, Some(8_192), false, &[], PRESETS_64K, false),

        // ── Tencent Hunyuan / TokenHub (cloud.tencent.com, Jul 2026) ─
        "hy3" | "hy3-preview" => ModelCapabilities::owned(
            256_000,
            Some(128_000),
            true,
            EFFORT_HY3,
            PRESETS_256K,
            false,
        ),
        "hunyuan-role-latest" | "hy-role" => {
            ModelCapabilities::owned(32_000, Some(4_096), false, &[], PRESETS_32K, false)
        }
        "hunyuan-t1-latest" | "hunyuan-turbos-latest" | "hunyuan-turbo" => {
            // Legacy (sunset Jun 2026) — keep for existing configs
            ModelCapabilities::owned(
                128_000,
                Some(16_384),
                true,
                EFFORT_STANDARD,
                PRESETS_128K,
                false,
            )
        }

        // ── Zhipu GLM (docs.bigmodel.cn) ─────────────────────────────
        "glm-5.2" | "glm5.2" => ModelCapabilities::owned(
            1_000_000,
            Some(128_000),
            true,
            EFFORT_STANDARD,
            PRESETS_1M,
            false,
        ),
        "glm-5.1" | "glm5.1" | "glm-5" | "glm-5-turbo" => ModelCapabilities::owned(
            200_000,
            Some(128_000),
            true,
            EFFORT_STANDARD,
            PRESETS_200K,
            false,
        ),
        "glm-4.7" | "glm4.7" | "glm-4.7-flash" | "glm-4.7-flashx" | "glm-4.6" => {
            ModelCapabilities::owned(
                200_000,
                Some(128_000),
                true,
                EFFORT_STANDARD,
                PRESETS_200K,
                false,
            )
        }
        "glm-4.5" | "glm-4.5-air" | "glm-4.5-airx" | "glm-4.5-flash" => {
            ModelCapabilities::owned(
                128_000,
                Some(96_000),
                true,
                EFFORT_STANDARD,
                PRESETS_128K,
                false,
            )
        }
        "glm-4-long" => ModelCapabilities::owned(
            1_000_000,
            Some(4_096),
            false,
            &[],
            PRESETS_1M,
            false,
        ),

        // Claude exact
        "claude-3-5-sonnet" | "claude-3-5-sonnet-20241022" | "claude-3-5-sonnet-latest" => {
            claude_200k()
        }
        "claude-3-5-haiku" | "claude-3-5-haiku-20241022" | "claude-3-5-haiku-latest" => {
            claude_200k()
        }
        "claude-3-opus" | "claude-3-opus-20240229" => claude_200k(),
        "claude-sonnet-4" | "claude-sonnet-4-20250514" | "claude-sonnet-4-5" => claude_200k(),
        "claude-opus-4" | "claude-opus-4-20250514" | "claude-opus-4-5" | "claude-opus-4-6" => {
            claude_200k()
        }
        "claude-haiku-4" | "claude-haiku-4-5" | "claude-haiku-4-5-20251001" => claude_200k(),

        // OpenAI exact
        "gpt-4o" | "gpt-4o-2024-08-06" | "gpt-4o-2024-11-20" => {
            ModelCapabilities::owned(128_000, Some(16_384), false, &[], PRESETS_128K, false)
        }
        "gpt-4o-mini" | "gpt-4o-mini-2024-07-18" => {
            ModelCapabilities::owned(128_000, Some(16_384), false, &[], PRESETS_128K, false)
        }
        "gpt-4.1" | "gpt-4.1-mini" | "gpt-4.1-nano" => {
            ModelCapabilities::owned(1_000_000, Some(32_768), false, &[], PRESETS_1M, false)
        }
        "gpt-5" | "gpt-5-mini" | "gpt-5-nano" | "gpt-5.1" | "gpt-5.2" | "gpt-5.4" => {
            openai_reasoning(400_000, PRESETS_1M)
        }
        "o1" | "o1-preview" | "o1-mini" | "o1-pro" => {
            openai_reasoning(200_000, PRESETS_200K)
        }
        "o3" | "o3-mini" | "o3-pro" => openai_reasoning(200_000, PRESETS_200K),
        "o4-mini" => openai_reasoning(200_000, PRESETS_200K),

        // Gemini exact
        "gemini-1.5-flash" | "gemini-1.5-flash-latest" => {
            ModelCapabilities::owned(1_000_000, Some(8_192), false, &[], PRESETS_GEMINI, true)
        }
        "gemini-1.5-pro" | "gemini-1.5-pro-latest" => {
            ModelCapabilities::owned(2_000_000, Some(8_192), false, &[], PRESETS_GEMINI, false)
        }
        "gemini-2.0-flash" | "gemini-2.5-flash" => {
            ModelCapabilities::owned(1_000_000, Some(65_536), true, EFFORT_STANDARD, PRESETS_GEMINI, true)
        }
        "gemini-2.5-pro" => {
            ModelCapabilities::owned(1_000_000, Some(65_536), true, EFFORT_STANDARD, PRESETS_GEMINI, false)
        }

        // Llama exact (preserve legacy heuristic for bare llama3-*)
        "llama3-8b" | "llama3-70b" | "llama-3-8b" | "llama-3-70b" => {
            ModelCapabilities::owned(8_192, Some(4_096), false, &[], PRESETS_8K, false)
        }
        "llama-3.1-8b" | "llama-3.1-70b" | "llama-3.1-405b" | "llama3.1-8b" | "llama3.1-70b" => {
            ModelCapabilities::owned(128_000, Some(4_096), false, &[], PRESETS_128K, false)
        }
        "llama-3.3-70b" => {
            ModelCapabilities::owned(128_000, Some(4_096), false, &[], PRESETS_128K, false)
        }

        _ => return None,
    };
    Some(caps)
}

fn family_match(id: &str) -> Option<ModelCapabilities> {
    // DeepSeek family
    if id.contains("deepseek") {
        if id.contains("v4") {
            let fast = id.contains("flash");
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(384_000),
                true,
                EFFORT_STANDARD,
                PRESETS_1M,
                fast,
            ));
        }
        if id.contains("reasoner") || id.contains("r1") {
            return Some(ModelCapabilities::owned(
                64_000,
                Some(8_192),
                true,
                EFFORT_STANDARD,
                PRESETS_64K,
                false,
            ));
        }
        if id.contains("flash") {
            return Some(ModelCapabilities::owned(
                128_000,
                Some(8_192),
                true,
                EFFORT_STANDARD,
                PRESETS_128K,
                true,
            ));
        }
        return Some(ModelCapabilities::owned(
            64_000,
            Some(8_192),
            false,
            &[],
            PRESETS_64K,
            false,
        ));
    }

    // NVIDIA Nemotron family (before generic llama — ids contain both)
    if id.contains("nemotron") {
        if id.contains("nemotron-3") || id.contains("nemotron_3") {
            return Some(nemotron3_1m(id.contains("nano")));
        }
        return Some(ModelCapabilities::owned(
            128_000,
            Some(16_384),
            true,
            EFFORT_STANDARD,
            PRESETS_128K,
            id.contains("nano") || id.contains("mini"),
        ));
    }

    // Moonshot Kimi
    if id.contains("kimi") || id.starts_with("moonshot") {
        if id.contains("k3") {
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(128_000),
                true,
                EFFORT_KIMI_K3,
                PRESETS_1M,
                false,
            ));
        }
        if id.contains("k2.7") || id.contains("k2-7") {
            return Some(ModelCapabilities::owned(
                256_000,
                Some(256_000),
                true,
                EFFORT_STANDARD,
                PRESETS_256K,
                id.contains("highspeed") || id.contains("turbo"),
            ));
        }
        if id.contains("thinking") {
            return Some(ModelCapabilities::owned(
                256_000,
                Some(32_768),
                true,
                EFFORT_STANDARD,
                PRESETS_256K,
                false,
            ));
        }
        if id.contains("k2.6")
            || id.contains("k2.5")
            || id.contains("k2-0905")
            || id.contains("k2-turbo")
        {
            return Some(ModelCapabilities::owned(
                256_000,
                Some(32_768),
                true,
                EFFORT_STANDARD,
                PRESETS_256K,
                false,
            ));
        }
        // kimi-k2-instruct / generic kimi
        return Some(ModelCapabilities::owned(
            128_000,
            Some(8_192),
            false,
            &[],
            PRESETS_128K,
            false,
        ));
    }

    // MiniMax
    if id.contains("minimax") || id.starts_with("m2-her") {
        if id.contains("m3") {
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(128_000),
                true,
                EFFORT_STANDARD,
                PRESETS_1M,
                false,
            ));
        }
        if id.contains("her") {
            return Some(ModelCapabilities::owned(
                64_000,
                Some(8_192),
                false,
                &[],
                PRESETS_64K,
                false,
            ));
        }
        return Some(ModelCapabilities::owned(
            204_800,
            Some(128_000),
            true,
            EFFORT_STANDARD,
            PRESETS_204K,
            id.contains("highspeed"),
        ));
    }

    // Tencent Hunyuan / Hy3
    if id.starts_with("hy3")
        || id.contains("hunyuan")
        || id.starts_with("hy-")
        || id.starts_with("hy_")
    {
        if id.starts_with("hy3") {
            return Some(ModelCapabilities::owned(
                256_000,
                Some(128_000),
                true,
                EFFORT_HY3,
                PRESETS_256K,
                false,
            ));
        }
        if id.contains("role") {
            return Some(ModelCapabilities::owned(
                32_000,
                Some(4_096),
                false,
                &[],
                PRESETS_32K,
                false,
            ));
        }
        if id.contains("t1") || id.contains("turbos") || id.contains("thinking") {
            return Some(ModelCapabilities::owned(
                128_000,
                Some(16_384),
                true,
                EFFORT_STANDARD,
                PRESETS_128K,
                false,
            ));
        }
        return Some(ModelCapabilities::owned(
            128_000,
            Some(16_384),
            false,
            &[],
            PRESETS_128K,
            false,
        ));
    }

    // Zhipu GLM / z-ai
    if id.contains("glm") {
        if id.contains("5.2") || id.contains("glm5.2") {
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(128_000),
                true,
                EFFORT_STANDARD,
                PRESETS_1M,
                false,
            ));
        }
        if id.contains("5.1")
            || id.contains("glm5.1")
            || id.contains("glm-5")
            || id.contains("glm5")
            || id.contains("4.7")
            || id.contains("glm4.7")
            || id.contains("4.6")
        {
            return Some(ModelCapabilities::owned(
                200_000,
                Some(128_000),
                true,
                EFFORT_STANDARD,
                PRESETS_200K,
                id.contains("flash") || id.contains("turbo"),
            ));
        }
        if id.contains("4-long") || id.contains("4_long") {
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(4_096),
                false,
                &[],
                PRESETS_1M,
                false,
            ));
        }
        if id.contains("4.5") || id.contains("4-flash") {
            return Some(ModelCapabilities::owned(
                128_000,
                Some(96_000),
                true,
                EFFORT_STANDARD,
                PRESETS_128K,
                id.contains("flash") || id.contains("airx"),
            ));
        }
        return Some(ModelCapabilities::owned(
            128_000,
            Some(16_384),
            false,
            &[],
            PRESETS_128K,
            false,
        ));
    }

    // Claude family
    if id.contains("claude") {
        if id.contains("opus") || id.contains("sonnet") || id.contains("haiku") {
            return Some(claude_200k());
        }
        return Some(claude_200k());
    }

    // OpenAI reasoning / GPT-5 family
    if id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") || id.contains("codex")
    {
        return Some(openai_reasoning(200_000, PRESETS_200K));
    }
    if id.starts_with("gpt-5") {
        return Some(openai_reasoning(400_000, PRESETS_1M));
    }
    if id.starts_with("gpt-4.1") {
        return Some(ModelCapabilities::owned(
            1_000_000,
            Some(32_768),
            false,
            &[],
            PRESETS_1M,
            false,
        ));
    }
    if id.contains("gpt-4o") || id.contains("gpt-4-turbo") || id.contains("gpt-4-1106")
        || id.contains("gpt-4-0125")
    {
        return Some(ModelCapabilities::owned(
            128_000,
            Some(16_384),
            false,
            &[],
            PRESETS_128K,
            false,
        ));
    }
    if id.contains("gpt-4-32k") {
        return Some(ModelCapabilities::owned(
            32_768,
            Some(8_192),
            false,
            &[],
            PRESETS_32K,
            false,
        ));
    }
    if id.contains("gpt-4") {
        return Some(ModelCapabilities::owned(
            8_192,
            Some(8_192),
            false,
            &[],
            PRESETS_8K,
            false,
        ));
    }
    if id.contains("gpt-3.5") {
        if id.contains("16k") {
            return Some(ModelCapabilities::owned(
                16_384,
                Some(4_096),
                false,
                &[],
                PRESETS_16K,
                false,
            ));
        }
        return Some(ModelCapabilities::owned(
            4_096,
            Some(4_096),
            false,
            &[],
            PRESETS_4K,
            false,
        ));
    }

    // Gemini family
    if id.contains("gemini") {
        if id.contains("pro") && (id.contains("1.5") || id.contains("2.0") || id.contains("2.5")) {
            let thinking = id.contains("2.5") || id.contains("2.0");
            return Some(ModelCapabilities::owned(
                if id.contains("1.5-pro") { 2_000_000 } else { 1_000_000 },
                Some(65_536),
                thinking,
                if thinking { EFFORT_STANDARD } else { &[] },
                PRESETS_GEMINI,
                false,
            ));
        }
        if id.contains("flash") {
            let thinking = id.contains("2.5") || id.contains("2.0");
            return Some(ModelCapabilities::owned(
                1_000_000,
                Some(8_192),
                thinking,
                if thinking { EFFORT_STANDARD } else { &[] },
                PRESETS_GEMINI,
                true,
            ));
        }
        return Some(ModelCapabilities::owned(
            1_000_000,
            Some(8_192),
            false,
            &[],
            PRESETS_GEMINI,
            false,
        ));
    }

    // Llama family
    if id.contains("llama-3.1")
        || id.contains("llama3.1")
        || id.contains("llama-3.2")
        || id.contains("llama-3.3")
        || id.contains("llama-3-")
    {
        return Some(ModelCapabilities::owned(
            128_000,
            Some(4_096),
            false,
            &[],
            PRESETS_128K,
            false,
        ));
    }
    if id.contains("llama-3") || id.contains("llama3") {
        return Some(ModelCapabilities::owned(
            8_192,
            Some(4_096),
            false,
            &[],
            PRESETS_8K,
            false,
        ));
    }
    if id.contains("llama-2") || id.contains("llama2") {
        return Some(ModelCapabilities::owned(
            4_096,
            Some(2_048),
            false,
            &[],
            PRESETS_4K,
            false,
        ));
    }

    // Qwen family
    if id.contains("qwen") {
        if id.contains("qwen3") || id.contains("qwen2.5") {
            let thinking = id.contains("think") || id.contains("reason");
            return Some(ModelCapabilities::owned(
                128_000,
                Some(8_192),
                thinking,
                if thinking { EFFORT_STANDARD } else { &[] },
                PRESETS_128K,
                id.contains("flash") || id.contains("turbo"),
            ));
        }
        return Some(ModelCapabilities::owned(
            32_768,
            Some(8_192),
            false,
            &[],
            PRESETS_32K,
            false,
        ));
    }

    // Grok
    if id.contains("grok") {
        return Some(ModelCapabilities::owned(
            256_000,
            Some(16_384),
            true,
            EFFORT_EXTENDED,
            &[128_000, 256_000],
            id.contains("fast"),
        ));
    }

    None
}

fn claude_200k() -> ModelCapabilities {
    ModelCapabilities::owned(
        200_000,
        Some(64_000),
        true,
        EFFORT_EXTENDED,
        PRESETS_200K,
        false,
    )
}

fn nemotron3_1m(supports_fast: bool) -> ModelCapabilities {
    // Nemotron 3 family: up to 1M; NIM default often 256k (docs.api.nvidia.com)
    ModelCapabilities::owned(
        1_000_000,
        Some(65_536),
        true,
        EFFORT_STANDARD,
        PRESETS_NEMOTRON3,
        supports_fast,
    )
}

fn openai_reasoning(context: usize, presets: &[usize]) -> ModelCapabilities {
    ModelCapabilities::owned(
        context,
        Some(100_000),
        true,
        EFFORT_OPENAI,
        presets,
        false,
    )
}

/// Format a token count for UI badges (`128K`, `1M`, `200K`).
pub fn format_context_label(tokens: usize) -> String {
    if tokens >= 1_000_000 && tokens % 1_000_000 == 0 {
        format!("{}M", tokens / 1_000_000)
    } else if tokens >= 1_000 {
        let k = tokens as f64 / 1_000.0;
        if (k - k.round()).abs() < 0.05 {
            format!("{}K", k.round() as usize)
        } else {
            format!("{:.1}K", k)
        }
    } else {
        tokens.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_deepseek_chat() {
        let caps = lookup_capabilities("deepseek-chat");
        assert_eq!(caps.context_tokens, 64_000);
        assert!(!caps.supports_thinking);
    }

    #[test]
    fn exact_claude_opus() {
        let caps = lookup_capabilities("claude-opus-4-5");
        assert_eq!(caps.context_tokens, 200_000);
        assert!(caps.supports_thinking);
        assert!(!caps.effort_levels.is_empty());
    }

    #[test]
    fn family_gemini_flash() {
        let caps = lookup_capabilities("gemini-1.5-flash");
        assert_eq!(caps.context_tokens, 1_000_000);
    }

    #[test]
    fn family_llama3_bare() {
        let caps = lookup_capabilities("llama3-8b");
        assert_eq!(caps.context_tokens, 8_192);
    }

    #[test]
    fn family_llama31() {
        let caps = lookup_capabilities("llama-3.1-70b");
        assert_eq!(caps.context_tokens, 128_000);
    }

    #[test]
    fn unknown_defaults() {
        let caps = lookup_capabilities("my-custom-model-xyz");
        assert_eq!(caps.context_tokens, DEFAULT_CONTEXT_TOKENS);
        assert!(!caps.supports_thinking);
        assert!(caps.effort_levels.is_empty());
    }

    #[test]
    fn resolve_respects_explicit_override() {
        assert_eq!(resolve_context_tokens("claude-opus-4", 50_000), 50_000);
        assert_eq!(resolve_context_tokens("claude-opus-4", 128_000), 200_000);
    }

    #[test]
    fn resolve_max_output() {
        assert_eq!(
            resolve_max_output_tokens("gpt-4o", None),
            Some(16_384)
        );
        assert_eq!(resolve_max_output_tokens("gpt-4o", Some(100)), Some(100));
    }

    #[test]
    fn format_labels() {
        assert_eq!(format_context_label(128_000), "128K");
        assert_eq!(format_context_label(1_000_000), "1M");
        assert_eq!(format_context_label(200_000), "200K");
    }

    #[test]
    fn case_insensitive() {
        let a = lookup_capabilities("Claude-Opus-4-5");
        let b = lookup_capabilities("claude-opus-4-5");
        assert_eq!(a.context_tokens, b.context_tokens);
    }

    #[test]
    fn nvidia_nemotron3() {
        let caps = lookup_capabilities("nvidia/nemotron-3-super-120b-a12b");
        assert_eq!(caps.context_tokens, 1_000_000);
        assert!(caps.supports_thinking);
        assert!(caps.context_presets.contains(&256_000));
    }

    #[test]
    fn kimi_k3_and_k26() {
        let k3 = lookup_capabilities("kimi-k3");
        assert_eq!(k3.context_tokens, 1_000_000);
        assert_eq!(k3.effort_levels, vec!["max".to_string()]);
        let k26 = lookup_capabilities("kimi-k2.6");
        assert_eq!(k26.context_tokens, 256_000);
        assert!(k26.supports_thinking);
    }

    #[test]
    fn minimax_m3_and_m27() {
        assert_eq!(lookup_capabilities("MiniMax-M3").context_tokens, 1_000_000);
        assert_eq!(lookup_capabilities("minimax-m2.7").context_tokens, 204_800);
        assert!(lookup_capabilities("minimax-m2.7").supports_thinking);
    }

    #[test]
    fn hunyuan_hy3() {
        let caps = lookup_capabilities("hy3-preview");
        assert_eq!(caps.context_tokens, 256_000);
        assert!(caps.supports_thinking);
        assert_eq!(caps.max_output_tokens, Some(128_000));
    }

    #[test]
    fn glm_latest() {
        assert_eq!(lookup_capabilities("glm-5.2").context_tokens, 1_000_000);
        assert_eq!(lookup_capabilities("glm-4.7").context_tokens, 200_000);
        assert!(lookup_capabilities("glm4.7").supports_thinking);
    }

    #[test]
    fn deepseek_v4_is_1m() {
        let caps = lookup_capabilities("deepseek-v4-flash");
        assert_eq!(caps.context_tokens, 1_000_000);
    }
}
