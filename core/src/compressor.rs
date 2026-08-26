//! 5-stage compression pipeline for long context windows.
//!
//! Architecture:
//! ```text
//! Stage 1: snipCompact     — truncate tool results > budget
//! Stage 2: dedupCompact    — collapse duplicate tool outputs
//! Stage 3: chunkCompact    — merge tool_call + tool_result pairs
//! Stage 4: summaryCompact  — LLM-generated structured summary
//! Stage 5: gradientCompact — age-tiered compression (recent=raw, old=summary)
//! ```
//!
//! Stages run in order: 1→2→3→4→5. Each stage reduces token count.
//! The pipeline stops early if tokens are already under threshold.

use crate::types::Message;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// Tool-result truncation (Stage 1) delegates to `hygiene::policy`, shared with
// the hygiene layer so L2 (request) and L3 (persistent history) stay identical.

// ── Compression metrics ──────────────────────────────────────────────

/// Result of running the compression pipeline.
#[derive(Debug, Clone)]
pub struct CompressionResult {
    /// Which stages ran.
    pub stages_ran: Vec<&'static str>,
    /// Tokens before compression.
    pub tokens_before: usize,
    /// Tokens after compression.
    pub tokens_after: usize,
    /// Messages before compression.
    pub messages_before: usize,
    /// Messages after compression.
    pub messages_after: usize,
}

impl CompressionResult {
    pub fn savings_percent(&self) -> f64 {
        if self.tokens_before == 0 {
            return 0.0;
        }
        (1.0 - self.tokens_after as f64 / self.tokens_before as f64) * 100.0
    }

    pub fn summary(&self) -> String {
        format!(
            "Compressed {} → {} messages ({} → {} tokens, {:.0}% saved). Stages: [{}]",
            self.messages_before,
            self.messages_after,
            self.tokens_before,
            self.tokens_after,
            self.savings_percent(),
            self.stages_ran.join(", ")
        )
    }
}

// ── Stage 4: Rolling structured summary (PLAN-0016) ─────────────────

/// Stable prefix for the single RollingSummary assistant message in the model window.
pub const ROLLING_SUMMARY_PREFIX: &str = "[RollingSummary]";

const MAX_DECISIONS: usize = 12;
const MAX_FACTS: usize = 8;
const MAX_NOTES: usize = 6;
const MAX_ERRORS: usize = 8;
const MAX_FILES_PER_BUCKET: usize = 40;

/// File paths in a rolling summary (deterministic merge + ledger).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryFiles {
    #[serde(default)]
    pub read: Vec<String>,
    #[serde(default)]
    pub wrote: Vec<String>,
    #[serde(default)]
    pub deleted: Vec<String>,
}

/// Structured rolling summary — decisions / facts / errors / files.
/// Does **not** duplicate Goal/Progress (live todo + EXECUTION STATE owns that).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnSummary {
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub decisions: Vec<String>,
    #[serde(default)]
    pub files: SummaryFiles,
    #[serde(default)]
    pub errors_open: Vec<String>,
    #[serde(default)]
    pub facts: Vec<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Alias used in docs / call sites that speak in PLAN-0016 terms.
pub type RollingSummary = TurnSummary;

impl TurnSummary {
    /// Build a context string from the summary (capped lists).
    pub fn to_context_string(&self) -> String {
        let mut parts = Vec::new();

        if let Some(goal) = self
            .goal
            .as_ref()
            .map(|g| g.trim())
            .filter(|g| !g.is_empty())
        {
            parts.push(format!("Goal: {goal}"));
        }

        if !self.decisions.is_empty() {
            parts.push(format!(
                "Decisions:\n{}",
                bullet_list(&self.decisions, MAX_DECISIONS)
            ));
        }

        let mut file_lines = Vec::new();
        if !self.files.wrote.is_empty() {
            file_lines.push(format!(
                "  wrote: {}",
                self.files
                    .wrote
                    .iter()
                    .take(MAX_FILES_PER_BUCKET)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.files.read.is_empty() {
            file_lines.push(format!(
                "  read: {}",
                self.files
                    .read
                    .iter()
                    .take(MAX_FILES_PER_BUCKET)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !self.files.deleted.is_empty() {
            file_lines.push(format!(
                "  deleted: {}",
                self.files
                    .deleted
                    .iter()
                    .take(MAX_FILES_PER_BUCKET)
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !file_lines.is_empty() {
            parts.push(format!("Files:\n{}", file_lines.join("\n")));
        }

        if !self.errors_open.is_empty() {
            parts.push(format!(
                "Open errors:\n{}",
                bullet_list(&self.errors_open, MAX_ERRORS)
            ));
        }

        if !self.facts.is_empty() {
            parts.push(format!(
                "Key facts:\n{}",
                bullet_list(&self.facts, MAX_FACTS)
            ));
        }

        if !self.notes.is_empty() {
            parts.push(format!("Notes:\n{}", bullet_list(&self.notes, MAX_NOTES)));
        }

        parts.join("\n")
    }

    /// Render as the model-window RollingSummary message body.
    pub fn to_message_content(&self) -> String {
        format!("{ROLLING_SUMMARY_PREFIX}\n{}", self.to_context_string())
    }

    /// Parse a RollingSummary (or legacy `[Compressed turns…]`) assistant message.
    pub fn parse_from_message(content: &str) -> Option<Self> {
        let trimmed = content.trim();
        if let Some(rest) = trimmed.strip_prefix(ROLLING_SUMMARY_PREFIX) {
            return Some(parse_context_string(rest.trim()));
        }
        if trimmed.starts_with("[Compressed turns") {
            // Best-effort: treat remaining body as free text notes.
            let body = trimmed.lines().skip(1).collect::<Vec<_>>().join("\n");
            if body.trim().is_empty() {
                return Some(Self::default());
            }
            return Some(Self {
                notes: vec![truncate_str(body.trim(), 500)],
                ..Self::default()
            });
        }
        None
    }

    /// Generate a prompt asking an LLM for a **delta** summary of old turns.
    /// File paths are filled deterministically from the ledger — do not invent them.
    pub fn prompt_for_llm(turns_text: &str) -> String {
        format!(
            r#"Summarize the following conversation turns into a structured JSON delta.
Be concise. Extract only what matters for continuing the work.

Rules:
- Do NOT invent Done/In Progress / todo checklists (progress lives elsewhere).
- Do NOT invent file paths (leave files empty or omit; the runtime merges a file ledger).
- Prefer short bullets. Cap each list at a handful of items.

{{
  "goal": "optional one-liner if the user goal is clear, else null",
  "decisions": ["non-obvious choices that cannot be inferred from files alone"],
  "files": {{ "read": [], "wrote": [], "deleted": [] }},
  "errors_open": ["still-open errors or problems"],
  "facts": ["important discoveries"],
  "notes": ["rare: user constraints, env quirks"]
}}

Conversation:
{turns_text}

Return ONLY valid JSON, no other text."#
        )
    }
}

fn bullet_list(items: &[String], cap: usize) -> String {
    items
        .iter()
        .take(cap)
        .map(|d| format!("  • {d}"))
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let end = crate::util::floor_char_boundary(s, max);
    format!("{}…", &s[..end])
}

/// Parse `to_context_string` output so a persisted RollingSummary round-trips
/// into structured fields (files / decisions / facts / …). Without this,
/// the next compact re-merge would drop structured file lists into freeform
/// notes and lose ledger continuity across runs.
fn parse_context_string(body: &str) -> TurnSummary {
    #[derive(Clone, Copy)]
    enum Section {
        Decisions,
        Files,
        Errors,
        Facts,
        Notes,
    }

    let mut summary = TurnSummary::default();
    let mut section: Option<Section> = None;

    for raw in body.lines() {
        let line = raw.trim_end();
        if let Some(goal) = line.strip_prefix("Goal: ") {
            let goal = goal.trim();
            if !goal.is_empty() {
                summary.goal = Some(goal.to_string());
            }
            section = None;
            continue;
        }
        match line {
            "Decisions:" => {
                section = Some(Section::Decisions);
                continue;
            }
            "Files:" => {
                section = Some(Section::Files);
                continue;
            }
            "Open errors:" => {
                section = Some(Section::Errors);
                continue;
            }
            "Key facts:" => {
                section = Some(Section::Facts);
                continue;
            }
            "Notes:" => {
                section = Some(Section::Notes);
                continue;
            }
            _ => {}
        }

        match section {
            Some(Section::Decisions) => {
                if let Some(item) = strip_bullet(line) {
                    summary.decisions.push(item);
                }
            }
            Some(Section::Files) => parse_file_bucket_line(line, &mut summary.files),
            Some(Section::Errors) => {
                if let Some(item) = strip_bullet(line) {
                    summary.errors_open.push(item);
                }
            }
            Some(Section::Facts) => {
                if let Some(item) = strip_bullet(line) {
                    summary.facts.push(item);
                }
            }
            Some(Section::Notes) => {
                if let Some(item) = strip_bullet(line) {
                    summary.notes.push(item);
                }
            }
            None => {}
        }
    }

    summary
}

fn strip_bullet(line: &str) -> Option<String> {
    let trimmed = line.trim();
    let item = trimmed
        .strip_prefix('•')
        .or_else(|| trimmed.strip_prefix("- "))
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(item.to_string())
}

fn parse_file_bucket_line(line: &str, files: &mut SummaryFiles) {
    let trimmed = line.trim();
    let (bucket, rest) = if let Some(rest) = trimmed.strip_prefix("wrote:") {
        (&mut files.wrote, rest)
    } else if let Some(rest) = trimmed.strip_prefix("read:") {
        (&mut files.read, rest)
    } else if let Some(rest) = trimmed.strip_prefix("deleted:") {
        (&mut files.deleted, rest)
    } else {
        return;
    };
    for path in rest.split(',') {
        let path = path.trim();
        if path.is_empty() {
            continue;
        }
        if !bucket.iter().any(|p| p == path) {
            bucket.push(path.to_string());
        }
    }
}

/// Merge prior rolling summary + LLM delta + deterministic file ledger.
///
/// File lists are owned by the ledger/merge logic — delta file fields are
/// unioned but ledger wins for wrote-over-read dominance.
pub fn merge_summary(
    old: &TurnSummary,
    delta: &TurnSummary,
    ledger: Option<&crate::runtime::FileLedger>,
) -> TurnSummary {
    use crate::runtime::file_ledger::merge_path_lists;

    let goal = delta
        .goal
        .as_ref()
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .map(|g| g.to_string())
        .or_else(|| old.goal.clone());

    let decisions = merge_unique_capped(&old.decisions, &delta.decisions, MAX_DECISIONS);
    let facts = merge_unique_capped(&old.facts, &delta.facts, MAX_FACTS);
    let notes = merge_unique_capped(&old.notes, &delta.notes, MAX_NOTES);
    let errors_open = merge_unique_capped(&old.errors_open, &delta.errors_open, MAX_ERRORS);

    let mut read = merge_path_lists(&old.files.read, &delta.files.read, MAX_FILES_PER_BUCKET);
    let mut wrote = merge_path_lists(&old.files.wrote, &delta.files.wrote, MAX_FILES_PER_BUCKET);
    let mut deleted = merge_path_lists(
        &old.files.deleted,
        &delta.files.deleted,
        MAX_FILES_PER_BUCKET,
    );

    if let Some(led) = ledger {
        read = merge_path_lists(&read, &led.read, MAX_FILES_PER_BUCKET);
        wrote = merge_path_lists(&wrote, &led.wrote, MAX_FILES_PER_BUCKET);
        deleted = merge_path_lists(&deleted, &led.deleted, MAX_FILES_PER_BUCKET);
    }

    // wrote dominates read; deleted removes from both.
    for d in &deleted {
        read.retain(|r| r != d);
        wrote.retain(|w| w != d);
    }
    for w in &wrote {
        read.retain(|r| r != w);
    }

    TurnSummary {
        goal,
        decisions,
        files: SummaryFiles {
            read,
            wrote,
            deleted,
        },
        errors_open,
        facts,
        notes,
    }
}

fn merge_unique_capped(a: &[String], b: &[String], cap: usize) -> Vec<String> {
    let mut out = Vec::new();
    for item in a.iter().chain(b.iter()) {
        let t = item.trim();
        if t.is_empty() {
            continue;
        }
        if out.iter().any(|x: &String| x.eq_ignore_ascii_case(t)) {
            continue;
        }
        out.push(t.to_string());
    }
    while out.len() > cap {
        out.remove(0);
    }
    out
}

/// Find an existing RollingSummary message at the front of the model window.
pub fn find_rolling_summary(messages: &[Message]) -> Option<TurnSummary> {
    messages.first().and_then(|m| {
        if m.role != crate::types::Role::Assistant {
            return None;
        }
        m.content
            .as_deref()
            .and_then(TurnSummary::parse_from_message)
    })
}

// ── Compression pipeline ─────────────────────────────────────────────

/// The compression pipeline operates on a slice/vec of messages.
pub struct Compressor {
    /// Legacy tool-result budget field. Actual truncation is driven by
    /// `hygiene::policy` (per-tool-kind budgets) since PLAN-0008; retained for
    /// API compatibility but no longer gates snipCompact.
    pub tool_result_budget: usize,
    /// Threshold ratio: if tokens > max_tokens * threshold, start compressing.
    pub auto_compact_threshold: f64,
    /// Target ratio: compress until tokens < max_tokens * target.
    pub target_ratio: f64,
    /// How many recent messages to keep raw in gradientCompact.
    pub gradient_keep_recent: usize,
    /// How many semi-recent messages get snipCompact in gradientCompact.
    pub gradient_snip_range: usize,
    /// Max token budget for the tool catalog segment in the frozen system
    /// prompt. Large tool catalogs inflate the stable prefix and increase
    /// the constant attention overhead. When set (>0), tool descriptions
    /// beyond this budget are truncated from the catalog.
    pub max_tool_catalog_tokens: usize,
}

/// Index of the latest real User task. Messages from this boundary onward are
/// the active ReAct turn and must remain structurally intact.
fn active_turn_start(messages: &[Message]) -> usize {
    messages
        .iter()
        .rposition(|message| message.role == crate::types::Role::User)
        .unwrap_or(messages.len())
}

impl Default for Compressor {
    fn default() -> Self {
        Self {
            tool_result_budget: crate::hygiene::policy::INCIDENTAL_MAX_CHARS,
            auto_compact_threshold: 0.8,
            target_ratio: 0.2,
            gradient_keep_recent: 6,
            gradient_snip_range: 6,
            max_tool_catalog_tokens: 2000,
        }
    }
}

impl Compressor {
    pub fn new() -> Self {
        Self::default()
    }

    // ── Stage 1: snipCompact ────────────────────────────────────────

    /// Truncate tool results exceeding the budget.
    /// Delegates to the shared `hygiene::policy` so behaviour matches the
    /// hygiene layer (L2 == L3). See PLAN-0008.
    /// Returns the number of messages modified.
    pub fn snip_compact(&self, messages: &mut Vec<Message>) -> usize {
        let mut modified = 0;
        let active_start = active_turn_start(messages);
        for msg in messages.iter_mut().take(active_start) {
            if msg.role == crate::types::Role::Tool
                && let Some(ref content) = msg.content
                && let Some(truncated) =
                    crate::hygiene::policy::truncate_content(msg.name.as_deref(), content)
            {
                msg.content = Some(truncated);
                modified += 1;
            }
        }
        modified
    }

    // ── Stage 2: dedupCompact ───────────────────────────────────────

    /// Detect consecutive identical tool results and replace with references.
    /// E.g., three identical `read_file("main.rs")` → keep first, reference others.
    /// Returns the number of messages deduplicated.
    pub fn dedup_compact(&self, messages: &mut Vec<Message>) -> usize {
        let mut deduped = 0;
        let mut seen: HashMap<String, (usize, String)> = HashMap::new(); // content_hash → (msg_index, tool_name)
        let active_start = active_turn_start(messages);

        for i in 0..active_start {
            let (content, tool_name) = {
                let msg = &messages[i];
                if msg.role != crate::types::Role::Tool {
                    continue;
                }
                let content = match &msg.content {
                    Some(c) => c,
                    None => continue,
                };
                let name = msg.name.clone().unwrap_or_default();
                (content.clone(), name)
            };

            // Skip very short results (< 50 chars) — not worth deduping
            if content.len() < 50 {
                continue;
            }

            let hash_key = format!("{}::{}", tool_name, content);

            if let Some((first_idx, first_name)) = seen.get(&hash_key) {
                let truncated = truncate_preview(&content, 200);
                messages[i].content = Some(format!(
                    "[Same as #{} — {}... ({} chars)]",
                    first_idx + 1,
                    truncated,
                    content.len()
                ));
                let _ = first_name;
                deduped += 1;
            } else {
                seen.insert(hash_key, (i, tool_name));
            }
        }

        deduped
    }

    // ── Stage 3: chunkCompact ───────────────────────────────────────

    /// Merge consecutive tool_call → tool_result pairs into assistant-owned summaries.
    /// Only operates before the latest User boundary, preserving the entire
    /// active ReAct turn regardless of how many tool calls it contains.
    /// Returns the number of pairs merged.
    pub fn chunk_compact(&self, messages: &mut Vec<Message>) -> usize {
        let mut limit = active_turn_start(messages);
        let mut merged = 0;
        let mut i = 0;

        while i + 1 < limit {
            let (tool_name, tool_args, call_id) = {
                let msg = &messages[i];
                if msg.role != crate::types::Role::Assistant {
                    i += 1;
                    continue;
                }
                match &msg.tool_calls {
                    Some(calls) if calls.len() == 1 => {
                        let call = &calls[0];
                        (
                            call.function.name.clone(),
                            call.function.arguments.clone(),
                            call.id.clone(),
                        )
                    }
                    _ => {
                        i += 1;
                        continue;
                    }
                }
            };

            let next = &messages[i + 1];
            if next.role == crate::types::Role::Tool
                && next.tool_call_id.as_deref() == Some(&call_id)
            {
                let result = next.content.clone().unwrap_or_default();
                let result_preview = truncate_preview(&result, 500);

                // Tool output is untrusted and must not become a System message.
                let chunk = format!(
                    "[Tool: {} | Args: {}]\n→ Result: {}",
                    tool_name,
                    truncate_preview(&tool_args, 200),
                    result_preview
                );
                messages[i] = Message::assistant(&chunk);

                // Remove the tool result message
                messages.remove(i + 1);
                limit -= 1;
                merged += 1;
            }

            i += 1;
        }

        merged
    }

    // ── Stage 4: summaryCompact ─────────────────────────────────────

    /// Build text suitable for LLM summarization from old messages.
    /// Does NOT call the LLM — returns the text and indices to remove.
    /// The caller (Agent) calls the LLM and then replaces messages.
    pub fn prepare_summary_compact(
        &self,
        messages: &[Message],
        num_turns_to_summarize: usize,
    ) -> Option<SummarizeRequest> {
        if num_turns_to_summarize == 0 || messages.is_empty() {
            return None;
        }

        // Find the split point: each User message starts a new turn
        let mut turn_boundaries = Vec::new();
        for (i, msg) in messages.iter().enumerate() {
            if msg.role == crate::types::Role::User {
                turn_boundaries.push(i);
            }
        }

        if turn_boundaries.len() <= num_turns_to_summarize {
            return None; // not enough turns
        }

        let split_idx = turn_boundaries[num_turns_to_summarize];
        let old_slice = &messages[..split_idx];

        // Build a text representation of old turns
        let mut turns_text = String::new();
        let mut turn_num = 0;
        for msg in old_slice {
            match msg.role {
                crate::types::Role::User => {
                    turn_num += 1;
                    turns_text.push_str(&format!(
                        "\n--- Turn {} (User) ---\n{}\n",
                        turn_num,
                        msg.content.as_deref().unwrap_or("")
                    ));
                }
                crate::types::Role::Assistant => {
                    if let Some(ref content) = msg.content {
                        let visible = crate::hygiene::strip_thinking_in_content(content);
                        if !visible.is_empty() {
                            turns_text.push_str(&format!("[Assistant]: {}\n", visible));
                        }
                    }
                    if let Some(ref calls) = msg.tool_calls {
                        for call in calls {
                            turns_text.push_str(&format!("[Tool call: {}]\n", call.function.name));
                        }
                    }
                }
                crate::types::Role::Tool => {
                    let preview = msg
                        .content
                        .as_deref()
                        .map(|c| truncate_preview(c, 300))
                        .unwrap_or_default();
                    let name = msg.name.as_deref().unwrap_or("unknown");
                    turns_text.push_str(&format!("[Tool result: {}] {}\n", name, preview));
                }
                _ => {}
            }
        }

        let prompt = TurnSummary::prompt_for_llm(&turns_text);

        Some(SummarizeRequest {
            turns_text,
            turns_to_remove: num_turns_to_summarize,
            split_index: split_idx,
            prompt,
        })
    }

    /// Apply a summary delta returned by the LLM, merging into any existing
    /// RollingSummary at the front of the window (PLAN-0016).
    /// Historical thinking is stripped without touching the retained active turn.
    pub fn apply_summary(
        messages: &mut Vec<Message>,
        split_idx: usize,
        summary: &TurnSummary,
        _num_turns: usize,
    ) -> String {
        Self::apply_summary_with_ledger(messages, split_idx, summary, None)
    }

    /// Like [`apply_summary`], merging an optional file ledger into the files block.
    pub fn apply_summary_with_ledger(
        messages: &mut Vec<Message>,
        split_idx: usize,
        delta: &TurnSummary,
        ledger: Option<&crate::runtime::FileLedger>,
    ) -> String {
        let prior = messages
            .first()
            .and_then(|m| m.content.as_deref())
            .and_then(TurnSummary::parse_from_message)
            .unwrap_or_default();

        let merged = merge_summary(&prior, delta, ledger);
        let summary_text = merged.to_message_content();

        messages.drain(..split_idx);
        if messages
            .first()
            .and_then(|m| m.content.as_deref())
            .is_some_and(|c| {
                c.starts_with(ROLLING_SUMMARY_PREFIX) || c.starts_with("[Compressed turns")
            })
        {
            messages.remove(0);
        }
        messages.insert(0, Message::assistant(&summary_text));
        crate::hygiene::strip_historical_thinking(messages);

        summary_text
    }

    /// Ensure a RollingSummary exists at the front and merge `ledger` into it
    /// without dropping conversation turns (used before chunked_drop).
    pub fn upsert_ledger_into_rolling_summary(
        messages: &mut Vec<Message>,
        ledger: &crate::runtime::FileLedger,
    ) -> String {
        let prior = find_rolling_summary(messages).unwrap_or_default();
        let merged = merge_summary(&prior, &TurnSummary::default(), Some(ledger));
        let text = merged.to_message_content();
        if messages
            .first()
            .and_then(|m| m.content.as_deref())
            .is_some_and(|c| {
                c.starts_with(ROLLING_SUMMARY_PREFIX) || c.starts_with("[Compressed turns")
            })
        {
            messages[0] = Message::assistant(&text);
        } else {
            messages.insert(0, Message::assistant(&text));
        }
        text
    }

    // ── Stage 1-3 pipeline ──────────────────────────────────────────

    /// Run Stage 1-3 (snip, dedup, chunk) in order. These are purely
    /// deterministic — no LLM required. Stage 4 (LLM summary) is handled
    /// externally by the caller via `prepare_summary_compact` / `apply_summary`.
    ///
    /// Returns a CompressionResult with stats.
    pub fn run_stages_1_3(
        &mut self,
        messages: &mut Vec<Message>,
        token_counter: impl Fn(&[Message]) -> usize,
    ) -> CompressionResult {
        let tokens_before = token_counter(messages);
        let msgs_before = messages.len();
        let mut stages = Vec::new();

        let snipped = self.snip_compact(messages);
        if snipped > 0 {
            stages.push("snipCompact");
        }

        let deduped = self.dedup_compact(messages);
        if deduped > 0 {
            stages.push("dedupCompact");
        }

        let chunked = self.chunk_compact(messages);
        if chunked > 0 {
            stages.push("chunkCompact");
        }

        CompressionResult {
            stages_ran: stages,
            tokens_before,
            tokens_after: token_counter(messages),
            messages_before: msgs_before,
            messages_after: messages.len(),
        }
    }

    /// Run the full pipeline (stages 1-3 + gradient). Stages 4-5 require LLM.
    pub fn run_pipeline(
        &mut self,
        messages: &mut Vec<Message>,
        token_counter: impl Fn(&[Message]) -> usize,
        max_tokens: usize,
    ) -> CompressionResult {
        let current = token_counter(messages);
        let threshold = (max_tokens as f64 * self.auto_compact_threshold) as usize;

        if current <= threshold {
            return CompressionResult {
                stages_ran: vec![],
                tokens_before: current,
                tokens_after: current,
                messages_before: messages.len(),
                messages_after: messages.len(),
            };
        }

        self.run_stages_1_3(messages, token_counter)
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

fn truncate_preview(text: &str, max_chars: usize) -> String {
    if text.len() <= max_chars {
        return text.to_string();
    }
    let end = text.floor_char_boundary(max_chars);
    format!("{}...", &text[..end])
}

/// Request for LLM-based summarization.
#[derive(Debug, Clone)]
pub struct SummarizeRequest {
    /// Text representation of the turns to summarize.
    pub turns_text: String,
    /// Number of turns being summarized.
    pub turns_to_remove: usize,
    /// Index in the messages vec where to split.
    pub split_index: usize,
    /// Prompt to send to the LLM.
    pub prompt: String,
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{FunctionCall, Role, ToolCall};

    fn make_tool_call_msg(id: &str, name: &str, args: &str) -> Message {
        Message {
            role: Role::Assistant,
            content: Some("Let me use a tool.".to_string()),
            tool_calls: Some(vec![ToolCall {
                id: id.to_string(),
                call_type: "function".to_string(),
                function: FunctionCall {
                    name: name.to_string(),
                    arguments: args.to_string(),
                },
            }]),
            tool_call_id: None,
            name: None,
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    fn make_tool_result(id: &str, name: &str, content: &str) -> Message {
        Message {
            role: Role::Tool,
            content: Some(content.to_string()),
            tool_calls: None,
            tool_call_id: Some(id.to_string()),
            name: Some(name.to_string()),
            model: None,
            metadata: None,
            reasoning: None,
            images: None,
        }
    }

    #[test]
    fn test_snip_compact_truncates_long_results() {
        let comp = Compressor::new();
        // Incidental tool (exec) over the dual budget → truncated.
        let oversized = "x".repeat(crate::hygiene::policy::INCIDENTAL_MAX_CHARS + 1024);
        let mut msgs = vec![make_tool_result("c1", "exec", &oversized)];
        let modified = comp.snip_compact(&mut msgs);
        assert_eq!(modified, 1);
        let content = msgs[0].content.as_ref().unwrap();
        assert!(content.len() < oversized.len());
        assert!(content.contains("truncated"));
    }

    #[test]
    fn test_snip_compact_skips_short_results() {
        let comp = Compressor::new();
        let mut msgs = vec![make_tool_result("c1", "exec", "short result")];
        let modified = comp.snip_compact(&mut msgs);
        assert_eq!(modified, 0);
    }

    #[test]
    fn test_dedup_compact_collapses_duplicates() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_result("c1", "read_file", &"A".repeat(100)),
            make_tool_result("c2", "read_file", &"A".repeat(100)), // same!
            make_tool_result("c3", "read_file", &"B".repeat(100)), // different
        ];
        let deduped = comp.dedup_compact(&mut msgs);
        assert_eq!(deduped, 1);
        assert!(msgs[1].content.as_ref().unwrap().contains("Same as"));
        assert_eq!(msgs[2].content.as_deref().unwrap(), &"B".repeat(100));
    }

    #[test]
    fn test_dedup_compact_skips_short() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_result("c1", "read_file", "hi"),
            make_tool_result("c2", "read_file", "hi"),
        ];
        let deduped = comp.dedup_compact(&mut msgs);
        assert_eq!(deduped, 0); // too short to dedup
    }

    #[test]
    fn test_chunk_compact_merges_pairs() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_call_msg("c1", "read_file", r#"{"path":"main.rs"}"#),
            make_tool_result("c1", "read_file", "fn main() {}"),
            // padding so protect_recent (8) doesn't cover the pair above
            Message::user("padding 1"),
            Message::assistant("padding 2"),
            Message::user("padding 3"),
            Message::assistant("padding 4"),
            Message::user("padding 5"),
            Message::assistant("padding 6"),
            Message::user("padding 7"),
            Message::assistant("padding 8"),
        ];
        let merged = comp.chunk_compact(&mut msgs);
        assert_eq!(merged, 1);
        assert_eq!(msgs[0].role, crate::types::Role::Assistant);
        let content = msgs[0].content.as_ref().unwrap();
        assert!(content.contains("read_file"));
        assert!(content.contains("fn main()"));
    }

    #[test]
    fn test_chunk_compact_skips_non_matching_ids() {
        let comp = Compressor::new();
        let mut msgs = vec![
            make_tool_call_msg("c1", "read_file", r#"{"path":"a.rs"}"#),
            make_tool_result("c2", "read_file", "content"), // different id
        ];
        let merged = comp.chunk_compact(&mut msgs);
        assert_eq!(merged, 0);
        assert_eq!(msgs.len(), 2);
    }

    #[test]
    fn active_react_turn_is_never_compacted() {
        let mut comp = Compressor::default();
        let mut msgs = vec![Message::user("single long task")];
        for i in 0..6 {
            let id = format!("active_{i}");
            msgs.push(make_tool_call_msg(&id, "exec", "{}"));
            msgs.push(make_tool_result(
                &id,
                "exec",
                &format!("unique-{i}-{}", "x".repeat(20_000)),
            ));
        }
        let result = comp.run_stages_1_3(&mut msgs, |messages| messages.len());

        assert!(result.stages_ran.is_empty());
        assert_eq!(msgs.len(), 13);
        for i in 0..6 {
            assert_eq!(
                msgs[1 + i * 2].tool_calls.as_ref().unwrap()[0].id,
                format!("active_{i}")
            );
            assert!(
                msgs[2 + i * 2]
                    .content
                    .as_ref()
                    .unwrap()
                    .contains(&format!("unique-{i}"))
            );
        }
    }

    #[test]
    fn test_prepare_summary_compact() {
        let comp = Compressor::new();
        let msgs = vec![
            Message::user("task 1"),
            Message::assistant("working on it"),
            make_tool_call_msg("c1", "read_file", "{}"),
            make_tool_result("c1", "read_file", "data"),
            Message::user("task 2"),
            Message::assistant("done"),
        ];

        let req = comp.prepare_summary_compact(&msgs, 1);
        assert!(req.is_some());
        let req = req.unwrap();
        assert!(req.prompt.contains("task 1"));
        assert!(req.prompt.contains("read_file"));
        assert_eq!(req.split_index, 4); // second User message starts at index 4
    }

    #[test]
    fn test_prepare_summary_compact_not_enough_turns() {
        let comp = Compressor::new();
        let msgs = vec![Message::user("hi"), Message::assistant("hello")];
        let req = comp.prepare_summary_compact(&msgs, 3);
        assert!(req.is_none());
    }

    #[test]
    fn test_apply_summary() {
        let active = Message::assistant("done2").with_reasoning(crate::types::ReasoningState {
            text: Some("active reasoning".into()),
            encrypted_content: Some("active blob".into()),
            signature: None,
            summary: None,
        });
        let mut msgs = vec![
            Message::user("task1"),
            Message::assistant("done1"),
            Message::user("task2"),
            active,
        ];

        let summary = TurnSummary {
            decisions: vec!["decided to refactor".to_string()],
            files: SummaryFiles {
                wrote: vec!["src/main.rs".to_string()],
                ..Default::default()
            },
            facts: vec!["codebase uses async".to_string()],
            ..Default::default()
        };

        let text = Compressor::apply_summary(&mut msgs, 2, &summary, 1);
        assert_eq!(msgs.len(), 3); // summary + task2 + done2
        assert_eq!(msgs[0].role, crate::types::Role::Assistant);
        assert_eq!(
            msgs[2]
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.encrypted_content.as_deref()),
            Some("active blob")
        );
        assert!(
            msgs[0]
                .content
                .as_ref()
                .unwrap()
                .contains("decided to refactor")
        );
        assert!(text.contains(ROLLING_SUMMARY_PREFIX));
    }

    #[test]
    fn test_run_stages_1_3() {
        let mut comp = Compressor::default();

        let mut msgs = vec![
            Message::user("old task"),
            Message::assistant("working"),
            make_tool_call_msg("c1", "read_file", "{}"),
            // Incidental tool over the dual budget so snipCompact engages.
            make_tool_result(
                "c1",
                "exec",
                &"x".repeat(crate::hygiene::policy::INCIDENTAL_MAX_CHARS + 1024),
            ),
            Message::user("recent task"),
            Message::assistant("done"),
        ];

        let result = comp.run_stages_1_3(&mut msgs, |m| m.len() * 100);
        assert!(result.stages_ran.contains(&"snipCompact"));
    }

    #[test]
    fn test_compression_result_savings() {
        let result = CompressionResult {
            stages_ran: vec!["snipCompact", "chunkCompact"],
            tokens_before: 1000,
            tokens_after: 400,
            messages_before: 10,
            messages_after: 5,
        };
        assert!((result.savings_percent() - 60.0).abs() < 0.01);
        assert!(result.summary().contains("60%"));
    }

    #[test]
    fn test_turn_summary_to_context_string() {
        let summary = TurnSummary {
            decisions: vec!["use async/await".to_string()],
            files: SummaryFiles {
                wrote: vec!["main.rs".to_string(), "lib.rs".to_string()],
                ..Default::default()
            },
            errors_open: vec!["tokio runtime panic".to_string()],
            facts: vec!["Rust 2024 edition".to_string()],
            notes: vec!["need to test edge case".to_string()],
            ..Default::default()
        };

        let s = summary.to_context_string();
        assert!(s.contains("async/await"));
        assert!(s.contains("main.rs, lib.rs"));
        assert!(s.contains("tokio runtime"));
        assert!(s.contains("edge case"));
        assert!(s.contains("Rust 2024"));
    }

    #[test]
    fn test_prompt_for_llm() {
        let prompt = TurnSummary::prompt_for_llm("User: hello\nAssistant: hi");
        assert!(prompt.contains("decisions"));
        assert!(prompt.contains("hello"));
        assert!(prompt.contains("JSON"));
        assert!(prompt.contains("Do NOT invent Done/In Progress"));
    }

    #[test]
    fn rolling_summary_round_trips_structured_fields() {
        let original = TurnSummary {
            goal: Some("ship compact fixes".into()),
            decisions: vec!["prefer chunked_drop".into(), "clamp max_output".into()],
            files: SummaryFiles {
                read: vec!["a.rs".into(), "b.rs".into()],
                wrote: vec!["compact.rs".into()],
                deleted: vec!["tmp.log".into()],
            },
            errors_open: vec!["flaky test".into()],
            facts: vec!["threshold uses usable budget".into()],
            notes: vec!["keep summary parse honest".into()],
        };
        let rendered = original.to_message_content();
        let parsed = TurnSummary::parse_from_message(&rendered).expect("parse");
        assert_eq!(parsed, original);
    }

    #[test]
    fn upsert_after_parse_keeps_prior_files_when_ledger_emptyish() {
        // Simulate restore: only the rendered message exists; re-parse then
        // merge a new ledger write without wiping prior structured files.
        let prior = TurnSummary {
            files: SummaryFiles {
                wrote: vec!["old.rs".into()],
                read: vec!["docs.md".into()],
                ..Default::default()
            },
            decisions: vec!["keep dual-track".into()],
            ..Default::default()
        };
        let content = prior.to_message_content();
        let mut messages = vec![Message::assistant(&content)];
        let mut ledger = crate::runtime::FileLedger::new();
        ledger.record_wrote("new.rs");
        Compressor::upsert_ledger_into_rolling_summary(&mut messages, &ledger);
        let merged =
            TurnSummary::parse_from_message(messages[0].content.as_deref().expect("content"))
                .expect("parse");
        assert!(merged.files.wrote.iter().any(|p| p == "old.rs"));
        assert!(merged.files.wrote.iter().any(|p| p == "new.rs"));
        assert!(merged.files.read.iter().any(|p| p == "docs.md"));
        assert!(merged.decisions.iter().any(|d| d == "keep dual-track"));
    }

    #[test]
    fn test_merge_summary_caps_and_wrote_over_read() {
        let old = TurnSummary {
            decisions: vec!["a".into()],
            files: SummaryFiles {
                read: vec!["a.rs".into()],
                ..Default::default()
            },
            facts: (0..10).map(|i| format!("f{i}")).collect(),
            ..Default::default()
        };
        let delta = TurnSummary {
            decisions: vec!["b".into()],
            files: SummaryFiles {
                wrote: vec!["a.rs".into()],
                ..Default::default()
            },
            facts: vec!["new".into()],
            ..Default::default()
        };
        let mut ledger = crate::runtime::FileLedger::new();
        ledger.record_read("b.rs");
        ledger.record_wrote("c.rs");

        let merged = merge_summary(&old, &delta, Some(&ledger));
        assert!(merged.decisions.contains(&"a".into()));
        assert!(merged.decisions.contains(&"b".into()));
        assert!(!merged.files.read.iter().any(|p| p == "a.rs"));
        assert!(merged.files.wrote.iter().any(|p| p == "a.rs"));
        assert!(merged.files.wrote.iter().any(|p| p == "c.rs"));
        assert!(merged.files.read.iter().any(|p| p == "b.rs"));
        assert!(merged.facts.len() <= MAX_FACTS);
    }

    #[test]
    fn test_merge_summary_bounded_across_three_merges() {
        let mut s = TurnSummary::default();
        for i in 0..3 {
            let delta = TurnSummary {
                decisions: vec![format!("d{i}")],
                facts: (0..5).map(|j| format!("f{i}-{j}")).collect(),
                ..Default::default()
            };
            s = merge_summary(&s, &delta, None);
        }
        assert!(s.decisions.len() <= MAX_DECISIONS);
        assert!(s.facts.len() <= MAX_FACTS);
        assert!(s.decisions.contains(&"d2".into()));
    }
}
