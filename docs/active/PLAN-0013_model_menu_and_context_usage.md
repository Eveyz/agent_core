# PLAN-0013 — Model capability registry, chat model menu, context usage

## Goal

Cursor-like chat model menu (thinking / effort / context presets persisted to config) plus Context Usage popover, backed by a curated `model_id` → capabilities registry (provider-agnostic).

## How to add a model to the registry

Sources (Jul 2026 — official docs, not scrape):

- NVIDIA NIM LLM list: https://docs.api.nvidia.com/nim/reference/llm-apis
- Kimi: https://platform.kimi.ai/docs/models
- MiniMax: https://platform.minimax.io/docs/guides/text-generation
- Tencent Hunyuan TokenHub: https://cloud.tencent.com/document/product/1823/130051
- Zhipu GLM: https://docs.bigmodel.cn/cn/guide/start/model-overview

Edit [`core/src/model_capabilities.rs`](../../core/src/model_capabilities.rs):

1. Prefer an **exact** entry in `exact_match` for the canonical `model_id` string users put in config.
2. Otherwise extend `family_match` with a stable prefix/family rule.
3. Mirror the same patterns in [`app/src/utils/modelCapabilities.ts`](../../app/src/utils/modelCapabilities.ts) for the chat menu UI.
4. Add a unit test in the Rust module for the new id / family.

Fields: `context_tokens`, `max_output_tokens`, `supports_thinking`, `effort_levels`, `context_presets`, `supports_fast`.

Resolution: if config `max_context_tokens` is still the default `128000`, registry wins; explicit non-default values are respected.

## Surfaces

- Chat `ModelSelector` hover submenu → `updateModelSettings` → `save_config` + `switch_model`
- Footer `ContextUsagePopover` → Tauri `get_context_usage` → `ContextEngine::usage_snapshot`
