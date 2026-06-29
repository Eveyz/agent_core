//! Context Engine — 7-segment semantic context assembly with token budgeting,
//! stability tracking, and per-segment refresh policies.
//!
//! Architecture:
//! ```text
//! System Prompt (single message, assembled from 7 segments)
//!
//! ┌─ Segment 1: IDENTITY ────────────────── stable, never refeshes
//! │  "You are WorkBuddy, a Rust-native AI Agent"
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
use std::collections::HashMap;

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
}

impl CacheHint {
    pub fn summary(&self) -> String {
        format!(
            "KV Cache: {} tokens stable prefix ({}), {} tokens system, strategy={}",
            self.stable_prefix_tokens,
            self.stable_segment_names.join(", "),
            self.system_prompt_tokens,
            self.strategy,
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
        Self {
            name: name.to_string(),
            label: label.to_string(),
            content: String::new(),
            max_tokens,
            priority,
            refresh,
            stability,
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
        let truncated = truncate_to_token_budget(&self.content, self.max_tokens);
        if truncated.is_empty() {
            return String::new();
        }
        format!("== {} ==\n{}\n", self.label, truncated)
    }

    /// Estimated token count of assembled output.
    pub fn token_estimate(&self) -> usize {
        if !self.enabled || self.content.is_empty() {
            return 0;
        }
        let header = format!("== {} ==\n\n", self.label);
        let header_tokens = rough_token_count(&header);
        let body_tokens = rough_token_count(&self.content).min(self.max_tokens);
        header_tokens + body_tokens
    }
}

// ── Context Engine ────────────────────────────────────────────────────

/// The main context engine — manages 7 semantic segments + message history.
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
    compressor: Compressor,

    /// Track which segments belong to the stable prefix.
    stable_segment_names: Vec<String>,
}

impl ContextEngine {
    /// Create a new ContextEngine with default 7-segment setup.
    pub fn new(system_prompt: &str, max_tokens: usize) -> Self {
        let mut engine = Self {
            segments: HashMap::new(),
            messages: Vec::new(),
            max_tokens,
            tool_result_budget: 4000,
            auto_compact_threshold: 0.8,
            system_prefix_budget: (max_tokens as f64 * 0.08) as usize,
            stable_segment_names: Vec::new(),
            compressor: Compressor::new(),
        };
        engine.init_segments(system_prompt);
        engine
    }

    /// Initialize the 7 standard segments.
    fn init_segments(&mut self, base_identity: &str) {
        // Segment 1: IDENTITY — who the agent is
        let mut identity = ContextSegment::new(
            "identity",
            "Identity",
            0,
            200,
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
            400,
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
        self.stable_segment_names.push("environment".to_string());
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

        // Segment 6: LOADED SKILLS — active skill descriptions
        let skills = ContextSegment::new(
            "loaded_skills",
            "Loaded Skills",
            5,
            2000,
            RefreshPolicy::PerTurn,
            Stability::Dynamic,
        );
        self.segments.insert("loaded_skills".to_string(), skills);

        // Segment 7: EXECUTION PLAN — todo + task board
        let plan = ContextSegment::new(
            "execution_plan",
            "Execution Plan",
            6,
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

            let remaining = self.system_prefix_budget.saturating_sub(used_tokens);
            if remaining == 0 && !parts.is_empty() {
                break;
            }

            if seg_tokens > remaining && !parts.is_empty() {
                let truncated = truncate_to_token_budget(&seg.content, remaining);
                if !truncated.is_empty() {
                    parts.push(format!("== {} (truncated) ==\n{}\n", seg.label, truncated));
                }
                break;
            }

            parts.push(text);
            used_tokens = used_tokens.saturating_add(seg_tokens);
        }

        parts.join("\n")
    }

    /// Assemble the **dynamic** context injection from non-Stable segments.
    /// This content changes every turn and is injected into the last user
    /// message to preserve the cacheability of the system prompt + conversation
    /// history prefix.
    pub fn assemble_context_injection(&self) -> String {
        let mut segments: Vec<&ContextSegment> = self.segments.values().collect();
        segments.sort_by_key(|s| s.priority);

        let mut parts = Vec::new();

        for seg in &segments {
            if !seg.enabled || seg.content.is_empty() {
                continue;
            }
            // Only non-Stable segments go into the context injection.
            if seg.stability == Stability::Stable {
                continue;
            }
            let text = seg.assemble();
            if text.is_empty() {
                continue;
            }
            parts.push(text);
        }

        if parts.is_empty() {
            return String::new();
        }

        format!("<context_injection>\n{}\n</context_injection>", parts.join("\n"))
    }

    /// Number of tokens in the stable prefix (identity + principles + env + tool_catalog).
    /// Useful for KV cache management in local models.
    pub fn stable_prefix_token_count(&self) -> usize {
        let mut total = 0;
        for name in &self.stable_segment_names {
            if let Some(seg) = self.segments.get(name) {
                total += seg.token_estimate();
            }
        }
        // Add inter-segment separator tokens
        total + self.stable_segment_names.len() * 2
    }

    /// Compute a fingerprint (hash) of the stable prefix segments.
    /// Returns a hex-encoded string that can be compared across turns
    /// to detect unintended drift in the cacheable prefix.
    /// Only includes segments marked as Stability::Stable to match what
    /// assemble_system_prompt() actually includes in the frozen prompt.
    pub fn stable_prefix_fingerprint(&self) -> String {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        let mut segments: Vec<&ContextSegment> = self.segments.values().collect();
        segments.sort_by_key(|s| s.priority);
        for seg in &segments {
            if seg.enabled && !seg.content.is_empty() && seg.stability == Stability::Stable {
                seg.name.hash(&mut hasher);
                seg.content.hash(&mut hasher);
            }
        }
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

        CacheHint {
            stable_prefix_tokens: stable_tokens,
            can_reuse_cache: stable_tokens > 0,
            system_prompt_tokens: system_tokens,
            stable_segment_names: self.stable_segment_names.clone(),
            strategy,
        }
    }

    /// Get the raw text of the stable prefix, suitable for KV cache priming.
    /// Only includes segments marked as Stable or SemiStable.
    pub fn stable_prefix_text(&self) -> String {
        let mut segments: Vec<&ContextSegment> = self
            .segments
            .values()
            .filter(|s| {
                s.enabled
                    && !s.content.is_empty()
                    && matches!(s.stability, Stability::Stable | Stability::SemiStable)
            })
            .collect();
        segments.sort_by_key(|s| s.priority);

        let mut parts = Vec::new();
        for seg in segments {
            let text = seg.assemble();
            if !text.is_empty() {
                parts.push(text);
            }
        }
        parts.join("\n")
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

    /// Build the full message array: system (frozen) + conversation history
    /// with dynamic context injection appended to the last user message.
    ///
    /// This structure maximizes prompt cache hits:
    /// - The system message is frozen (Stable segments only).
    /// - Conversation history is untouched → prefix is cacheable.
    /// - Dynamic content (environment, memory, plan) is injected into the
    ///   last user message, which is always a cache miss anyway.
    pub fn messages(&self) -> Vec<Message> {
        let mut result = Vec::new();
        let system_content = self.assemble_system_prompt();
        if !system_content.is_empty() {
            result.push(Message::system(&system_content));
        }

        let injection = self.assemble_context_injection();

        let n = self.messages.len();
        for (i, msg) in self.messages.iter().enumerate() {
            let mut msg = msg.clone();
            // Inject dynamic context into the last user message.
            if i == n - 1
                && msg.role == crate::types::Role::User
                && !injection.is_empty()
            {
                if let Some(ref content) = msg.content {
                    msg.content = Some(format!("{content}\n\n{injection}"));
                } else {
                    msg.content = Some(injection.clone());
                }
            }
            result.push(msg);
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
        self.messages.insert(0, Message::system(&summary));

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
    pub fn chunked_drop(&mut self, keep_recent: usize) -> usize {
        if self.messages.len() <= keep_recent {
            return 0;
        }
        let drop_count = self.messages.len() - keep_recent;
        self.messages.drain(..drop_count);
        drop_count
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

    // Binary search for the right char boundary
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

    let truncated: String = text.chars().take(lo).collect();
    if truncated.len() < text.len() {
        format!(
            "{}\n[... segment truncated: budget {} tokens]",
            truncated, max_tokens
        )
    } else {
        truncated
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    #[test]
    fn test_seven_segments_created() {
        let engine = ContextEngine::new("test identity", 128000);
        assert_eq!(engine.segments.len(), 7);
        assert!(engine.segments.contains_key("identity"));
        assert!(engine.segments.contains_key("principles"));
        assert!(engine.segments.contains_key("environment"));
        assert!(engine.segments.contains_key("tool_catalog"));
        assert!(engine.segments.contains_key("active_memory"));
        assert!(engine.segments.contains_key("loaded_skills"));
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
    fn test_context_injection_appended_to_last_user_message() {
        let mut engine = ContextEngine::new("identity", 128000);
        engine.set_environment("CWD: /tmp");
        engine.set_active_memory("User likes Rust");
        engine.add(Message::user("first question"));
        engine.add(Message::assistant("answer"));
        engine.add(Message::user("second question"));

        let msgs = engine.messages();
        // system + 3 conversation messages
        assert_eq!(msgs.len(), 4);

        // Last user message should contain the injection
        let last_user = &msgs[3];
        assert_eq!(last_user.role, Role::User);
        let content = last_user.content.as_ref().unwrap();
        assert!(content.contains("second question"));
        assert!(content.contains("<context_injection>"));
        assert!(content.contains("User likes Rust"));

        // First user message should NOT contain injection
        let first_user = &msgs[1];
        assert_eq!(first_user.role, Role::User);
        let first_content = first_user.content.as_ref().unwrap();
        assert!(first_content.contains("first question"));
        assert!(!first_content.contains("<context_injection>"));
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
        assert!(stable_tokens < 50);
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
        let big_result = "x".repeat(5000);
        engine.add(Message {
            role: Role::Tool,
            content: Some(big_result.clone()),
            tool_calls: None,
            tool_call_id: Some("call_1".to_string()),
            name: Some("test_tool".to_string()),
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
}
