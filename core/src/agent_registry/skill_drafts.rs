//! Experimental (PLAN-0009 Phase 6): agent skill-draft generation.
//!
//! Analyzes an agent's execution history to detect recurring patterns
//! (repeated tasks, common failure modes) and generates SKILL.md drafts.
//! Drafts are written to a `drafts/` directory and require explicit human
//! approval before being promoted to the live skills directory.

use anyhow::{Context, Result};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::memory::storage::Storage;
use crate::agent_registry::history::{list as history_list, AgentHistoryEntry};

/// A generated skill draft awaiting human review.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDraft {
    /// Slugified name (used as the file name).
    pub name: String,
    pub description: String,
    /// Why this draft was generated (grounded in execution history).
    pub rationale: String,
    /// The SKILL.md body content.
    pub body: String,
    /// Triggers that would activate this skill.
    pub triggers: Vec<String>,
    /// The agent id this draft was generated for.
    pub agent_id: String,
    /// Number of history entries analyzed.
    pub samples_analyzed: usize,
    pub generated_at: String,
}

/// Result of generating skill drafts for an agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DraftGenerationResult {
    pub agent_id: String,
    pub drafts: Vec<SkillDraft>,
    pub samples_analyzed: usize,
}

/// Analyze an agent's execution history and generate skill drafts.
///
/// This is a **heuristic** generator (no LLM call): it looks for:
/// 1. Repeated task patterns (common keywords in inputs)
/// 2. Recurring failure patterns (failed executions with similar inputs)
/// 3. High-iteration executions (may benefit from a skill that shortcuts them)
///
/// Drafts are written to `drafts_dir` but NOT activated — they require
/// explicit approval via [`approve_draft`].
pub fn generate_drafts(
    storage: &Storage,
    agent_id: &str,
    drafts_dir: &Path,
    limit: usize,
) -> Result<DraftGenerationResult> {
    let history = history_list(storage, agent_id, limit)?;
    let drafts = analyze_and_generate(&history, agent_id);
    let samples = history.len();

    // Write drafts to the drafts directory.
    std::fs::create_dir_all(drafts_dir)
        .with_context(|| format!("failed to create drafts dir: {:?}", drafts_dir))?;

    let now = Utc::now().to_rfc3339();
    for draft in &drafts {
        let path = drafts_dir.join(format!("{}.md", draft.name));
        let content = format_draft_md(draft, &now);
        std::fs::write(&path, content)
            .with_context(|| format!("failed to write draft: {:?}", path))?;
    }

    Ok(DraftGenerationResult {
        agent_id: agent_id.to_string(),
        drafts,
        samples_analyzed: samples,
    })
}

/// List all skill drafts in a directory.
pub fn list_drafts(drafts_dir: &Path) -> Result<Vec<SkillDraft>> {
    if !drafts_dir.exists() {
        return Ok(Vec::new());
    }
    let mut drafts = Vec::new();
    for entry in std::fs::read_dir(drafts_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        if let Ok(draft) = parse_draft_md(&path) {
            drafts.push(draft);
        }
    }
    drafts.sort_by(|a, b| b.generated_at.cmp(&a.generated_at));
    Ok(drafts)
}

/// Get a single draft by name.
pub fn get_draft(drafts_dir: &Path, name: &str) -> Result<SkillDraft> {
    let path = drafts_dir.join(format!("{name}.md"));
    parse_draft_md(&path).with_context(|| format!("draft '{name}' not found"))
}

/// Approve a draft: move it from `drafts_dir` to `skills_dir`.
pub fn approve_draft(
    drafts_dir: &Path,
    skills_dir: &Path,
    name: &str,
) -> Result<()> {
    let src = drafts_dir.join(format!("{name}.md"));
    if !src.exists() {
        anyhow::bail!("draft '{name}' not found");
    }
    std::fs::create_dir_all(skills_dir)?;
    let dst = skills_dir.join(format!("{name}.md"));
    std::fs::rename(&src, &dst)
        .with_context(|| format!("failed to move draft to skills dir: {:?} -> {:?}", src, dst))?;
    Ok(())
}

/// Reject a draft: delete it from `drafts_dir`.
pub fn reject_draft(drafts_dir: &Path, name: &str) -> Result<()> {
    let path = drafts_dir.join(format!("{name}.md"));
    if !path.exists() {
        anyhow::bail!("draft '{name}' not found");
    }
    std::fs::remove_file(&path)
        .with_context(|| format!("failed to delete draft: {:?}", path))?;
    Ok(())
}

// ── Internal: heuristic analysis ────────────────────────────────────

fn analyze_and_generate(history: &[AgentHistoryEntry], agent_id: &str) -> Vec<SkillDraft> {
    if history.len() < 3 {
        // Not enough data to detect patterns.
        return Vec::new();
    }

    let mut drafts = Vec::new();

    // Pattern 1: Recurring failure patterns.
    let failures: Vec<&AgentHistoryEntry> = history.iter().filter(|h| !h.success).collect();
    if failures.len() >= 2 {
        let common = extract_common_keywords(&failures.iter().map(|h| h.input.as_str()).collect::<Vec<_>>());
        if !common.is_empty() {
            let name = format!("{}-failure-recovery", slugify(agent_id));
            drafts.push(SkillDraft {
                name,
                description: format!(
                    "Recovery strategies for recurring {} failures",
                    common.join(", ")
                ),
                rationale: format!(
                    "Agent '{}' had {} failed executions. Common failure keywords: {}. \
                     This draft provides recovery guidance to reduce failure rates.",
                    agent_id,
                    failures.len(),
                    common.join(", ")
                ),
                body: format_failure_recovery_body(&common),
                triggers: common.iter().take(3).cloned().collect(),
                agent_id: agent_id.to_string(),
                samples_analyzed: history.len(),
                generated_at: Utc::now().to_rfc3339(),
            });
        }
    }

    // Pattern 2: High-iteration executions (may benefit from a shortcut skill).
    let high_iter: Vec<&AgentHistoryEntry> = history
        .iter()
        .filter(|h| h.iterations_used >= 10)
        .collect();
    if high_iter.len() >= 2 {
        let avg_iters = high_iter.iter().map(|h| h.iterations_used).sum::<u32>() as f64
            / high_iter.len() as f64;
        let name = format!("{}-efficiency", slugify(agent_id));
        drafts.push(SkillDraft {
            name,
            description: format!(
                "Efficiency shortcuts for {} (avg {} iterations per run)",
                agent_id, avg_iters as u32
            ),
            rationale: format!(
                "Agent '{}' had {} executions with 10+ iterations (avg {}). \
                 A skill with direct approaches may reduce iteration count.",
                agent_id,
                high_iter.len(),
                avg_iters as u32
            ),
            body: format_efficiency_body(agent_id, avg_iters),
            triggers: vec![],
            agent_id: agent_id.to_string(),
            samples_analyzed: history.len(),
            generated_at: Utc::now().to_rfc3339(),
        });
    }

    // Pattern 3: Repeated task types (common input keywords across all history).
    let all_inputs: Vec<&str> = history.iter().map(|h| h.input.as_str()).collect();
    let common_tasks = extract_common_keywords(&all_inputs);
    if common_tasks.len() >= 2 {
        let name = format!("{}-task-patterns", slugify(agent_id));
        drafts.push(SkillDraft {
            name,
            description: format!("Common task patterns for {}", agent_id),
            rationale: format!(
                "Agent '{}' executed {} tasks. Common task keywords detected: {}. \
                 A skill documenting these patterns may improve consistency.",
                agent_id,
                history.len(),
                common_tasks.join(", ")
            ),
            body: format_task_patterns_body(&common_tasks),
            triggers: common_tasks.iter().take(3).cloned().collect(),
            agent_id: agent_id.to_string(),
            samples_analyzed: history.len(),
            generated_at: Utc::now().to_rfc3339(),
        });
    }

    drafts
}

/// Extract common keywords from a list of text inputs.
fn extract_common_keywords(texts: &[&str]) -> Vec<String> {
    use std::collections::HashMap;
    let mut word_counts: HashMap<String, usize> = HashMap::new();
    let stop_words: &[&str] = &[
        "the", "a", "an", "to", "and", "or", "in", "on", "at", "for", "of", "is",
        "are", "was", "were", "be", "been", "this", "that", "it", "with", "as",
        "by", "from", "please", "can", "you", "i", "me", "my", "we", "do", "task",
    ];

    for text in texts {
        for word in text.split_whitespace() {
            let lower = word
                .to_lowercase()
                .trim_matches(|c: char| !c.is_alphanumeric())
                .to_string();
            if lower.len() < 3 || stop_words.contains(&lower.as_str()) {
                continue;
            }
            *word_counts.entry(lower).or_insert(0) += 1;
        }
    }

    let mut sorted: Vec<(String, usize)> = word_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));
    sorted
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .take(5)
        .map(|(word, _)| word)
        .collect()
}

fn slugify(s: &str) -> String {
    s.replace(|c: char| !c.is_alphanumeric() && c != '-', "-")
        .to_lowercase()
        .trim_matches('-')
        .to_string()
}

fn format_draft_md(draft: &SkillDraft, now: &str) -> String {
    format!(
        "---\n\
         name: {name}\n\
         description: {desc}\n\
         version: \"draft\"\n\
         generated_by: agent-history-analyzer\n\
         generated_at: {ts}\n\
         agent_id: {agent_id}\n\
         samples_analyzed: {samples}\n\
         triggers: [{triggers}]\n\
         status: draft\n\
         priority: 5\n\
         ---\n\
         \n\
         # Skill: {name}\n\
         \n\
         > **Draft — requires human review before activation.**\n\
         > {rationale}\n\
         \n\
         {body}\n",
        name = draft.name,
        desc = draft.description.replace('"', ""),
        ts = now,
        agent_id = draft.agent_id,
        samples = draft.samples_analyzed,
        triggers = draft.triggers.join(", "),
        rationale = draft.rationale,
        body = draft.body,
    )
}

fn parse_draft_md(path: &Path) -> Result<SkillDraft> {
    let content = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read draft: {:?}", path))?;

    // Parse frontmatter (between --- markers)
    let mut lines = content.lines();
    let first = lines.next().unwrap_or("");
    if first.trim() != "---" {
        anyhow::bail!("invalid frontmatter");
    }

    let mut frontmatter: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for line in &mut lines {
        let line = line.trim();
        if line == "---" {
            break;
        }
        if let Some((key, val)) = line.split_once(':') {
            frontmatter.insert(key.trim().to_string(), val.trim().to_string());
        }
    }

    let body: String = lines.collect::<Vec<_>>().join("\n").trim().to_string();
    let triggers: Vec<String> = frontmatter
        .get("triggers")
        .map(|s| {
            s.trim_matches(|c: char| c == '[' || c == ']')
                .split(',')
                .map(|t| t.trim().to_string())
                .filter(|t| !t.is_empty())
                .collect()
        })
        .unwrap_or_default();

    let name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();

    Ok(SkillDraft {
        name,
        description: frontmatter.get("description").cloned().unwrap_or_default(),
        rationale: frontmatter
            .get("rationale")
            .cloned()
            .unwrap_or_default(),
        body,
        triggers,
        agent_id: frontmatter.get("agent_id").cloned().unwrap_or_default(),
        samples_analyzed: frontmatter
            .get("samples_analyzed")
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(0),
        generated_at: frontmatter
            .get("generated_at")
            .cloned()
            .unwrap_or_default(),
    })
}

fn format_failure_recovery_body(keywords: &[String]) -> String {
    format!(
        "## Failure Recovery Strategies\n\n\
         This agent has experienced recurring failures related to: {}.\n\n\
         ### Recovery Guidelines\n\
         1. When encountering errors related to the above, check for common pitfalls first.\n\
         2. Break down complex operations into smaller, verifiable steps.\n\
         3. If a tool call fails, retry with adjusted parameters before giving up.\n\
         4. Log the failure context for future reference.\n\n\
         ### Common Pitfalls to Avoid\n\
         - Assuming file paths exist without checking.\n\
         - Not validating input before processing.\n\
         - Ignoring partial failures in multi-step operations.\n",
        keywords.join(", ")
    )
}

fn format_efficiency_body(agent_id: &str, avg_iters: f64) -> String {
    format!(
        "## Efficiency Shortcuts\n\n\
         Agent '{}' regularly uses 10+ iterations (avg {:.0}).\n\
         This skill provides direct approaches to reduce iteration count.\n\n\
         ### Direct Approaches\n\
         1. For file operations, batch read/edit instead of one-at-a-time.\n\
         2. For searches, use specific patterns rather than broad exploration.\n\
         3. For code changes, plan the full change set before executing.\n\
         4. For analysis, structure the output format upfront.\n",
        agent_id, avg_iters
    )
}

fn format_task_patterns_body(keywords: &[String]) -> String {
    format!(
        "## Common Task Patterns\n\n\
         This agent frequently handles tasks related to: {}.\n\n\
         ### Recommended Approach\n\
         1. Identify the task type early from the input.\n\
         2. Apply the standard workflow for that task type.\n\
         3. Use consistent output formatting.\n\
         4. Validate results before returning.\n",
        keywords.join(", ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::storage::Storage;
    use crate::agent_registry::history::{record, AgentHistoryEntry};
    use tempfile::TempDir;

    fn make_storage() -> Storage {
        let dir = TempDir::new().unwrap();
        let storage = Storage::new(dir.path().join("test.db").to_str().unwrap()).unwrap();
        // Create a dummy agent so the FK constraint on agent_history is satisfied.
        let agent = crate::agent_registry::AgentDef {
            id: "a1".to_string(),
            name: "Test Agent".to_string(),
            ..Default::default()
        };
        crate::agent_registry::create(&storage, &agent).unwrap();
        storage
    }

    fn make_entry(agent_id: &str, input: &str, success: bool, iters: u32) -> AgentHistoryEntry {
        AgentHistoryEntry {
            agent_id: agent_id.to_string(),
            input: input.to_string(),
            success,
            iterations_used: iters,
            ..Default::default()
        }
    }

    #[test]
    fn generate_drafts_insufficient_data() {
        let storage = make_storage();
        let dir = TempDir::new().unwrap();
        // Only 2 entries — need at least 3.
        record(&storage, &make_entry("a1", "test input", true, 5)).unwrap();
        record(&storage, &make_entry("a1", "test input", true, 5)).unwrap();
        let result = generate_drafts(&storage, "a1", dir.path(), 50).unwrap();
        assert_eq!(result.samples_analyzed, 2);
        assert!(result.drafts.is_empty());
    }

    #[test]
    fn generate_drafts_detects_failures() {
        let storage = make_storage();
        let dir = TempDir::new().unwrap();
        for _ in 0..3 {
            record(&storage, &make_entry("a1", "deploy the service", false, 5)).unwrap();
        }
        record(&storage, &make_entry("a1", "normal task", true, 3)).unwrap();
        let result = generate_drafts(&storage, "a1", dir.path(), 50).unwrap();
        assert!(result.drafts.iter().any(|d| d.name.contains("failure")));
        // Draft file should exist on disk.
        assert!(dir.path().read_dir().unwrap().count() > 0);
    }

    #[test]
    fn generate_drafts_detects_high_iterations() {
        let storage = make_storage();
        let dir = TempDir::new().unwrap();
        for _ in 0..3 {
            record(&storage, &make_entry("a1", "complex refactoring task code", true, 15)).unwrap();
        }
        let result = generate_drafts(&storage, "a1", dir.path(), 50).unwrap();
        assert!(result.drafts.iter().any(|d| d.name.contains("efficiency")));
    }

    #[test]
    fn list_drafts_empty_dir() {
        let dir = TempDir::new().unwrap();
        let drafts = list_drafts(dir.path()).unwrap();
        assert!(drafts.is_empty());
    }

    #[test]
    fn list_drafts_nonexistent_dir() {
        let drafts = list_drafts(Path::new("/nonexistent/path")).unwrap();
        assert!(drafts.is_empty());
    }

    #[test]
    fn approve_and_reject_draft() {
        let drafts_dir = TempDir::new().unwrap();
        let skills_dir = TempDir::new().unwrap();
        let storage = make_storage();

        // Generate a draft.
        for _ in 0..3 {
            record(&storage, &make_entry("a1", "deploy the service", false, 5)).unwrap();
        }
        let result = generate_drafts(&storage, "a1", drafts_dir.path(), 50).unwrap();
        assert!(!result.drafts.is_empty());

        let draft_name = &result.drafts[0].name;
        let draft_path = drafts_dir.path().join(format!("{draft_name}.md"));
        assert!(draft_path.exists());

        // Approve: move to skills dir.
        approve_draft(drafts_dir.path(), skills_dir.path(), draft_name).unwrap();
        assert!(!draft_path.exists());
        let skill_path = skills_dir.path().join(format!("{draft_name}.md"));
        assert!(skill_path.exists());

        // Generate another draft and reject it.
        let result2 = generate_drafts(&storage, "a1", drafts_dir.path(), 50).unwrap();
        if !result2.drafts.is_empty() {
            let name2 = &result2.drafts[0].name;
            reject_draft(drafts_dir.path(), name2).unwrap();
            assert!(!drafts_dir.path().join(format!("{name2}.md")).exists());
        }
    }

    #[test]
    fn parse_round_trip() {
        let draft = SkillDraft {
            name: "test-skill".into(),
            description: "A test skill".into(),
            rationale: "Because testing".into(),
            body: "## Body\nContent here.".into(),
            triggers: vec!["deploy".into(), "service".into()],
            agent_id: "a1".into(),
            samples_analyzed: 10,
            generated_at: "2026-06-30T00:00:00Z".into(),
        };
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("test-skill.md");
        std::fs::write(&path, format_draft_md(&draft, &draft.generated_at)).unwrap();

        let parsed = parse_draft_md(&path).unwrap();
        assert_eq!(parsed.name, "test-skill");
        assert_eq!(parsed.description, "A test skill");
        assert!(parsed.body.contains("Body"));
        assert_eq!(parsed.agent_id, "a1");
        assert_eq!(parsed.samples_analyzed, 10);
    }
}
