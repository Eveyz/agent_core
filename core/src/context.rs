//! Context Engine — 7-segment semantic context assembly with token budgeting,
//! stability tracking, and per-segment refresh policies.
//!
//! Architecture:
//! ```text
//! System Prompt (single message, assembled from 7 segments)
//!
//! ┌─ Segment 1: IDENTITY ────────────────── stable, never refeshes
//! │  "You are Agverse, a Rust-native AI Agent"
//! │  Budget: 200 tokens
//! ├─ Segment 2: PRINCIPLES ──────────────── stable, refresh on mode change
//! │  Permission mode, code conventions, security boundaries, output format
//! │  Budget: 400 tokens
//! ├─ Segment 3: ENVIRONMENT ─────────────── per-turn
//! │  CWD, OS, git branch, time, workspace tree
//! │  Budget: 200 tokens
//! ├─ Segment 4: TOOL CATALOG ────────────── on-register
//! │  Available tools (name + description + DangerLevel)
//! │  Budget: dynamic (auto-truncate to budget)
//! ├─ Segment 5: ACTIVE MEMORY ───────────── query-driven
//! │  Core Memory block + Recall search results (top 3)
//! │  Budget: 600 tokens
//! ├─ Segment 6: LOADED SKILLS ───────────── on-demand
//! │  Active skill descriptions (loaded/activated)
//! │  Budget: 500 tokens
//! └─ Segment 7: EXECUTION PLAN ──────────── per-turn
//!    Current Todo list + TaskBoard ready tasks
//!    Budget: 300 tokens
//! ```

use crate::compressor::{CompressionResult, Compressor, SummarizeRequest};
use crate::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// One aggregated bucket for the Context Usage UI.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextSegmentUsage {
    /// Stable key for coloring (`system`, `tools`, `rules`, …).
    pub key: String,
    /// Human-readable label.
    pub label: String,
    pub tokens: usize,
}

/// Snapshot of context window usage for the chat Context Usage popover.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextUsageSnapshot {
    pub used_tokens: usize,
    pub max_context_tokens: usize,
    pub segments: Vec<ContextSegmentUsage>,
    pub conversation_tokens: usize,
}

impl ContextUsageSnapshot {
    pub fn empty(max_context_tokens: usize) -> Self {
        Self {
            used_tokens: 0,
            max_context_tokens,
            segments: Vec::new(),
            conversation_tokens: 0,
        }
    }

    pub fn percent_full(&self) -> f64 {
        if self.max_context_tokens == 0 {
            return 0.0;
        }
        (self.used_tokens as f64 / self.max_context_tokens as f64) * 100.0
    }
}

// ── KV Cache hints ────────────────────────────────────────────────────

/// Hints for KV cache management in local models (llama.cpp, Ollama).
#[derive(Debug, Clone)]
pub struct CacheHint {
    /// Number of tokens in the stable prefix that can be cached across turns.
    pub stable_prefix_tokens: usize,
    /// Whether the current context is suitable for KV cache reuse.
    pub can_reuse_cache: bool,
    /// The total number of tokens in the system prompt (all segments).
    pub system_prompt_tokens: usize,
    /// Segments that are part of the stable prefix.
    pub stable_segment_names: Vec<String>,
    /// Suggested KV cache strategy:
    /// - "full": entire system prompt is stable, cache everything
    /// - "partial": only the first N segments are stable
    /// - "none": system prompt changes every turn
    pub strategy: &'static str,
    /// The number of tokens that actually form a cacheable prefix across
    /// turns: frozen system prompt + conversation history (everything
    /// before the per-turn context injection message). This is the value
    /// that a KV cache engine should use for prefix reuse, as it excludes
    /// the dynamic injection that changes every turn.
    pub cacheable_prefix_tokens: usize,
    /// Milliseconds since the last turn. 0 if this is the first turn.
    /// When this exceeds the provider's idle KV-cache TTL (commonly
    /// 300-600 s for DeepSeek), the next request will be a cold miss.
    pub last_turn_elapsed_ms: u64,
    /// True when the idle gap between turns likely exceeds the provider's
    /// KV cache TTL, meaning the next request will probably be a cold miss
    /// even though the prefix is structurally cacheable.
    pub expected_cold_miss: bool,
}

impl CacheHint {
    pub fn summary(&self) -> String {
        let cold = if self.expected_cold_miss {
            " (likely cold miss)"
        } else {
            ""
        };
        format!(
            "KV Cache: {} tokens stable prefix ({}), {} tokens system, {} tokens cacheable total, strategy={}, idle={}ms{}",
            self.stable_prefix_tokens,
            self.stable_segment_names.join(", "),
            self.system_prompt_tokens,
            self.cacheable_prefix_tokens,
            self.strategy,
            self.last_turn_elapsed_ms,
            cold,
        )
    }
}

// ── Segment types ────────────────────────────────────────────────────

/// How often a segment should be rebuilt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefreshPolicy {
    /// Never changes — set once at startup.
    Never,
    /// Changes only on explicit events (e.g. permission mode switch).
    OnEvent,
    /// Rebuilt every turn before LLM call.
    PerTurn,
    /// Rebuilt when the tool registry changes.
    OnRegister,
    /// Rebuilt on-demand (e.g. skill load/unload).
    OnDemand,
}

/// Is the segment part of the stable prefix (KV cache reusable)?
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stability {
    /// Always part of the stable prefix (segments 1, 2).
    Stable,
    /// Semi-stable — changes rarely but can change (segments 3, 4).
    SemiStable,
    /// Changes every turn (segments 5, 6, 7).
    Dynamic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextTrust {
    Runtime,
    Instruction,
    UserDerived,
}

/// A single segment of the system prompt.
#[derive(Debug, Clone)]
pub struct ContextSegment {
    /// Unique name (e.g. "identity", "principles").
    pub name: String,
    /// Display label in the assembled prompt.
    pub label: String,
    /// The actual text content.
    pub content: String,
    /// Token budget for this segment. Content is truncated if it exceeds this.
    pub max_tokens: usize,
    /// Display priority (lower = closer to top).
    pub priority: u8,
    /// When this segment should be refreshed.
    pub refresh: RefreshPolicy,
    /// KV cache stability level.
    pub stability: Stability,
    /// Explicit provenance/trust class used in model-visible framing.
    pub trust: ContextTrust,
    /// Whether this segment is currently enabled.
    pub enabled: bool,
    /// Last time this segment was built (for tracking).
    pub last_built: Option<std::time::Instant>,
    /// Whether the segment is dirty and needs refresh.
    dirty: bool,
}

impl ContextSegment {
    pub fn new(
        name: &str,
        label: &str,
        priority: u8,
        max_tokens: usize,
        refresh: RefreshPolicy,
        stability: Stability,
    ) -> Self {
        let trust = match name {
            "identity" | "principles" | "skill_catalog" | "loaded_skills" => {
                ContextTrust::Instruction
            }
            "active_memory" => ContextTrust::UserDerived,
            _ => ContextTrust::Runtime,
        };
        Self {
            name: name.to_string(),
            label: label.to_string(),
            content: String::new(),
            max_tokens,
            priority,
            refresh,
            stability,
            trust,
            enabled: true,
            last_built: None,
            dirty: true,
        }
    }

    /// Set content and mark as clean.
    pub fn set_content(&mut self, content: &str) {
        self.content = content.to_string();
        self.dirty = false;
        self.last_built = Some(std::time::Instant::now());
    }

    /// Force refresh on next assembly.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    pub fn needs_refresh(&self) -> bool {
        self.dirty || matches!(self.refresh, RefreshPolicy::PerTurn)
    }

    /// Build the segment text, respecting token budget.
    pub fn assemble(&self) -> String {
        if !self.enabled || self.content.is_empty() {
            return String::new();
        }
        let truncated = truncate_segment_content(&self.name, &self.content, self.max_tokens);
        if truncated.is_empty() {
            return String::new();
        }
        format!(
            "== {} [source={}, trust={:?}] ==\n{}\n",
            self.label, self.name, self.trust, truncated
        )
    }

    /// Estimated token count of assembled output.
    pub fn token_estimate(&self) -> usize {
        if !self.enabled || self.content.is_empty() {
            return 0;
        }
        let header = format!(
            "== {} [source={}, trust={:?}] ==\n\n",
            self.label, self.name, self.trust
        );
        let header_tokens = rough_token_count(&header);
        let content_tokens = rough_token_count(&self.content);
        let body_tokens = if self.max_tokens == 0 {
            content_tokens
        } else {
            content_tokens.min(self.max_tokens)
        };
        header_tokens + body_tokens
    }
}

// ── Context Engine ────────────────────────────────────────────────────

/// The main context engine — manages semantic segments + message history.
///
/// Each turn, `assemble_system_prompt()` is called internally by `messages()`.
/// The Agent can update individual segments via the setter methods.
pub struct ContextEngine {
    segments: HashMap<String, ContextSegment>,
    messages: Vec<Message>,
    max_tokens: usize,
    tool_result_budget: usize,
    auto_compact_threshold: f64,

    /// Total token budget for the system prefix (segments 1-7 combined).
    system_prefix_budget: usize,

    /// 5-stage compression pipeline.
    pub(crate) compressor: Compressor,

    /// Track which segments belong to the stable prefix.
    stable_segment_names: Vec<String>,

    /// Timestamp of the last turn, used to detect KV cache TTL expiry.
    /// When the elapsed time exceeds the provider's idle TTL (e.g.
    /// DeepSeek ~5 min), the next request will be a cold miss.
    last_turn_timestamp: Option<std::time::Instant>,
}

impl ContextEngine {
    /// Create a new ContextEngine with default 7-segment setup.
    pub fn new(system_prompt: &str, max_tokens: usize) -> Self {
        let mut engine = Self {
            segments: HashMap::new(),
            messages: Vec::new(),
            max_tokens,
            tool_result_budget: crate::hygiene::policy::INCIDENTAL_MAX_CHARS,
            auto_compact_threshold: 0.8,
            system_prefix_budget: (max_tokens as f64 * 0.08) as usize,
            stable_segment_names: Vec::new(),
            compressor: Compressor::new(),
            last_turn_timestamp: None,
        };
        engine.init_segments(system_prompt);
        engine
    }

    /// Initialize the 8 standard segments.
    fn init_segments(&mut self, base_identity: &str) {
        // Segment 1: IDENTITY — who the agent is
        let mut identity = ContextSegment::new(
            "identity",
            "Identity",
            0,
            2_000, // P0-reviewed hard cap; default identity remains atomic
            RefreshPolicy::Never,
            Stability::Stable,
        );
        identity.set_content(base_identity);
        self.stable_segment_names.push("identity".to_string());
        self.segments.insert("identity".to_string(), identity);

        // Segment 2: PRINCIPLES — rules, conventions, boundaries
        let principles = ContextSegment::new(
            "principles",
            "Principles",
            1,
            7_000, // P0-reviewed hard cap; default protocols remain atomic
            RefreshPolicy::OnEvent,
            Stability::Stable,
        );
        self.stable_segment_names.push("principles".to_string());
        self.segments.insert("principles".to_string(), principles);

        // Segment 3: ENVIRONMENT — OS, CWD, git, time
        let env = ContextSegment::new(
            "environment",
            "Environment",
            2,
            200,
            RefreshPolicy::PerTurn,
            Stability::SemiStable,
        );
        self.segments.insert("environment".to_string(), env);

        // Segment 4: TOOL CATALOG — available tools
        // Stable: tool list doesn't change within a session, so it stays
        // in the frozen system prompt for maximum cache hits.
        let tools = ContextSegment::new(
            "tool_catalog",
            "Tool Catalog",
            3,
            0, // dynamic — uses remaining budget
            RefreshPolicy::OnRegister,
            Stability::Stable,
        );
        self.stable_segment_names.push("tool_catalog".to_string());
        self.segments.insert("tool_catalog".to_string(), tools);

        // Segment 5: ACTIVE MEMORY — core memory + recall results
        let memory = ContextSegment::new(
            "active_memory",
            "Active Memory",
            4,
            600,
            RefreshPolicy::PerTurn,
            Stability::Dynamic,
        );
        self.segments.insert("active_memory".to_string(), memory);

        // Segment 6: LOADED SKILLS — instructions are high priority, but still
        // bounded so a large/malicious skill cannot consume the whole model window.
        let skill_budget = ((self.max_tokens as f64 * 0.08) as usize).clamp(2_000, 16_000);
        let skills = ContextSegment::new(
            "loaded_skills",
            "Loaded Skills",
            6,
            skill_budget,
            RefreshPolicy::PerTurn,
            Stability::Dynamic,
        );
        self.segments.insert("loaded_skills".to_string(), skills);

        // Compact discovery index is isolated from active instructions so a
        // large catalog cannot truncate the body of an activated skill.
        let skill_catalog = ContextSegment::new(
            "skill_catalog",
            "Skill Catalog",
            7,
            1_000,
            RefreshPolicy::PerTurn,
            Stability::Dynamic,
        );
        self.segments
            .insert("skill_catalog".to_string(), skill_catalog);

        // EXECUTION PLAN — todo + task board
        let plan = ContextSegment::new(
            "execution_plan",
            "Execution Plan",
            5,
            300,
            RefreshPolicy::PerTurn,
            Stability::Dynamic,
        );
        self.segments.insert("execution_plan".to_string(), plan);
    }

    // ── Segment setters ─────────────────────────────────────────────

    /// Set the IDENTITY segment (who the agent is).
    pub fn set_identity(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("identity") {
            seg.set_content(text);
        }
    }

    /// Set the PRINCIPLES segment (rules, permissions, conventions).
    pub fn set_principles(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("principles") {
            seg.set_content(text);
        }
    }

    /// Update the ENVIRONMENT segment (OS, CWD, git, time, etc.).
    /// Called each turn.
    pub fn set_environment(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("environment") {
            seg.set_content(text);
        }
    }

    /// Update the TOOL CATALOG segment (available tools list).
    /// Called when tools are registered/unregistered.
    /// No-op if the content hasn't changed (avoids unnecessary cache invalidation).
    pub fn set_tool_catalog(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("tool_catalog") {
            if seg.content == text {
                return;
            }
            // Apply the tool catalog token budget: large catalogs get
            // truncated via the segment's existing assemble() logic.
            seg.max_tokens = self.compressor.max_tool_catalog_tokens;
            seg.set_content(text);
        }
    }

    /// Update the ACTIVE MEMORY segment.
    /// Called each turn with core memory + recall results.
    pub fn set_active_memory(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("active_memory") {
            seg.set_content(text);
        }
    }

    /// Update the LOADED SKILLS segment.
    /// Called when skills are loaded/unloaded.
    pub fn set_loaded_skills(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("loaded_skills") {
            seg.set_content(text);
        }
    }

    pub fn set_skill_catalog(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("skill_catalog") {
            seg.set_content(text);
        }
    }

    /// Update the EXECUTION PLAN segment (todo + task board).
    /// Called each turn.
    pub fn set_execution_plan(&mut self, text: &str) {
        if let Some(seg) = self.segments.get_mut("execution_plan") {
            seg.set_content(text);
        }
    }

    /// Mark a segment as needing refresh on next assembly.
    pub fn invalidate_segment(&mut self, name: &str) {
        if let Some(seg) = self.segments.get_mut(name) {
            seg.mark_dirty();
        }
    }

    /// Get a reference to a segment (for reading).
    pub fn segment(&self, name: &str) -> Option<&ContextSegment> {
        self.segments.get(name)
    }

    // ── System prompt assembly ──────────────────────────────────────

    /// Assemble the **frozen** system prompt from Stable segments only.
    /// This content never changes within a session, enabling prompt cache hits.
    pub fn assemble_system_prompt(&self) -> String {
        let mut segments: Vec<&ContextSegment> = self.segments.values().collect();
        segments.sort_by_key(|s| s.priority);

        let mut parts = Vec::new();
        let mut used_tokens = 0usize;
        // One provider message must stay below 10K tokens and leave room for
        // dynamic context on smaller models. Critical segments get priority
        // within this hard envelope; overflow is explicitly marked.
        let hard_cap = 10_000usize.min(self.max_tokens.saturating_sub(2_500).max(512));
        let critical_tokens: usize = segments
            .iter()
            .filter(|seg| matches!(seg.name.as_str(), "identity" | "principles"))
            .map(|seg| seg.token_estimate())
            .sum();
        let prefix_budget = self
            .system_prefix_budget
            .max(critical_tokens.min(hard_cap))
            .min(hard_cap);

        for seg in &segments {
            if !seg.enabled || seg.content.is_empty() {
                continue;
            }
            // Only Stable segments go into the frozen system prompt.
            if seg.stability != Stability::Stable {
                continue;
            }
            let text = seg.assemble();
            if text.is_empty() {
                continue;
            }

            let seg_tokens = rough_token_count(&text);

            let remaining = prefix_budget.saturating_sub(used_tokens);
            if remaining == 0 {
                break;
            }

            if seg_tokens > remaining {
                let header_tokens = rough_token_count(&format!(
                    "== {} [source={}, trust={:?}, truncated] ==\n\n",
                    seg.label, seg.name, seg.trust
                ));
                let body_budget = remaining.saturating_sub(header_tokens);
                if body_budget == 0 {
                    break;
                }
                let truncated = truncate_to_token_budget(&seg.content, body_budget);
                if !truncated.is_empty() {
                    parts.push(format!(
                        "== {} [source={}, trust={:?}, truncated] ==\n{}\n",
                        seg.label, seg.name, seg.trust, truncated
                    ));
                }
                break;
            }

            parts.push(text);
            used_tokens = used_tokens.saturating_add(seg_tokens);
        }

        parts.join("\n")
    }

    /// Assemble the **dynamic** context injection from non-Stable segments.
    /// This content changes every turn and is appended as a trailing System
    /// message, preserving the cacheable system + conversation prefix.
    pub fn assemble_context_injection(&self) -> String {
        let mut segments: Vec<&ContextSegment> = self.segments.values().collect();
        segments.sort_by_key(|s| s.priority);

        let mut parts = Vec::new();
        // Leave room for environment + memory + one active skill + execution
        // state even on small-context models.
        let dynamic_budget = ((self.max_tokens as f64 * 0.12) as usize).clamp(2_500, 24_000);
        let mut used_tokens = 0usize;

        for seg in &segments {
            if !seg.enabled || seg.content.is_empty() {
                continue;
            }
            // Only non-Stable segments go into the context injection.
            if seg.stability == Stability::Stable {
                continue;
            }
            let remaining = dynamic_budget.saturating_sub(used_tokens);
            if remaining == 0 {
                break;
            }
            let text = seg.assemble();
            if text.is_empty() {
                continue;
            }
            let text_tokens = rough_token_count(&text);
            if text_tokens > remaining {
                let content =
                    truncate_segment_content(&seg.name, &seg.content, remaining.saturating_sub(8));
                if !content.is_empty() {
                    parts.push(format!(
                        "== {} [source={}, trust={:?}, truncated] ==\n{}\n",
                        seg.label, seg.name, seg.trust, content
                    ));
                }
                break;
            }
            parts.push(text);
            used_tokens = used_tokens.saturating_add(text_tokens);
        }

        if parts.is_empty() {
            return String::new();
        }

        format!(
            "<context_injection>\n{}\n</context_injection>",
            parts.join("\n")
        )
    }

    /// Number of tokens in the exact stable system prefix sent to the provider.
    /// Useful for KV cache management in local models.
    pub fn stable_prefix_token_count(&self) -> usize {
        rough_token_count(&self.assemble_system_prompt())
    }

    /// Compute a fingerprint (hash) of the stable prefix segments.
    /// Returns a hex-encoded string that can be compared across turns
    /// to detect unintended drift in the cacheable prefix.
    /// Only includes segments marked as Stability::Stable to match what
    /// assemble_system_prompt() actually includes in the frozen prompt.
    pub fn stable_prefix_fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.assemble_system_prompt().hash(&mut hasher);
        format!("{:016x}", hasher.finish())
    }

    /// Verify the stable prefix hasn't drifted since the last fingerprint.
    /// Returns Ok(()) if identical, or a Vec of drifted segment names.
    pub fn verify_prefix_stability(&self, last_fingerprint: &str) -> Result<(), Vec<String>> {
        let current = self.stable_prefix_fingerprint();
        if current == last_fingerprint {
            return Ok(());
        }
        Err(self.stable_segment_names.clone())
    }

    /// Get KV cache hints for local model optimization.
    /// This tells the inference engine which parts of the prompt are
    /// stable and can be cached across turns (llama.cpp `llama_kv_cache_seq_rm`).
    pub fn cache_hint(&self) -> CacheHint {
        let stable_tokens = self.stable_prefix_token_count();
        let system_tokens = rough_token_count(&self.assemble_system_prompt());

        let strategy = if stable_tokens == 0 {
            "none"
        } else if stable_tokens >= system_tokens * 3 / 4 {
            "full"
        } else {
            "partial"
        };

        // cacheable_prefix_tokens = system (frozen) + history messages
        // (everything before the per-turn context injection). This is the
        // prefix that remains byte-stable across turns and can be reused
        // by a KV cache engine.
        let injection_tokens = rough_token_count(&self.assemble_context_injection());
        let cacheable_prefix_tokens = self.current_token_count().saturating_sub(injection_tokens);

        // TTL probe: measure idle gap between turns. Providers like
        // DeepSeek expire KV cache after ~5-10 min of inactivity.
        const COLD_MISS_THRESHOLD_MS: u64 = 300_000; // 5 minutes
        let (last_turn_elapsed_ms, expected_cold_miss) = match self.last_turn_timestamp {
            Some(ts) => {
                let elapsed = ts.elapsed().as_millis() as u64;
                (elapsed, elapsed >= COLD_MISS_THRESHOLD_MS)
            }
            None => (0, false), // first turn, no cold-miss risk
        };

        CacheHint {
            stable_prefix_tokens: stable_tokens,
            can_reuse_cache: stable_tokens > 0,
            system_prompt_tokens: system_tokens,
            stable_segment_names: self.stable_segment_names.clone(),
            strategy,
            cacheable_prefix_tokens,
            last_turn_elapsed_ms,
            expected_cold_miss,
        }
    }

    /// Record that a turn just completed, resetting the KV cache TTL timer.
    /// Call this after every LLM request so `cache_hint()` can detect
    /// idle gaps that would cause a cold cache miss.
    pub fn record_turn_timestamp(&mut self) {
        self.last_turn_timestamp = Some(std::time::Instant::now());
    }

    /// Get the raw text of the stable prefix, suitable for KV cache priming.
    /// Only includes `Stability::Stable` segments — this MUST match exactly
    /// what `assemble_system_prompt()` actually sends as the frozen system
    /// message. Including `SemiStable` segments (e.g. ENVIRONMENT, which
    /// changes every turn) here would make the priming text diverge from the
    /// real prefix and cause a KV cache miss on local models.
    pub fn stable_prefix_text(&self) -> String {
        self.assemble_system_prompt()
    }

    // ── Message management (backward compatible with old Context) ───

    pub fn set_core_memory(&mut self, memory: &str) {
        // Route to active_memory segment (backward compat)
        self.set_active_memory(memory);
    }

    pub fn set_max_tokens(&mut self, max: usize) {
        self.max_tokens = max;
    }

    pub fn set_tool_result_budget(&mut self, budget: usize) {
        self.tool_result_budget = budget;
        self.compressor.tool_result_budget = budget;
    }

    pub fn set_system_prefix_budget(&mut self, budget: usize) {
        self.system_prefix_budget = budget;
    }

    pub fn add(&mut self, msg: Message) {
        self.messages.push(msg);
    }

    pub fn clear(&mut self) {
        self.messages.clear();
    }

    /// Truncate messages to keep only the first `keep_count` messages.
    /// Returns how many were removed.
    pub fn truncate_to(&mut self, keep_count: usize) -> usize {
        let before = self.messages.len();
        if keep_count < before {
            self.messages.truncate(keep_count);
        }
        before.saturating_sub(self.messages.len())
    }

    /// Get raw messages (without system prefix). Used for rewind/context display.
    pub fn raw_messages(&self) -> &[Message] {
        &self.messages
    }

    /// Strip plaintext thinking / opaque reasoning from conversation history
    /// after a compaction boundary (chunked drop or summary).
    pub fn strip_thinking_after_compact(&mut self) {
        crate::hygiene::strip_historical_thinking(&mut self.messages);
    }

    /// Build the full message array: system (frozen) + conversation history
    /// (untouched) + dynamic context injection as a separate trailing system
    /// message.
    ///
    /// This structure maximizes prompt cache hits:
    /// - The system message is frozen (Stable segments only).
    /// - Conversation history is never modified → prefix is cacheable.
    /// - Dynamic content (environment, memory, skills, plan) is appended as
    ///   a separate system message at the end, which is always a cache miss
    ///   but does not invalidate the cacheable prefix.
    ///
    /// Keeping runtime-owned context in the system role preserves its trust
    /// provenance instead of presenting memory/skills as user-authored text.
    /// Adding it as a separate message ensures it is delivered on **every** turn,
    /// including tool-call turns where the last conversation message is a
    /// Tool result.
    pub fn messages(&self) -> Vec<Message> {
        let mut result = Vec::new();
        let system_content = self.assemble_system_prompt();
        if !system_content.is_empty() {
            result.push(Message::system(&system_content));
        }

        // Push all conversation history messages untouched.
        for msg in &self.messages {
            result.push(msg.clone());
        }

        // Append dynamic context injection as a separate trusted system message.
        let injection = self.assemble_context_injection();
        if !injection.is_empty() {
            result.push(Message::system(&injection));
        }

        result
    }

    pub fn current_token_count(&self) -> usize {
        let system_tokens = rough_token_count(&self.assemble_system_prompt());
        let injection_tokens = rough_token_count(&self.assemble_context_injection());
        let mut total = system_tokens + injection_tokens;
        for msg in &self.messages {
            total += message_token_count(msg);
        }
        total
    }

    /// Token budget for this engine (model context window).
    pub fn max_tokens(&self) -> usize {
        self.max_tokens
    }

    /// Build a UI-facing usage breakdown (aggregated segment buckets + conversation).
    pub fn usage_snapshot(&self) -> ContextUsageSnapshot {
        let seg = |name: &str| -> usize {
            self.segments
                .get(name)
                .map(|s| s.token_estimate())
                .unwrap_or(0)
        };

        let system_tokens = seg("identity") + seg("principles");
        let tools_tokens = seg("tool_catalog");
        let rules_tokens = seg("active_memory");
        let skills_tokens = seg("loaded_skills") + seg("skill_catalog");
        let plan_tokens = seg("execution_plan");
        let env_tokens = seg("environment");
        let conversation_tokens: usize = self.messages.iter().map(message_token_count).sum();

        let mut segments = Vec::new();
        let push =
            |segments: &mut Vec<ContextSegmentUsage>, key: &str, label: &str, tokens: usize| {
                if tokens > 0 {
                    segments.push(ContextSegmentUsage {
                        key: key.to_string(),
                        label: label.to_string(),
                        tokens,
                    });
                }
            };
        push(&mut segments, "system", "System prompt", system_tokens);
        push(&mut segments, "tools", "Tool definitions", tools_tokens);
        push(&mut segments, "rules", "Rules / memory", rules_tokens);
        push(&mut segments, "skills", "Skills", skills_tokens);
        push(&mut segments, "plan", "Plan", plan_tokens);
        push(&mut segments, "environment", "Environment", env_tokens);
        push(
            &mut segments,
            "conversation",
            "Conversation",
            conversation_tokens,
        );

        let used_tokens = segments.iter().map(|s| s.tokens).sum();

        ContextUsageSnapshot {
            used_tokens,
            max_context_tokens: self.max_tokens,
            segments,
            conversation_tokens,
        }
    }

    /// Run Stage 1-3 compression (snip, dedup, chunk).
    /// Does NOT drain messages — the caller is responsible for LLM
    /// summarization (Stage 4) via `prepare_summary()` / `apply_summary()`
    /// when `should_auto_compact()` returns true after this call.
    pub fn trim_to_fit(&mut self) -> CompressionResult {
        let tokens_before = self.current_token_count();

        let system_tokens = rough_token_count(&self.assemble_system_prompt());
        let injection_tokens = rough_token_count(&self.assemble_context_injection());
        let max_tokens = self.max_tokens;

        let result = self.compressor.run_pipeline(
            &mut self.messages,
            |msgs| {
                let mut total = system_tokens + injection_tokens;
                for msg in msgs {
                    total += message_token_count(msg);
                }
                total
            },
            max_tokens,
        );

        CompressionResult {
            stages_ran: result.stages_ran,
            tokens_before,
            tokens_after: self.current_token_count(),
            messages_before: result.messages_before,
            messages_after: self.messages.len(),
        }
    }

    /// Prepare a summary request for old turns (Stage 4: summaryCompact).
    /// Returns None if there aren't enough turns to summarize.
    pub fn prepare_summary(&self, num_turns: usize) -> Option<SummarizeRequest> {
        self.compressor
            .prepare_summary_compact(&self.messages, num_turns)
    }

    /// Apply a summary returned by the LLM (Stage 4).
    pub fn apply_summary(
        &mut self,
        split_idx: usize,
        summary: &crate::compressor::TurnSummary,
        num_turns: usize,
    ) -> String {
        crate::compressor::Compressor::apply_summary(
            &mut self.messages,
            split_idx,
            summary,
            num_turns,
        )
    }

    /// Apply summary delta merged with a file ledger (PLAN-0016).
    pub fn apply_summary_with_ledger(
        &mut self,
        split_idx: usize,
        summary: &crate::compressor::TurnSummary,
        ledger: &crate::runtime::FileLedger,
    ) -> String {
        crate::compressor::Compressor::apply_summary_with_ledger(
            &mut self.messages,
            split_idx,
            summary,
            Some(ledger),
        )
    }

    /// Merge file ledger into the leading RollingSummary without dropping turns.
    pub fn upsert_ledger_into_rolling_summary(
        &mut self,
        ledger: &crate::runtime::FileLedger,
    ) -> String {
        crate::compressor::Compressor::upsert_ledger_into_rolling_summary(
            &mut self.messages,
            ledger,
        )
    }

    /// Snip compact — truncate large tool results.
    /// Then auto compact — drop oldest messages if over threshold.
    /// (Legacy method, prefer `trim_to_fit()` which uses the full pipeline.)
    pub fn trim_to_fit_legacy(&mut self) {
        // Layer 1: snip compact via compressor
        self.compressor.snip_compact(&mut self.messages);

        // Layer 2: auto compact
        let current = self.current_token_count();
        let threshold = (self.max_tokens as f64 * self.auto_compact_threshold) as usize;
        if current <= threshold {
            return;
        }

        let target = current.saturating_sub(self.max_tokens);
        let mut removed_tokens = 0usize;
        let mut remove_count = 0usize;

        for msg in &self.messages {
            if removed_tokens >= target {
                break;
            }
            removed_tokens += message_token_count(msg);
            remove_count += 1;
        }

        if remove_count > 0 {
            self.messages.drain(..remove_count);
        }
    }

    /// Micro-compact: summarize old messages, keeping recent N.
    pub fn micro_compact(&mut self, keep_recent: usize) -> Option<String> {
        if self.messages.len() <= keep_recent {
            return None;
        }

        let split_point = self.messages.len() - keep_recent;
        let old_messages: Vec<&Message> = self.messages[..split_point].iter().collect();

        let mut summary_parts = Vec::new();
        for msg in &old_messages {
            if let Some(ref content) = msg.content {
                let preview = if content.len() > 200 {
                    let end = content.floor_char_boundary(200);
                    format!("{}...", &content[..end])
                } else {
                    content.clone()
                };
                summary_parts.push(format!("[{}]: {}", msg.role, preview));
            }
        }

        let summary = format!(
            "[Context summary of {} earlier messages]\n{}",
            old_messages.len(),
            summary_parts.join("\n")
        );

        self.messages.drain(..split_point);
        // Summaries contain user/tool previews and must not be promoted into
        // trusted System instructions.
        self.messages.insert(0, Message::assistant(&summary));

        Some(summary)
    }

    /// Chunked drop: batch-remove the oldest conversation turns.
    ///
    /// Keeps the `keep_recent` most recent messages and drops everything
    /// before them. Returns the number of messages dropped (0 if nothing
    /// was dropped).
    ///
    /// # Cache behavior
    ///
    /// This is the preferred compact strategy for DeepSeek-prefixed models:
    /// - The one-time drop causes a single-turn cache miss (system-only hit).
    /// - For the next 10+ turns the new shorter prefix is fully stable and
    ///   enjoys full cache hits.
    /// - Zero LLM overhead — no summarization cost, latency, or hallucination.
    ///
    /// The alternative (LLM summarization) also causes a cache miss on
    /// compaction but adds 2-5 seconds of latency, risks hallucinated
    /// summaries, and produces non-deterministic content that may
    /// destabilize subsequent cache prefixes.
    /// Drop the oldest portion of the conversation to stay within budget.
    ///
    /// Cuts at a `User` message boundary (so whole turns — including
    /// assistant↔tool pairs — stay together) and keeps the most recent
    /// `keep_recent` messages. The kept region always begins on a real user
    /// turn, which guarantees no orphaned `tool` message: a bare `tool`
    /// result with no preceding assistant `tool_calls` makes the API reject
    /// the request with a 400.
    ///
    /// If no later `User` boundary exists in the droppable range (e.g. a
    /// single long ReAct episode), the history is left untouched. Starting at
    /// an Assistant may be syntactically valid but would discard the only task
    /// instruction and silently destroy semantic continuity.
    ///
    /// Returns the number of messages removed from the front.
    ///
    /// A leading `[RollingSummary]` / legacy `[Compressed turns…]` message is
    /// preserved (PLAN-0016) so file/decision memory survives the drop.
    pub fn chunked_drop(&mut self, keep_recent: usize) -> usize {
        let original_len = self.messages.len();
        if original_len <= keep_recent {
            return 0;
        }
        let max_split_idx = original_len - keep_recent;

        let preserve_summary = self
            .messages
            .first()
            .and_then(|m| m.content.as_deref())
            .is_some_and(|c| {
                c.starts_with(crate::compressor::ROLLING_SUMMARY_PREFIX)
                    || c.starts_with("[Compressed turns")
            });
        let start = if preserve_summary { 1 } else { 0 };
        if max_split_idx <= start {
            return 0;
        }

        // Preferred: cut at the last User message at or before max_split_idx
        // so entire conversation turns remain intact.
        let drop_end = (start..=max_split_idx)
            .rev()
            .find(|&i| self.messages[i].role == crate::types::Role::User);

        let Some(drop_end) = drop_end else {
            return 0;
        };

        if drop_end > start {
            self.messages.drain(start..drop_end);
        } else {
            return 0;
        }

        let mut removed = drop_end - start;

        // Defensive: never let the kept region begin with a dangling Tool
        // message (guards against upstream compaction/summary producing a
        // structure where a tool result lost its owning assistant). Skip a
        // leading RollingSummary when checking.
        let tool_check_idx = if self
            .messages
            .first()
            .and_then(|m| m.content.as_deref())
            .is_some_and(|c| {
                c.starts_with(crate::compressor::ROLLING_SUMMARY_PREFIX)
                    || c.starts_with("[Compressed turns")
            }) {
            1
        } else {
            0
        };
        while self
            .messages
            .get(tool_check_idx)
            .map_or(false, |m| m.role == crate::types::Role::Tool)
        {
            self.messages.remove(tool_check_idx);
            removed += 1;
        }

        removed
    }

    /// Drop whole old turns at the boundary whose resulting token count is
    /// closest to `target_tokens`. The target is advisory: cuts happen only at
    /// real User boundaries and at least `min_keep_recent` messages remain.
    pub fn chunked_drop_to_target(
        &mut self,
        target_tokens: usize,
        min_keep_recent: usize,
    ) -> usize {
        let current = self.current_token_count();
        if current <= target_tokens || self.messages.len() <= min_keep_recent {
            return 0;
        }

        let message_tokens: Vec<usize> = self.messages.iter().map(message_token_count).collect();
        let conversation_tokens: usize = message_tokens.iter().sum();
        let non_conversation_tokens = current.saturating_sub(conversation_tokens);
        let preserve_summary = self
            .messages
            .first()
            .and_then(|message| message.content.as_deref())
            .is_some_and(|content| {
                content.starts_with(crate::compressor::ROLLING_SUMMARY_PREFIX)
                    || content.starts_with("[Compressed turns")
            });
        let start = usize::from(preserve_summary);
        let max_split_idx = self.messages.len().saturating_sub(min_keep_recent);
        if max_split_idx <= start {
            return 0;
        }

        let mut suffix_tokens = vec![0usize; self.messages.len() + 1];
        for index in (0..self.messages.len()).rev() {
            suffix_tokens[index] = suffix_tokens[index + 1] + message_tokens[index];
        }
        let preserved_tokens = if preserve_summary {
            message_tokens[0]
        } else {
            0
        };

        let drop_end = ((start + 1)..=max_split_idx)
            .filter(|&index| self.messages[index].role == crate::types::Role::User)
            .min_by_key(|&index| {
                let resulting_tokens =
                    non_conversation_tokens + preserved_tokens + suffix_tokens[index];
                resulting_tokens.abs_diff(target_tokens)
            });

        drop_end
            .map(|index| self.chunked_drop(self.messages.len() - index))
            .unwrap_or(0)
    }

    pub fn should_auto_compact(&self) -> bool {
        let current = self.current_token_count();
        let threshold = (self.max_tokens as f64 * self.auto_compact_threshold) as usize;
        current > threshold
    }

    pub fn len(&self) -> usize {
        self.messages.len()
    }

    pub fn is_empty(&self) -> bool {
        self.messages.is_empty()
    }

    /// Build an environment string from OS info.
    pub fn build_environment_string(
        cwd: Option<&str>,
        os_info: Option<&str>,
        git_branch: Option<&str>,
    ) -> String {
        let mut parts = Vec::new();

        if let Some(cwd) = cwd {
            parts.push(format!("Working Directory: {}", cwd));
        }

        let os = os_info.unwrap_or_else(|| {
            if cfg!(target_os = "windows") {
                "Windows"
            } else if cfg!(target_os = "linux") {
                "Linux"
            } else if cfg!(target_os = "macos") {
                "macOS"
            } else {
                "Unknown"
            }
        });
        parts.push(format!("OS: {}", os));

        if let Some(branch) = git_branch {
            parts.push(format!("Git Branch: {}", branch));
        }

        let now = chrono::Local::now();
        parts.push(format!(
            "Current Time: {}",
            now.format("%Y-%m-%d %H:%M:%S %Z")
        ));

        parts.join(" | ")
    }

    /// Build a tool catalog string from tool definitions.
    pub fn build_tool_catalog_string(
        tools: &[crate::types::ToolDefinition],
        danger_map: &HashMap<String, crate::permission::DangerLevel>,
    ) -> String {
        let mut lines = Vec::new();
        for tool in tools {
            let danger = danger_map
                .get(&tool.function.name)
                .map(|d| format!("{:?}", d))
                .unwrap_or_else(|| "ReadOnly".to_string());
            lines.push(format!(
                "  • {} [{}] — {}",
                tool.function.name, danger, tool.function.description
            ));
        }
        if lines.is_empty() {
            return "No tools available.".to_string();
        }
        format!("Available Tools:\n{}", lines.join("\n"))
    }
}

// ── Backward compatibility alias ─────────────────────────────────────

/// `Context` is now an alias for `ContextEngine`.
/// All existing code using `Context::new()`, `.add()`, `.messages()`,
/// `.trim_to_fit()`, etc. continues to work unchanged.
pub type Context = ContextEngine;

// ── Token utilities ──────────────────────────────────────────────────

/// Accurate token count using tiktoken BPE tokenizer (o200k_base).
/// Falls back to chars/4 estimate if the tokenizer fails to initialize.
pub fn rough_token_count(text: &str) -> usize {
    use std::sync::OnceLock;
    use tiktoken_rs::o200k_base;

    static BPE: OnceLock<Option<tiktoken_rs::CoreBPE>> = OnceLock::new();
    let bpe = BPE.get_or_init(|| o200k_base().ok());
    match bpe {
        Some(b) => b.encode_with_special_tokens(text).len(),
        None => {
            let chars = text.chars().count();
            let ascii_count = text.chars().filter(|c| c.is_ascii()).count();
            let non_ascii = chars - ascii_count;
            (ascii_count / 4) + (non_ascii / 2)
        }
    }
}

fn message_token_count(msg: &Message) -> usize {
    let mut count = 4;

    if let Some(ref content) = msg.content {
        count += rough_token_count(content);
    }

    if let Some(ref tool_calls) = msg.tool_calls {
        for tc in tool_calls {
            count += rough_token_count(&tc.function.name);
            count += rough_token_count(&tc.function.arguments);
            count += 10;
        }
    }

    if let Some(ref name) = msg.name {
        count += rough_token_count(name);
    }

    // Thinking / reasoning occupies context on the wire.
    // Plaintext CoT is usually already embedded in content via <think> tags
    // (wrap_thinking); only count reasoning.text when it is not duplicated.
    // Opaque blobs / signatures are never in content — always count them.
    if let Some(ref reasoning) = msg.reasoning {
        if let Some(ref text) = reasoning.text {
            let already_in_content = msg.content.as_deref().is_some_and(|c| {
                c.contains("<think>") || (!text.is_empty() && c.contains(text.as_str()))
            });
            if !already_in_content {
                count += rough_token_count(text);
            }
        }
        if let Some(ref blob) = reasoning.encrypted_content {
            count += rough_token_count(blob);
        }
        if let Some(ref sig) = reasoning.signature {
            count += rough_token_count(sig);
        }
    }

    count
}

/// Truncate text to fit within a token budget.
fn truncate_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 {
        // 0 means "no budget limit" — use whole text
        return text.to_string();
    }

    let current = rough_token_count(text);
    if current <= max_tokens {
        return text.to_string();
    }

    let marker = format!("\n[... segment truncated: budget {max_tokens} tokens]");
    let marker_tokens = rough_token_count(&marker);
    if max_tokens <= marker_tokens {
        return prefix_to_token_budget(text, max_tokens);
    }
    let truncated = prefix_to_token_budget(text, max_tokens - marker_tokens);
    format!("{truncated}{marker}")
}

fn truncate_segment_content(name: &str, text: &str, max_tokens: usize) -> String {
    match name {
        "active_memory" | "loaded_skills" | "execution_plan" => {
            truncate_head_tail_to_token_budget(text, max_tokens)
        }
        _ => truncate_to_token_budget(text, max_tokens),
    }
}

fn prefix_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || rough_token_count(text) <= max_tokens {
        return text.to_string();
    }

    let char_count = text.chars().count();
    let mut lo = 0usize;
    let mut hi = char_count;

    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        let preview: String = text.chars().take(mid).collect();
        if rough_token_count(&preview) <= max_tokens {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    text.chars().take(lo).collect()
}

fn suffix_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || rough_token_count(text) <= max_tokens {
        return text.to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let count = (lo + hi + 1) / 2;
        let candidate: String = chars[chars.len() - count..].iter().collect();
        if rough_token_count(&candidate) <= max_tokens {
            lo = count;
        } else {
            hi = count - 1;
        }
    }
    chars[chars.len() - lo..].iter().collect()
}

fn truncate_head_tail_to_token_budget(text: &str, max_tokens: usize) -> String {
    if max_tokens == 0 || rough_token_count(text) <= max_tokens {
        return text.to_string();
    }
    let marker = format!("\n[... middle truncated: budget {max_tokens} tokens ...]\n");
    let marker_tokens = rough_token_count(&marker);
    if max_tokens <= marker_tokens + 2 {
        return truncate_to_token_budget(text, max_tokens);
    }
    let body_budget = max_tokens - marker_tokens;
    let head_budget = body_budget * 2 / 3;
    let tail_budget = body_budget - head_budget;
    let head = prefix_to_token_budget(text, head_budget);
    let tail = suffix_to_token_budget(text, tail_budget);
    format!("{head}{marker}{tail}")
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    #[test]
    fn test_eight_segments_created() {
        let engine = ContextEngine::new("test identity", 128000);
        assert_eq!(engine.segments.len(), 8);
        assert!(engine.segments.contains_key("identity"));
        assert!(engine.segments.contains_key("principles"));
        assert!(engine.segments.contains_key("environment"));
        assert!(engine.segments.contains_key("tool_catalog"));
        assert!(engine.segments.contains_key("active_memory"));
        assert!(engine.segments.contains_key("loaded_skills"));
        assert!(engine.segments.contains_key("skill_catalog"));
        assert!(engine.segments.contains_key("execution_plan"));
    }

    #[test]
    fn test_identity_segment_stable() {
        let engine = ContextEngine::new("I am TestBot", 128000);
        let seg = engine.segment("identity").unwrap();
        assert_eq!(seg.refresh, RefreshPolicy::Never);
        assert_eq!(seg.stability, Stability::Stable);
        assert!(seg.content.contains("TestBot"));
    }

    #[test]
    fn test_usage_snapshot_aggregates_segments() {
        let mut engine = ContextEngine::new("I am Bot", 200_000);
        engine.set_principles("Be safe.");
        engine.set_tool_catalog("read_file — Read a file");
        engine.set_active_memory("rule: always test");
        engine.add(Message::user("hello world"));

        let snap = engine.usage_snapshot();
        assert_eq!(snap.max_context_tokens, 200_000);
        assert!(snap.used_tokens > 0);
        assert!(snap.conversation_tokens > 0);
        let keys: Vec<_> = snap.segments.iter().map(|s| s.key.as_str()).collect();
        assert!(keys.contains(&"system"));
        assert!(keys.contains(&"tools"));
        assert!(keys.contains(&"rules"));
        assert!(keys.contains(&"conversation"));
    }

    #[test]
    fn chunked_drop_to_target_keeps_the_largest_recent_window_that_fits() {
        let mut engine = ContextEngine::new("identity", 20_000);
        for i in 0..20 {
            engine.add(Message::user(&format!("task {i} {}", "u".repeat(1_000))));
            engine.add(Message::assistant(&format!(
                "answer {i} {}",
                "a".repeat(1_000)
            )));
        }

        let before = engine.current_token_count();
        let removed = engine.chunked_drop_to_target(4_000, 6);
        let after = engine.current_token_count();

        assert!(removed > 0);
        assert!(
            engine.len() > 6,
            "target selection must not collapse every long session to the fixed six-message floor"
        );
        assert!(after < before);
    }

    #[test]
    fn chunked_drop_to_target_accepts_the_closest_boundary_above_soft_target() {
        let mut engine = ContextEngine::new(&"system ".repeat(2_000), 20_000);
        for i in 0..8 {
            engine.add(Message::user(&format!("task {i} {}", "u".repeat(500))));
            engine.add(Message::assistant(&format!(
                "answer {i} {}",
                "a".repeat(500)
            )));
        }

        let removed = engine.chunked_drop_to_target(1_000, 4);

        assert!(
            removed > 0,
            "a soft target must not prevent a valid turn cut"
        );
        assert!(
            engine.current_token_count() > 1_000,
            "fixed context alone exceeds the advisory target in this fixture"
        );
    }

    #[test]
    fn usage_snapshot_counts_thinking_in_content_tags() {
        let mut engine = ContextEngine::new("Bot", 128_000);
        let visible = Message::assistant("short answer");
        let with_think = Message::assistant(
            "<think>long chain of thought about the problem space and next steps</think>\nshort answer",
        );
        engine.add(Message::user("q"));
        engine.add(visible.clone());
        let without = engine.usage_snapshot().conversation_tokens;

        engine = ContextEngine::new("Bot", 128_000);
        engine.add(Message::user("q"));
        engine.add(with_think);
        let with = engine.usage_snapshot().conversation_tokens;

        assert!(
            with > without,
            "thinking tags in content must increase conversation tokens ({with} vs {without})"
        );
    }

    #[test]
    fn usage_snapshot_counts_reasoning_text_when_not_in_content() {
        use crate::types::ReasoningState;

        let mut engine = ContextEngine::new("Bot", 128_000);
        engine.add(Message::user("q"));
        engine.add(
            Message::assistant("answer").with_reasoning(ReasoningState::from_text(
                "private reasoning that never appears in the visible content body",
            )),
        );
        let with_reasoning = engine.usage_snapshot().conversation_tokens;

        engine = ContextEngine::new("Bot", 128_000);
        engine.add(Message::user("q"));
        engine.add(Message::assistant("answer"));
        let without = engine.usage_snapshot().conversation_tokens;

        assert!(
            with_reasoning > without,
            "reasoning.text must count when content has no <think> tags"
        );
    }

    #[test]
    fn usage_snapshot_does_not_double_count_wrapped_thinking() {
        use crate::types::ReasoningState;

        let think = "same reasoning body used in both places";
        let content = format!("<think>{think}</think>\nvisible");

        let mut engine = ContextEngine::new("Bot", 128_000);
        engine.add(Message::user("q"));
        engine.add(Message::assistant(&content));
        let tags_only = engine.usage_snapshot().conversation_tokens;

        engine = ContextEngine::new("Bot", 128_000);
        engine.add(Message::user("q"));
        engine.add(Message::assistant(&content).with_reasoning(ReasoningState::from_text(think)));
        let tags_and_field = engine.usage_snapshot().conversation_tokens;

        assert_eq!(
            tags_only, tags_and_field,
            "reasoning.text duplicated in <think> tags must not double-count"
        );
    }

    #[test]
    fn test_environment_per_turn() {
        let engine = ContextEngine::new("test", 128000);
        let seg = engine.segment("environment").unwrap();
        assert_eq!(seg.refresh, RefreshPolicy::PerTurn);
        assert_eq!(seg.stability, Stability::SemiStable);
    }

    #[test]
    fn test_set_and_get_segment() {
        let mut engine = ContextEngine::new("test", 128000);
        engine.set_principles("Be helpful. Be safe.");
        let seg = engine.segment("principles").unwrap();
        assert!(seg.content.contains("Be helpful"));
        assert!(!seg.dirty);
    }

    #[test]
    fn test_assemble_system_prompt() {
        let mut engine = ContextEngine::new("I am a helper.", 128000);
        engine.set_principles("Always be honest.");
        engine.set_environment("CWD: /tmp | OS: Linux");
        engine.set_tool_catalog("  • read_file — reads a file");
        engine.set_active_memory("User prefers Rust.");
        engine.set_execution_plan("Todo: [ ] fix bug");

        // Frozen system prompt: only Stable segments (identity, principles, tool_catalog)
        let prompt = engine.assemble_system_prompt();
        assert!(prompt.contains("I am a helper"));
        assert!(prompt.contains("Always be honest"));
        assert!(prompt.contains("read_file"));
        // Dynamic segments should NOT be in the frozen system prompt
        assert!(!prompt.contains("/tmp"));
        assert!(!prompt.contains("User prefers Rust"));
        assert!(!prompt.contains("fix bug"));

        // Dynamic context injection: environment, active_memory, execution_plan
        let injection = engine.assemble_context_injection();
        assert!(injection.contains("/tmp"));
        assert!(injection.contains("User prefers Rust"));
        assert!(injection.contains("fix bug"));
        assert!(injection.contains("<context_injection>"));
    }

    #[test]
    fn test_disabled_segment_not_included() {
        let mut engine = ContextEngine::new("test", 128000);
        if let Some(seg) = engine.segments.get_mut("loaded_skills") {
            seg.enabled = false;
            seg.set_content("should not appear");
        }
        let prompt = engine.assemble_system_prompt();
        assert!(!prompt.contains("should not appear"));
    }

    #[test]
    fn default_principles_keep_all_required_protocols() {
        let mut engine = ContextEngine::new(crate::prompt::DEFAULT_IDENTITY, 128_000);
        let principles = format!(
            "{}\n\n{}",
            crate::prompt::default_principles_build(),
            crate::prompt::MEMORY_PROTOCOL
        );
        engine.set_principles(&principles);

        let prompt = engine.assemble_system_prompt();
        assert!(prompt.contains("## Clarification Protocol"));
        assert!(prompt.contains("## Planning Protocol"));
        assert!(prompt.contains("### Subagent decision rules"));
        assert!(prompt.contains("## Memory Protocol"));
        assert!(!prompt.contains("Principles (truncated)"));
    }

    #[test]
    fn stable_system_message_has_a_hard_cap_and_marks_custom_overflow() {
        let mut engine = ContextEngine::new(&"identity ".repeat(8_000), 128_000);
        engine.set_principles(&"principle ".repeat(20_000));
        engine.set_tool_catalog(&"tool ".repeat(20_000));

        let prompt = engine.assemble_system_prompt();
        assert!(rough_token_count(&prompt) <= 10_000);
        assert!(prompt.contains("truncated"));
    }

    #[test]
    fn loaded_skills_segment_is_bounded() {
        let mut engine = ContextEngine::new("test", 32_000);
        let big = "skill-body-line\n".repeat(8_000);
        engine.set_loaded_skills(&big);
        let injection = engine.assemble_context_injection();
        assert!(injection.contains("skill-body-line"));
        assert!(injection.contains("truncated"));
        assert!(rough_token_count(&injection) <= 2_700);
    }

    #[test]
    fn large_catalog_cannot_truncate_active_skill_instructions() {
        let mut engine = ContextEngine::new("test", 32_000);
        engine.set_loaded_skills("ACTIVE_SKILL_SENTINEL: follow these instructions");
        engine.set_skill_catalog(&"catalog entry ".repeat(10_000));
        let injection = engine.assemble_context_injection();
        assert!(injection.contains("ACTIVE_SKILL_SENTINEL"));
        let active_pos = injection.find("ACTIVE_SKILL_SENTINEL").unwrap();
        let catalog_pos = injection.find("Skill Catalog").unwrap();
        assert!(active_pos < catalog_pos);
    }

    #[test]
    fn active_memory_truncation_preserves_latest_recall_at_the_tail() {
        let mut engine = ContextEngine::new("identity", 128_000);
        let memory = format!(
            "{}\nLATEST_RECALL_SENTINEL",
            "old project instructions ".repeat(1_000)
        );
        engine.set_active_memory(&memory);

        let injection = engine.assemble_context_injection();
        assert!(injection.contains("old project instructions"));
        assert!(injection.contains("LATEST_RECALL_SENTINEL"));
        assert!(injection.contains("middle truncated"));
    }

    #[test]
    fn dynamic_budget_reserves_room_for_execution_plan() {
        let mut engine = ContextEngine::new("identity", 8_000);
        engine.set_environment(&"environment ".repeat(300));
        engine.set_active_memory(&"memory ".repeat(1_000));
        engine.set_loaded_skills(&"skill instructions ".repeat(2_000));
        engine.set_execution_plan("EXECUTION_PLAN_SENTINEL: continue step 3");

        let injection = engine.assemble_context_injection();
        assert!(injection.contains("EXECUTION_PLAN_SENTINEL"));
        assert!(rough_token_count(&injection) <= 2_700);
    }

    #[test]
    fn test_messages_includes_system_and_conversation() {
        let mut engine = ContextEngine::new("identity here", 128000);
        engine.add(Message::user("hello"));
        engine.add(Message::assistant("hi"));

        let msgs = engine.messages();
        assert_eq!(msgs.len(), 3); // system + user + assistant
        assert_eq!(msgs[0].role, Role::System);
        assert_eq!(msgs[1].role, Role::User);
        assert_eq!(msgs[2].role, Role::Assistant);
        assert!(msgs[0].content.as_ref().unwrap().contains("identity here"));
    }

    #[test]
    fn test_context_injection_as_separate_message() {
        let mut engine = ContextEngine::new("identity", 128000);
        engine.set_environment("CWD: /tmp");
        engine.set_active_memory("User likes Rust");
        engine.add(Message::user("first question"));
        engine.add(Message::assistant("answer"));
        engine.add(Message::user("second question"));

        let msgs = engine.messages();
        // system + 3 conversation messages + 1 injection message
        assert_eq!(msgs.len(), 5);

        // Last message is the injection (separate trusted system message)
        let injection_msg = &msgs[4];
        assert_eq!(injection_msg.role, Role::System);
        let content = injection_msg.content.as_ref().unwrap();
        assert!(content.contains("<context_injection>"));
        assert!(content.contains("User likes Rust"));

        // Conversation messages should NOT contain injection
        let last_user = &msgs[3];
        assert_eq!(last_user.role, Role::User);
        let last_content = last_user.content.as_ref().unwrap();
        assert!(last_content.contains("second question"));
        assert!(!last_content.contains("<context_injection>"));

        let first_user = &msgs[1];
        assert_eq!(first_user.role, Role::User);
        let first_content = first_user.content.as_ref().unwrap();
        assert!(first_content.contains("first question"));
        assert!(!first_content.contains("<context_injection>"));
    }

    #[test]
    fn test_context_injection_delivered_on_tool_turns() {
        let mut engine = ContextEngine::new("identity", 128000);
        engine.set_environment("CWD: /tmp");
        engine.set_active_memory("User likes Rust");
        engine.add(Message::user("do something"));
        engine.add(Message::assistant_with_tools("let me check", vec![]));
        engine.add(Message {
            role: Role::Tool,
            content: Some("tool result".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("test_tool".to_string()),
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        });

        let msgs = engine.messages();
        // system + user + assistant + tool + injection
        assert_eq!(msgs.len(), 5);

        // Injection should be present even though the last conversation
        // message is a Tool result (not a User message).
        let injection_msg = &msgs[4];
        assert_eq!(injection_msg.role, Role::System);
        let content = injection_msg.content.as_ref().unwrap();
        assert!(content.contains("<context_injection>"));
        assert!(content.contains("User likes Rust"));
    }

    #[test]
    fn test_set_core_memory_backward_compat() {
        let mut engine = ContextEngine::new("test", 128000);
        engine.set_core_memory("Remember: user likes Rust");
        let seg = engine.segment("active_memory").unwrap();
        assert!(seg.content.contains("Remember"));
    }

    #[test]
    fn test_token_truncation() {
        let long_text = "word ".repeat(1000); // ~500 tokens
        let truncated = truncate_to_token_budget(&long_text, 50);
        assert!(truncated.len() < long_text.len());
        assert!(truncated.contains("truncated"));
    }

    #[test]
    fn test_no_truncation_when_under_budget() {
        let short = "hello world";
        let result = truncate_to_token_budget(short, 100);
        assert_eq!(result, short);
    }

    #[test]
    fn test_zero_budget_means_unlimited() {
        let text = "some text here that would normally fit";
        let result = truncate_to_token_budget(text, 0);
        assert_eq!(result, text);
    }

    #[test]
    fn test_empty_segment_not_assembled() {
        let engine = ContextEngine::new("test", 128000);
        let prompt = engine.assemble_system_prompt();
        // Only identity should be in the prompt (others are empty)
        assert!(prompt.contains("test"));
        // principles, environment, etc. shouldn't appear because they're empty
        assert!(!prompt.contains("== Principles =="));
    }

    #[test]
    fn test_stable_prefix_tokens() {
        let mut engine = ContextEngine::new("short id", 128000);
        engine.set_principles("short principles");
        engine.set_environment("CWD: /tmp");
        engine.set_tool_catalog("• tool1");

        let stable_tokens = engine.stable_prefix_token_count();
        // Should be > 0 since all stable segments have content
        assert!(stable_tokens > 0);
        // Should be small since content is minimal
        assert!(stable_tokens < 100);
    }

    #[test]
    fn test_invalidate_segment() {
        let mut engine = ContextEngine::new("test", 128000);
        engine.set_principles("old principles");
        assert!(!engine.segment("principles").unwrap().dirty);

        engine.invalidate_segment("principles");
        assert!(engine.segment("principles").unwrap().dirty);
    }

    #[test]
    fn test_build_environment_string() {
        let env =
            ContextEngine::build_environment_string(Some("/home/user/project"), None, Some("main"));
        assert!(env.contains("/home/user/project"));
        assert!(env.contains("main"));
        // Windows or Linux should appear
        assert!(env.contains("Windows") || env.contains("Linux") || env.contains("macOS"));
    }

    #[test]
    fn test_build_tool_catalog() {
        let tools = vec![crate::types::ToolDefinition {
            tool_type: "function".to_string(),
            function: crate::types::FunctionSchema {
                name: "read_file".to_string(),
                description: "Read a file from disk".to_string(),
                parameters: serde_json::json!({}),
            },
        }];
        let mut danger_map = HashMap::new();
        danger_map.insert(
            "read_file".to_string(),
            crate::permission::DangerLevel::ReadOnly,
        );

        let catalog = ContextEngine::build_tool_catalog_string(&tools, &danger_map);
        assert!(catalog.contains("read_file"));
        assert!(catalog.contains("ReadOnly"));
        assert!(catalog.contains("Read a file from disk"));
    }

    #[test]
    fn test_snip_compact_truncates_large_tool_results() {
        let mut engine = ContextEngine::new("test", 128000);
        let big_result = "x".repeat(crate::hygiene::policy::INCIDENTAL_MAX_CHARS + 1024); // over incidental budget
        engine.add(Message {
            role: Role::Tool,
            content: Some(big_result.clone()),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("test_tool".to_string()),
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        });

        // Use compressor directly to test snipCompact
        engine.compressor.snip_compact(&mut engine.messages);
        let msgs = engine.messages();
        let content = msgs[1].content.as_ref().unwrap();
        assert!(content.len() < big_result.len());
        assert!(content.contains("truncated"));
    }

    #[test]
    fn test_trim_to_fit_does_not_drain_messages() {
        // trim_to_fit now only runs Stage 1-3 (snip/dedup/chunk) and does NOT
        // drain old messages. The LLM summary (Stage 4) is handled by the
        // caller via maybe_compact. This test verifies that pure user
        // messages are preserved — no data loss without an explicit compact.
        let mut engine = ContextEngine::new("test", 100);
        engine.set_max_tokens(100);

        for i in 0..20 {
            engine.add(Message::user(&format!("message number {}", i)));
        }

        engine.trim_to_fit();
        assert_eq!(engine.len(), 20); // messages preserved, no drain
    }

    #[test]
    fn test_micro_compact_keeps_recent() {
        let mut engine = ContextEngine::new("test", 128000);
        for i in 0..10 {
            engine.add(Message::user(&format!("msg {}", i)));
        }
        let summary = engine.micro_compact(3);
        assert!(summary.is_some());
        assert!(engine.len() <= 4); // summary msg + 3 kept
        assert!(summary.unwrap().contains("Context summary"));
    }

    #[test]
    fn test_chunked_drop_batch_removal() {
        let mut engine = ContextEngine::new("test", 128000);
        for i in 0..20 {
            engine.add(Message::user(&format!("msg {}", i)));
        }
        assert_eq!(engine.len(), 20);

        // Keep 5 most recent — drops first 15
        let dropped = engine.chunked_drop(5);
        assert_eq!(dropped, 15);
        assert_eq!(engine.len(), 5);
        // Verify we kept the right messages (messages() prepends system prompt)
        let msgs = engine.messages();
        // Skip system message, check range [1..]
        let user_msgs: Vec<_> = msgs.iter().skip(1).collect();
        assert_eq!(user_msgs.len(), 5);
        let first_of_kept = &user_msgs[0].content;
        assert!(first_of_kept.as_ref().unwrap().contains("msg 15"));
        let last_of_kept = &user_msgs.last().unwrap().content;
        assert!(last_of_kept.as_ref().unwrap().contains("msg 19"));
    }

    #[test]
    fn test_chunked_drop_no_op_when_under_limit() {
        let mut engine = ContextEngine::new("test", 128000);
        for i in 0..5 {
            engine.add(Message::user(&format!("msg {}", i)));
        }
        // keep_recent = 10 > len = 5 — no-op
        let dropped = engine.chunked_drop(10);
        assert_eq!(dropped, 0);
        assert_eq!(engine.len(), 5);
    }

    #[test]
    fn test_chunked_drop_avoids_orphaned_tools() {
        let mut engine = ContextEngine::new("test", 128000);

        // Turn 1: User message
        engine.add(Message::user("Hello"));
        // Turn 1: Assistant makes tool calls
        let mut assistant_msg = Message::assistant("let me write a file");
        assistant_msg.tool_calls = Some(vec![crate::types::ToolCall {
            id: "call_1".to_string(),
            call_type: "function".to_string(),
            function: crate::types::FunctionCall {
                name: "write_file".to_string(),
                arguments: "{}".to_string(),
            },
        }]);
        engine.add(assistant_msg);

        // Turn 1: Tool response
        let tool_msg = Message {
            role: Role::Tool,
            content: Some("success".to_string()),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("write_file".to_string()),
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        };
        engine.add(tool_msg);

        // Turn 2: User message
        engine.add(Message::user("Next step"));
        // Turn 2: Assistant final response
        engine.add(Message::assistant("Done"));

        // Context contains 5 messages in raw history:
        // [0] User ("Hello")
        // [1] Assistant (tool calls)
        // [2] Tool (call_1 response)
        // [3] User ("Next step")
        // [4] Assistant ("Done")

        // If we want to keep 3 messages (which would normally split at index 2, keeping [2, 3, 4], i.e., Tool, User, Assistant),
        // we should instead find the User message at/before index 2, which is index 0.
        // So it should drop 0 messages to avoid leaving Tool message at the start.
        let dropped = engine.chunked_drop(3);
        assert_eq!(dropped, 0);
        assert_eq!(engine.len(), 5);

        // If we want to keep 2 messages (which would split at index 3, which is User "Next step"),
        // index 3 is a User message, so it is safe to split. It should drop first 3 messages.
        let dropped_more = engine.chunked_drop(2);
        assert_eq!(dropped_more, 3);
        assert_eq!(engine.len(), 2);
        assert_eq!(engine.messages[0].content.as_deref().unwrap(), "Next step");
    }

    #[test]
    fn test_truncate_to() {
        let mut engine = ContextEngine::new("test", 128000);
        for i in 0..10 {
            engine.add(Message::user(&format!("msg {}", i)));
        }
        assert_eq!(engine.len(), 10);
        let removed = engine.truncate_to(5);
        assert_eq!(removed, 5);
        assert_eq!(engine.len(), 5);
    }

    #[test]
    fn test_truncate_to_beyond_len_noop() {
        let mut engine = ContextEngine::new("test", 128000);
        engine.add(Message::user("hello"));
        let removed = engine.truncate_to(10);
        assert_eq!(removed, 0); // nothing removed
        assert_eq!(engine.len(), 1);
    }

    #[test]
    fn test_raw_messages() {
        let mut engine = ContextEngine::new("test", 128000);
        engine.add(Message::user("hello"));
        engine.add(Message::assistant("hi"));
        let raw = engine.raw_messages();
        assert_eq!(raw.len(), 2);
        assert_eq!(raw[0].role, Role::User);
    }

    #[test]
    fn test_chunked_drop_never_leaves_orphan_tool() {
        // Build a history with two tool-call turns, then drop the oldest
        // half. The kept region must never begin with a bare `tool` message
        // (which would make the API reject the request with a 400), and tool
        // pairs must stay together.
        use crate::types::{FunctionCall, ToolCall};

        let mut engine = ContextEngine::new("test", 128000);
        engine.add(Message::user("task A"));
        engine.add(Message::assistant_with_tools(
            "run",
            vec![ToolCall {
                id: "c1".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "exec".into(),
                    arguments: "{}".into(),
                },
            }],
        ));
        engine.add(Message::tool(
            "c1".into(),
            "out".into(),
            Some("exec".into()),
        ));
        engine.add(Message::assistant("reply"));
        engine.add(Message::user("task B"));
        engine.add(Message::assistant_with_tools(
            "run2",
            vec![ToolCall {
                id: "c2".into(),
                call_type: "function".into(),
                function: FunctionCall {
                    name: "exec".into(),
                    arguments: "{}".into(),
                },
            }],
        ));
        engine.add(Message::tool(
            "c2".into(),
            "out2".into(),
            Some("exec".into()),
        ));
        engine.add(Message::assistant("done"));

        // 8 messages, keep 4 → cut at User "task B" (index 4), removing the
        // entire first tool turn wholesale.
        let dropped = engine.chunked_drop(4);
        assert_eq!(dropped, 4);
        assert_eq!(engine.len(), 4);
        // Kept region starts on the user turn.
        assert_eq!(engine.messages[0].content.as_deref().unwrap(), "task B");
        // The first tool turn (assistant-with-toolcalls + its tool result)
        // was removed together, so no orphaned tool message remains.
        assert!(
            !engine
                .messages
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("c1"))
        );
        // The second tool pair is fully intact in the kept region.
        assert!(
            engine
                .messages
                .iter()
                .any(|m| m.role == Role::Tool && m.tool_call_id.as_deref() == Some("c2"))
        );
    }

    #[test]
    fn chunked_drop_keeps_the_initiating_user_of_a_single_react_turn() {
        use crate::types::{FunctionCall, ToolCall};

        let mut engine = ContextEngine::new("identity", 128_000);
        engine.add(Message::user("the only task instruction"));
        for i in 0..6 {
            let id = format!("call_{i}");
            engine.add(Message::assistant_with_tools(
                "",
                vec![ToolCall {
                    id: id.clone(),
                    call_type: "function".into(),
                    function: FunctionCall {
                        name: "shell".into(),
                        arguments: "{}".into(),
                    },
                }],
            ));
            engine.add(Message::tool(id, "ok".into(), Some("shell".into())));
        }

        assert_eq!(engine.chunked_drop(4), 0);
        assert_eq!(engine.raw_messages().first().unwrap().role, Role::User);
        assert_eq!(
            engine.raw_messages().first().unwrap().content.as_deref(),
            Some("the only task instruction")
        );
    }

    #[test]
    fn test_stable_prefix_consistency_excludes_environment() {
        // P0-2: stable_prefix_text / stable_prefix_token_count must reflect
        // exactly the frozen system prompt (Stable segments only) and must
        // NOT include ENVIRONMENT (SemiStable), which lives in the injection.
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_principles("Be safe.");
        engine.set_environment("CWD: /tmp | OS: Linux | Git Branch: main");
        engine.set_tool_catalog("read_file");

        let prompt = engine.assemble_system_prompt();
        let prefix_text = engine.stable_prefix_text();

        // Both must contain the Stable segments...
        assert!(prompt.contains("I am Bot"));
        assert!(prompt.contains("Be safe."));
        assert!(prefix_text.contains("I am Bot"));
        assert!(prefix_text.contains("Be safe."));

        // ...and NEITHER must contain the SemiStable environment.
        assert!(!prompt.contains("/tmp"));
        assert!(!prefix_text.contains("/tmp"));

        assert_eq!(prefix_text, prompt);
        assert_eq!(
            engine.stable_prefix_token_count(),
            rough_token_count(&prompt)
        );
    }

    // ── P2-9: cache_hint & fingerprint tests ─────────────────────────

    #[test]
    fn test_cache_hint_computes_all_fields() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_principles("Be concise.");
        engine.set_tool_catalog("read_file");

        let hint = engine.cache_hint();
        assert!(hint.stable_prefix_tokens > 0, "must have stable prefix");
        assert!(hint.can_reuse_cache);
        // strategy is "full" or "partial" depending on stable-segment
        // token ratio; both are valid when there are Stable segments.
        assert!(hint.strategy == "full" || hint.strategy == "partial");
        assert!(hint.system_prompt_tokens > 0);
        assert!(hint.cacheable_prefix_tokens > 0);
        assert!(!hint.stable_segment_names.is_empty());
        // First turn: no idle gap.
        assert_eq!(hint.last_turn_elapsed_ms, 0);
        assert!(!hint.expected_cold_miss);
    }

    #[test]
    fn test_cache_hint_no_stable_prefix() {
        // Empty engine: only identity (which may be minimal), no principles/tools.
        let engine = ContextEngine::new("", 128000);
        let hint = engine.cache_hint();
        // May or may not have stable prefix depending on identity content.
        // But the hint must be well-formed regardless.
        assert!(hint.summary().contains("KV Cache"));
    }

    #[test]
    fn test_cache_hint_cold_miss_on_idle() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        // First turn — no cold miss.
        let h1 = engine.cache_hint();
        assert!(!h1.expected_cold_miss);

        // Simulate an idle gap by artificially setting the last turn
        // timestamp far in the past.
        engine.last_turn_timestamp =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(600));

        let h2 = engine.cache_hint();
        assert!(h2.expected_cold_miss);
        assert!(h2.last_turn_elapsed_ms >= 300_000);
    }

    #[test]
    fn test_record_turn_timestamp_resets_cold_miss() {
        let mut engine = ContextEngine::new("I am Bot", 128000);

        // First turn
        engine.record_turn_timestamp();
        let h1 = engine.cache_hint();
        assert!(!h1.expected_cold_miss);

        // Simulate idle gap
        engine.last_turn_timestamp =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(600));
        let h2 = engine.cache_hint();
        assert!(h2.expected_cold_miss);

        // New turn resets
        engine.record_turn_timestamp();
        let h3 = engine.cache_hint();
        assert!(!h3.expected_cold_miss);
    }

    #[test]
    fn test_stable_prefix_fingerprint_consistent() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_principles("Be safe.");
        engine.set_tool_catalog("toolA");

        let fp1 = engine.stable_prefix_fingerprint();
        let fp2 = engine.stable_prefix_fingerprint();
        assert_eq!(fp1, fp2, "fingerprint must be deterministic");
    }

    #[test]
    fn test_stable_prefix_fingerprint_changes_on_catalog() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_tool_catalog("toolA");
        let fp1 = engine.stable_prefix_fingerprint();

        engine.set_tool_catalog("toolA\ntoolB");
        let fp2 = engine.stable_prefix_fingerprint();
        assert_ne!(
            fp1, fp2,
            "fingerprint must change when stable content changes"
        );
    }

    #[test]
    fn test_verify_prefix_stability_detects_drift() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_tool_catalog("toolA");
        let fp = engine.stable_prefix_fingerprint();

        // Same state → no drift
        assert!(engine.verify_prefix_stability(&fp).is_ok());

        // Different state → drift detected
        engine.set_tool_catalog("toolB");
        assert!(engine.verify_prefix_stability(&fp).is_err());
    }

    #[test]
    fn test_cache_hint_cacheable_prefix_gt_stable_prefix() {
        let mut engine = ContextEngine::new("I am Bot", 128000);
        engine.set_tool_catalog("toolA");
        engine.add(Message::user("hello"));
        engine.add(Message::assistant("hi there"));

        let hint = engine.cache_hint();
        // cacheable_prefix (system + history) > stable_prefix (system only)
        assert!(
            hint.cacheable_prefix_tokens > hint.stable_prefix_tokens,
            "cacheable_prefix (system+history) must be larger than stable_prefix (segments only)"
        );
    }
}
