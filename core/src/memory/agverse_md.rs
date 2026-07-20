//! Policy for `agverse.md`: injection budget, section routing, fact quality,
//! Pending Notes lifecycle, and capacity maintenance.

use chrono::{Duration, NaiveDate, Utc};
use regex::Regex;
use std::collections::HashMap;
use std::sync::OnceLock;

/// Hard cap for Global Memory text injected every turn (chars).
pub const GLOBAL_INJECT_BUDGET_CHARS: usize = 4_000;
/// Soft size limit for on-disk `agverse.md` before trimming Architecture bullets.
pub const AGVERSE_SOFT_LIMIT_CHARS: usize = 6_000;
/// Max chars for a single always-on fact bullet.
pub const MAX_FACT_CHARS: usize = 200;
/// Pending Notes older than this many days are archived off always-on storage.
pub const PENDING_TTL_DAYS: i64 = 7;
/// How many Pending Notes lines to inject (0 = never inject Pending).
pub const PENDING_INJECT_MAX: usize = 0;

/// Canonical section headers Reflection may write into.
pub const STANDARD_SECTIONS: &[&str] = &[
    "Known Projects (catalog)",
    "Project Overview",
    "Tech Stack & Commands",
    "Architecture Decisions",
    "Coding Conventions",
    "User Preferences",
    "Agent Instructions",
];

const PENDING_SECTION: &str = "Pending Notes";

/// Where a fact should live after classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactDisposition {
    /// Write into always-on markdown (global or project).
    AlwaysOn,
    /// Store in archival / reflection_facts only — not agverse.md.
    ArchivalOnly,
    /// Drop entirely.
    Reject,
}

/// Scope for always-on markdown writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactScope {
    Global,
    Project,
}

#[derive(Debug, Clone)]
pub struct InjectOptions {
    pub budget_chars: usize,
    pub pending_inject_max: usize,
}

impl Default for InjectOptions {
    fn default() -> Self {
        Self {
            budget_chars: GLOBAL_INJECT_BUDGET_CHARS,
            pending_inject_max: PENDING_INJECT_MAX,
        }
    }
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MaintainReport {
    pub pending_expired: usize,
    pub trimmed_bullets: usize,
    pub sections_ensured: bool,
}

/// Normalize a free-form section label to a canonical standard section.
pub fn normalize_section_name(raw: &str) -> Option<&'static str> {
    let key = raw.trim().to_lowercase();
    let key = key.trim_start_matches('#').trim();
    if key.is_empty() || key == "pending notes" || key == "pending" {
        return None;
    }
    if key.contains("known project") || key == "catalog" {
        return Some("Known Projects (catalog)");
    }
    if key.contains("project overview") || key == "overview" || key == "project" {
        return Some("Project Overview");
    }
    if key.contains("tech stack") || key.contains("commands") || key == "stack" {
        return Some("Tech Stack & Commands");
    }
    if key.contains("architecture") || key.contains("decision") {
        return Some("Architecture Decisions");
    }
    if key.contains("coding") || key.contains("convention") || key.contains("style") {
        return Some("Coding Conventions");
    }
    if key.contains("user preference") || key.contains("preference") || key == "preferences" {
        return Some("User Preferences");
    }
    if key.contains("agent instruction") || key.contains("instruction") {
        return Some("Agent Instructions");
    }
    // Exact match against canonical names (case-insensitive).
    for section in STANDARD_SECTIONS {
        if section.eq_ignore_ascii_case(raw.trim()) {
            return Some(*section);
        }
    }
    None
}

pub fn scope_for_section(section: &str) -> FactScope {
    match section {
        "User Preferences" | "Agent Instructions" | "Known Projects (catalog)" => FactScope::Global,
        _ => FactScope::Project,
    }
}

/// Ensure standard `# Section` headers exist so facts never dump into Pending
/// solely because the template was incomplete.
pub fn ensure_standard_sections(content: &str) -> String {
    let mut result = content.to_string();
    if result.is_empty() {
        result.push_str("# OS-Level Memory Architecture\n\n");
    }
    for section in STANDARD_SECTIONS {
        let header = format!("# {section}");
        if !has_section_header(&result, section) {
            if !result.ends_with('\n') {
                result.push('\n');
            }
            result.push_str(&format!("\n{header}\n"));
        }
    }
    result
}

fn has_section_header(content: &str, section: &str) -> bool {
    let needle = format!("# {section}");
    content.lines().any(|line| line.trim() == needle)
}

/// Prepare markdown for prompt injection: drop/limit Pending, prioritize
/// durable sections, enforce a character budget.
pub fn prepare_for_injection(content: &str, opts: &InjectOptions) -> String {
    if content.trim().is_empty() {
        return String::new();
    }
    let (preamble, mut sections) = parse_sections(content);

    // Pending: exclude by default, or keep only the newest N lines.
    if let Some(pending_body) = sections.remove(PENDING_SECTION) {
        if opts.pending_inject_max > 0 {
            let lines: Vec<&str> = pending_body
                .lines()
                .filter(|l| is_bullet(l))
                .collect();
            let keep = lines
                .iter()
                .rev()
                .take(opts.pending_inject_max)
                .copied()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !keep.is_empty() {
                sections.insert(PENDING_SECTION.to_string(), keep);
            }
        }
    }

    // Priority order for always-on injection.
    const PRIORITY: &[&str] = &[
        "User Preferences",
        "Agent Instructions",
        "Known Projects (catalog)",
        "Active Project Rule (CRITICAL)",
        "Project Overview",
        "Tech Stack & Commands",
        "Coding Conventions",
        "Architecture Decisions",
        PENDING_SECTION,
    ];

    let mut out = String::new();
    let preamble_trim = preamble.trim();
    if !preamble_trim.is_empty() {
        // Keep a short preamble only — full OS manifesto wastes budget.
        let short = truncate_chars(preamble_trim, 600);
        out.push_str(&short);
        if !short.ends_with('\n') {
            out.push('\n');
        }
        out.push('\n');
    }

    let mut remaining = opts.budget_chars.saturating_sub(out.chars().count());
    let mut emitted: std::collections::HashSet<String> = std::collections::HashSet::new();

    for name in PRIORITY {
        if remaining == 0 {
            break;
        }
        let Some(body) = sections.get(*name) else {
            continue;
        };
        let chunk = format_section(name, body);
        let chunk_len = chunk.chars().count();
        if chunk_len <= remaining {
            out.push_str(&chunk);
            remaining = remaining.saturating_sub(chunk_len);
            emitted.insert((*name).to_string());
        } else if remaining > 80 {
            // Fit a truncated version of high-priority sections.
            let header = format!("# {name}\n");
            let header_len = header.chars().count();
            if remaining > header_len + 20 {
                let body_budget = remaining - header_len - 20;
                let truncated_body = truncate_chars(body.trim(), body_budget);
                out.push_str(&header);
                out.push_str(&truncated_body);
                out.push_str("\n…(truncated)\n\n");
                remaining = 0;
                emitted.insert((*name).to_string());
            }
        }
    }

    // Include any other non-pending sections that still fit (e.g. custom).
    let mut other_names: Vec<_> = sections
        .keys()
        .filter(|k| !emitted.contains(*k) && k.as_str() != PENDING_SECTION)
        .cloned()
        .collect();
    other_names.sort();
    for name in other_names {
        if remaining == 0 {
            break;
        }
        let body = sections.get(&name).unwrap();
        let chunk = format_section(&name, body);
        let chunk_len = chunk.chars().count();
        if chunk_len <= remaining {
            out.push_str(&chunk);
            remaining = remaining.saturating_sub(chunk_len);
        }
    }

    out.trim_end().to_string()
}

fn format_section(name: &str, body: &str) -> String {
    let body = body.trim();
    if body.is_empty() {
        format!("# {name}\n\n")
    } else {
        format!("# {name}\n{body}\n\n")
    }
}

/// Classify whether a fact should be always-on, archival-only, or rejected.
pub fn classify_fact(section: &str, text: &str) -> FactDisposition {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return FactDisposition::Reject;
    }
    if trimmed.chars().count() > MAX_FACT_CHARS {
        return FactDisposition::Reject;
    }
    if normalize_section_name(section).is_none() {
        return FactDisposition::ArchivalOnly;
    }

    // Line-number / path dumps (e.g. `foo.rs:123`, `lib.rs:57`).
    if line_ref_re().is_match(trimmed) {
        return FactDisposition::ArchivalOnly;
    }

    // Transient / snapshot language — belongs in archival, not constitution.
    if is_transient_fact(trimmed) {
        return FactDisposition::ArchivalOnly;
    }

    // Audit / checklist dumps.
    if looks_like_audit_dump(trimmed) {
        return FactDisposition::ArchivalOnly;
    }

    FactDisposition::AlwaysOn
}

fn line_ref_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"(?i)\b[\w./-]+\.(rs|ts|tsx|js|py|toml|md):\d+\b").expect("regex"))
}

fn is_transient_fact(text: &str) -> bool {
    let lower = text.to_lowercase();
    const MARKERS: &[&str] = &[
        "as of this batch",
        "now has",
        "now includes",
        "passing tests",
        "tests passed",
        "cargo check pass",
        "zero tests",
        "fails to compile",
        "does not compile",
        "identified during",
        "proposed fix",
        "pending user approval",
        "todo:",
        "wip:",
        "for now",
        "this session",
        "in this conversation",
        "as of this",
        "currently has",
        "currently only",
        "currently uses",
        "currently fails",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn looks_like_audit_dump(text: &str) -> bool {
    let lower = text.to_lowercase();
    // Numbered defect lists: "(1) ... (2) ..."
    let numbered = (1..=6)
        .filter(|n| lower.contains(&format!("({n})")) || lower.contains(&format!("{n}) ")))
        .count();
    if numbered >= 3 {
        return true;
    }
    // Many file:line refs in one bullet.
    if line_ref_re().find_iter(text).count() >= 2 {
        return true;
    }
    false
}

/// True when two facts in the same section are similar enough that the new
/// one should replace the old (Jaccard over significant words, or long
/// alphanumeric prefix match).
pub fn facts_conflict(a: &str, b: &str) -> bool {
    let na: String = a
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    let nb: String = b
        .chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect();
    if !na.is_empty() && !nb.is_empty() {
        let prefix = na
            .chars()
            .zip(nb.chars())
            .take_while(|(x, y)| x == y)
            .count();
        if prefix >= 20 {
            return true;
        }
    }

    let wa = significant_words(a);
    let wb = significant_words(b);
    if wa.is_empty() || wb.is_empty() {
        return false;
    }
    let inter = wa.intersection(&wb).count();
    let union = wa.union(&wb).count().max(1);
    (inter as f32 / union as f32) >= 0.55
}

fn significant_words(text: &str) -> std::collections::HashSet<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.len() > 2)
        .filter(|w| {
            !matches!(
                *w,
                "the" | "and" | "for" | "with" | "from" | "that" | "this" | "are" | "was"
                    | "were" | "have" | "has" | "not" | "but" | "via" | "into" | "only"
            )
        })
        .map(str::to_string)
        .collect()
}

/// Append facts into matching sections. Ensures standard sections exist,
/// normalizes section names, and replaces conflicting bullets in-section.
pub fn append_facts_to_sections(content: &str, facts: &[(String, String)]) -> String {
    let mut result = ensure_standard_sections(content);

    for (section_raw, text) in facts {
        let Some(section) = normalize_section_name(section_raw) else {
            // Unknown section → Pending with date stamp for TTL.
            result = append_pending(result, section_raw, text);
            continue;
        };
        result = remove_conflicts_in_section(&result, section, text);
        result = insert_bullet_in_section(&result, section, text);
    }

    result
}

fn append_pending(content: String, section: &str, text: &str) -> String {
    let mut result = content;
    if !has_section_header(&result, PENDING_SECTION) {
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("\n# {PENDING_SECTION}\n"));
    }
    let today = Utc::now().format("%Y-%m-%d");
    let line = format!("- [{section}|{today}] {text}\n");
    // Insert at end of Pending section.
    insert_bullet_in_section(&result, PENDING_SECTION, line.trim_start_matches("- ").trim())
}

fn insert_bullet_in_section(content: &str, section: &str, text: &str) -> String {
    let header = format!("# {section}");
    let Some(pos) = find_section_header_pos(content, section) else {
        let mut result = content.to_string();
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(&format!("\n{header}\n- {text}\n"));
        return result;
    };
    let after_header = pos + header.len();
    let next_section = content[after_header..]
        .find("\n# ")
        .map(|p| after_header + p)
        .unwrap_or(content.len());
    let mut result = content.to_string();
    let bullet = if text.starts_with('-') {
        format!("\n{text}")
    } else {
        format!("\n- {text}")
    };
    result.insert_str(next_section, &bullet);
    result
}

fn find_section_header_pos(content: &str, section: &str) -> Option<usize> {
    let needle = format!("# {section}");
    let mut offset = 0;
    for line in content.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']).trim() == needle {
            return Some(offset);
        }
        offset += line.len();
    }
    None
}

fn remove_conflicts_in_section(content: &str, section: &str, new_text: &str) -> String {
    let (preamble, mut sections) = parse_sections(content);
    let Some(body) = sections.get_mut(section) else {
        return content.to_string();
    };
    let kept: Vec<&str> = body
        .lines()
        .filter(|line| {
            if !is_bullet(line) {
                return true;
            }
            let candidate = bullet_text(line);
            !facts_conflict(candidate, new_text)
        })
        .collect();
    *body = kept.join("\n");
    rebuild_markdown(&preamble, &sections)
}

/// Remove the entire `# Pending Notes` section.
pub fn clear_pending_notes(content: &str) -> (String, usize) {
    let (preamble, mut sections) = parse_sections(content);
    let removed = sections
        .remove(PENDING_SECTION)
        .map(|body| body.lines().filter(|l| is_bullet(l)).count())
        .unwrap_or(0);
    (rebuild_markdown(&preamble, &sections), removed)
}

/// Promote Pending Notes into their tagged standard sections when possible.
pub fn promote_pending_notes(content: &str) -> (String, usize) {
    let (preamble, mut sections) = parse_sections(content);
    let Some(pending_body) = sections.remove(PENDING_SECTION) else {
        return (content.to_string(), 0);
    };

    let mut promoted = 0;
    let mut leftover = Vec::new();
    let mut result = rebuild_markdown(&preamble, &sections);

    for line in pending_body.lines() {
        if !is_bullet(line) {
            continue;
        }
        let raw = bullet_text(line);
        let (tag, text) = split_pending_tag(raw);
        if let Some(section) = tag.and_then(normalize_section_name) {
            result = remove_conflicts_in_section(&result, section, text);
            result = insert_bullet_in_section(&result, section, text);
            promoted += 1;
        } else {
            leftover.push(line.to_string());
        }
    }

    if !leftover.is_empty() {
        let mut map = parse_sections(&result).1;
        map.insert(PENDING_SECTION.to_string(), leftover.join("\n"));
        let (pre, _) = parse_sections(&result);
        result = rebuild_markdown(&pre, &map);
    }

    (ensure_standard_sections(&result), promoted)
}

/// Expire dated Pending Notes older than `ttl_days`. Undated pending lines
/// are treated as expired (legacy backlog).
pub fn expire_pending_notes(content: &str, ttl_days: i64) -> (String, usize) {
    let (preamble, mut sections) = parse_sections(content);
    let Some(pending_body) = sections.get(PENDING_SECTION).cloned() else {
        return (content.to_string(), 0);
    };
    let cutoff = (Utc::now().naive_utc().date() - Duration::days(ttl_days)).to_string();
    let mut kept = Vec::new();
    let mut expired = 0;
    for line in pending_body.lines() {
        if !is_bullet(line) {
            kept.push(line.to_string());
            continue;
        }
        let raw = bullet_text(line);
        match pending_date(raw) {
            Some(date) if date.as_str() >= cutoff.as_str() => kept.push(line.to_string()),
            Some(_) | None => expired += 1,
        }
    }
    if kept.iter().any(|l| is_bullet(l)) {
        sections.insert(PENDING_SECTION.to_string(), kept.join("\n"));
    } else {
        sections.remove(PENDING_SECTION);
    }
    (rebuild_markdown(&preamble, &sections), expired)
}

fn pending_date(raw: &str) -> Option<String> {
    // Formats: [Section|YYYY-MM-DD] text  OR  [Section] text
    let rest = raw.strip_prefix('[')?;
    let (inside, _) = rest.split_once(']')?;
    let date = inside.split('|').nth(1)?.trim();
    NaiveDate::parse_from_str(date, "%Y-%m-%d").ok()?;
    Some(date.to_string())
}

fn split_pending_tag(raw: &str) -> (Option<&str>, &str) {
    let trimmed = raw.trim();
    if let Some(rest) = trimmed.strip_prefix('[')
        && let Some((inside, text)) = rest.split_once(']')
    {
        let tag = inside.split('|').next().unwrap_or(inside).trim();
        return (Some(tag), text.trim().trim_start_matches(':').trim());
    }
    (None, trimmed)
}

/// If content exceeds soft limit, drop oldest Architecture / Project Overview
/// bullets until under budget.
pub fn trim_to_soft_limit(content: &str, soft_limit: usize) -> (String, usize) {
    if content.chars().count() <= soft_limit {
        return (content.to_string(), 0);
    }
    let (preamble, mut sections) = parse_sections(content);
    let mut trimmed = 0;
    const TRIM_ORDER: &[&str] = &["Architecture Decisions", "Project Overview", "Tech Stack & Commands"];

    for section in TRIM_ORDER {
        while rebuild_markdown(&preamble, &sections).chars().count() > soft_limit {
            let Some(body) = sections.get_mut(*section) else {
                break;
            };
            let lines: Vec<&str> = body.lines().collect();
            let Some(idx) = lines.iter().rposition(|l| is_bullet(l)) else {
                break;
            };
            let mut new_lines = lines;
            new_lines.remove(idx);
            *body = new_lines.join("\n");
            trimmed += 1;
        }
    }

    (rebuild_markdown(&preamble, &sections), trimmed)
}

/// Run ensure + expire + trim maintenance on agverse.md content.
pub fn maintain_agverse_content(content: &str) -> (String, MaintainReport) {
    let mut report = MaintainReport::default();
    let ensured = ensure_standard_sections(content);
    report.sections_ensured = ensured != content;
    let (expired_content, expired) = expire_pending_notes(&ensured, PENDING_TTL_DAYS);
    report.pending_expired = expired;
    let (trimmed_content, trimmed) = trim_to_soft_limit(&expired_content, AGVERSE_SOFT_LIMIT_CHARS);
    report.trimmed_bullets = trimmed;
    (trimmed_content, report)
}

pub fn clear_pending_notes_file(path: &std::path::Path) -> anyhow::Result<usize> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let (updated, removed) = clear_pending_notes(&content);
    if removed > 0 {
        atomic_write(path, &updated)?;
    }
    Ok(removed)
}

pub fn promote_pending_notes_file(path: &std::path::Path) -> anyhow::Result<usize> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(e.into()),
    };
    let (updated, promoted) = promote_pending_notes(&content);
    if promoted > 0 || updated != content {
        atomic_write(path, &updated)?;
    }
    Ok(promoted)
}

pub fn maintain_agverse_file(path: &std::path::Path) -> anyhow::Result<MaintainReport> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Ok(MaintainReport::default());
        }
        Err(e) => return Err(e.into()),
    };
    let (updated, report) = maintain_agverse_content(&content);
    if updated != content {
        atomic_write(path, &updated)?;
    }
    Ok(report)
}

fn atomic_write(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp = path.with_extension(format!("md.policy.{}.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temp, content)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

// ── Markdown section parsing ──────────────────────────────────────────

fn parse_sections(content: &str) -> (String, HashMap<String, String>) {
    let mut preamble = String::new();
    let mut sections: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;
    let mut body = String::new();

    for line in content.lines() {
        if let Some(name) = parse_h1(line) {
            if let Some(prev) = current.take() {
                sections.insert(prev, body.trim_end().to_string());
            } else if !body.is_empty() {
                preamble = body.trim_end().to_string();
            }
            current = Some(name);
            body = String::new();
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    if let Some(prev) = current {
        sections.insert(prev, body.trim_end().to_string());
    } else if preamble.is_empty() {
        preamble = body.trim_end().to_string();
    } else {
        preamble.push('\n');
        preamble.push_str(body.trim_end());
    }
    (preamble, sections)
}

fn parse_h1(line: &str) -> Option<String> {
    let t = line.trim();
    if let Some(rest) = t.strip_prefix("# ")
        && !rest.starts_with('#')
    {
        return Some(rest.trim().to_string());
    }
    None
}

fn rebuild_markdown(preamble: &str, sections: &HashMap<String, String>) -> String {
    let mut out = String::new();
    if !preamble.trim().is_empty() {
        out.push_str(preamble.trim_end());
        out.push_str("\n\n");
    }
    // Stable order: standard sections first, then others alphabetically,
    // Pending last.
    let mut names: Vec<_> = sections.keys().cloned().collect();
    names.sort_by(|a, b| {
        let rank = |n: &str| {
            STANDARD_SECTIONS
                .iter()
                .position(|s| *s == n)
                .or_else(|| {
                    if n == "Active Project Rule (CRITICAL)" {
                        Some(0)
                    } else if n == PENDING_SECTION {
                        Some(1000)
                    } else {
                        None
                    }
                })
                .unwrap_or(500)
        };
        rank(a).cmp(&rank(b)).then_with(|| a.cmp(b))
    });
    for name in names {
        let body = sections.get(&name).map(|s| s.as_str()).unwrap_or("");
        out.push_str(&format!("# {name}\n"));
        if !body.is_empty() {
            out.push_str(body);
            if !body.ends_with('\n') {
                out.push('\n');
            }
        }
        out.push('\n');
    }
    out
}

fn is_bullet(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("- ") || t.starts_with("* ")
}

fn bullet_text(line: &str) -> &str {
    line.trim()
        .trim_start_matches(['-', '*'])
        .trim()
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    s.chars().take(max).collect::<String>() + "…"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_section_aliases() {
        assert_eq!(
            normalize_section_name("architecture decisions"),
            Some("Architecture Decisions")
        );
        assert_eq!(
            normalize_section_name("User Preferences"),
            Some("User Preferences")
        );
        assert_eq!(normalize_section_name("Pending Notes"), None);
    }

    #[test]
    fn prepare_excludes_pending_by_default() {
        let md = "# User Preferences\n- likes rust\n\n# Pending Notes\n- [Architecture Decisions] too detailed dump\n";
        let out = prepare_for_injection(md, &InjectOptions::default());
        assert!(out.contains("likes rust"));
        assert!(!out.contains("too detailed dump"));
        assert!(!out.contains("Pending Notes"));
    }

    #[test]
    fn prepare_respects_budget() {
        let mut md = String::from("# User Preferences\n- short pref\n\n# Architecture Decisions\n");
        for i in 0..80 {
            md.push_str(&format!("- decision number {i} with lots of padding text to inflate size\n"));
        }
        let out = prepare_for_injection(
            &md,
            &InjectOptions {
                budget_chars: 800,
                pending_inject_max: 0,
            },
        );
        assert!(out.chars().count() <= 850);
        assert!(out.contains("short pref"));
    }

    #[test]
    fn classify_rejects_line_refs_and_transient() {
        assert_eq!(
            classify_fact(
                "Architecture Decisions",
                "Bug in core/src/runtime/run.rs:1325 needs fix"
            ),
            FactDisposition::ArchivalOnly
        );
        assert_eq!(
            classify_fact(
                "Project Overview",
                "MCP Router now has full test coverage as of this batch"
            ),
            FactDisposition::ArchivalOnly
        );
        assert_eq!(
            classify_fact("User Preferences", "User prefers English responses matching input language"),
            FactDisposition::AlwaysOn
        );
    }

    #[test]
    fn append_ensures_sections_and_avoids_pending() {
        let content = "# Project Overview\n\nSome overview.\n";
        let facts = vec![(
            "User Preferences".to_string(),
            "User prefers dark mode".to_string(),
        )];
        let result = append_facts_to_sections(content, &facts);
        assert!(result.contains("# User Preferences"));
        assert!(result.contains("- User prefers dark mode"));
        assert!(!result.contains("# Pending Notes"));
    }

    #[test]
    fn append_replaces_conflicting_fact() {
        let content = ensure_standard_sections(
            "# Architecture Decisions\n- Use PostgreSQL for local storage\n",
        );
        let facts = vec![(
            "Architecture Decisions".to_string(),
            "Use SQLite for local storage".to_string(),
        )];
        let result = append_facts_to_sections(&content, &facts);
        assert!(result.contains("SQLite"));
        assert!(!result.contains("PostgreSQL"));
    }

    #[test]
    fn clear_and_promote_pending() {
        let md = "# User Preferences\n\n# Pending Notes\n- [User Preferences] likes tea\n- [Weird] keep me\n";
        let (promoted_md, n) = promote_pending_notes(md);
        assert_eq!(n, 1);
        assert!(promoted_md.contains("# User Preferences"));
        assert!(promoted_md.contains("likes tea"));
        assert!(promoted_md.contains("keep me"));

        let (cleared, removed) = clear_pending_notes(&promoted_md);
        assert_eq!(removed, 1);
        assert!(!cleared.contains("Pending Notes"));
    }

    #[test]
    fn expire_undated_pending() {
        let md = "# Pending Notes\n- [Architecture Decisions] old undated junk\n";
        let (out, n) = expire_pending_notes(md, 7);
        assert_eq!(n, 1);
        assert!(!out.contains("old undated junk"));
    }

    #[test]
    fn scope_routing() {
        assert_eq!(scope_for_section("User Preferences"), FactScope::Global);
        assert_eq!(scope_for_section("Architecture Decisions"), FactScope::Project);
    }
}
