//! Time-decay salience scoring with Ebbinghaus forgetting curve,
//! importance auto-rating, and access-count reinforcement.
//!
//! Core formula:
//! ```text
//! recall(t, S) = e^(-t / (S × half_life))
//! score = α · semantic_similarity + β · recall(t,S) + γ · importance
//!
//! Where:
//!   t  = hours since creation
//!   S  = memory_strength (starts at 1.0, grows with accesses)
//!   half_life = configurable per category (default 168h = 1 week)
//! ```

use serde::{Deserialize, Serialize};

// ── Configuration ────────────────────────────────────────────────────

/// Configuration for the salience scorer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SalienceConfig {
    /// Weight for semantic similarity (cosine).
    #[serde(default = "default_alpha")]
    pub alpha: f32,

    /// Weight for time-decay recall score.
    #[serde(default = "default_beta")]
    pub beta: f32,

    /// Weight for importance rating.
    #[serde(default = "default_gamma")]
    pub gamma: f32,

    /// Default half-life for memories, in hours. After this many hours,
    /// a memory with strength=1.0 will have recall ≈ 0.37.
    #[serde(default = "default_half_life")]
    pub default_half_life_hours: f32,

    /// Half-life multiplier for high-importance memories (importance > 0.7).
    /// E.g., 3.0 means high-importance memories decay 3× slower.
    #[serde(default = "default_decay_modifier")]
    pub importance_decay_modifier: f32,

    /// How much to increase memory_strength on each access (additive).
    #[serde(default = "default_bump_add")]
    pub strength_bump_additive: f32,

    /// How much to multiply memory_strength on each access (multiplicative).
    /// Final: S_new = S_old * multiplier + additive, capped at max.
    #[serde(default = "default_bump_mul")]
    pub strength_bump_multiplicative: f32,

    /// Maximum memory strength. Beyond this, accesses are ignored.
    #[serde(default = "default_max_strength")]
    pub max_strength: f32,

    /// Minimum recall score floor (e.g., 0.01 means memories never fully decay).
    #[serde(default = "default_recall_floor")]
    pub recall_floor: f32,
}

fn default_alpha() -> f32 {
    0.55
}
fn default_beta() -> f32 {
    0.25
}
fn default_gamma() -> f32 {
    0.20
}
fn default_half_life() -> f32 {
    168.0 // 1 week
}
fn default_decay_modifier() -> f32 {
    3.0
}
fn default_bump_add() -> f32 {
    0.15
}
fn default_bump_mul() -> f32 {
    1.05
}
fn default_max_strength() -> f32 {
    5.0
}
fn default_recall_floor() -> f32 {
    0.01
}

impl Default for SalienceConfig {
    fn default() -> Self {
        Self {
            alpha: default_alpha(),
            beta: default_beta(),
            gamma: default_gamma(),
            default_half_life_hours: default_half_life(),
            importance_decay_modifier: default_decay_modifier(),
            strength_bump_additive: default_bump_add(),
            strength_bump_multiplicative: default_bump_mul(),
            max_strength: default_max_strength(),
            recall_floor: default_recall_floor(),
        }
    }
}

// ── Salience Scorer ──────────────────────────────────────────────────

/// The core salience scorer — implements Ebbinghaus decay, importance
/// heuristics, and strength reinforcement.
pub struct SalienceScorer {
    config: SalienceConfig,
}

impl SalienceScorer {
    pub fn new(config: SalienceConfig) -> Self {
        Self { config }
    }

    pub fn config(&self) -> &SalienceConfig {
        &self.config
    }

    // ── Ebbinghaus recall ───────────────────────────────────────────

    /// Compute the Ebbinghaus recall score:
    ///
    ///   R(t, S, importance) = e^(-t / (effective_half_life))
    ///
    /// where effective_half_life = S × half_life × decay_modifier(importance)
    pub fn recall_score(
        &self,
        hours_since_created: f32,
        memory_strength: f32,
        importance: f32,
    ) -> f32 {
        if hours_since_created <= 0.0 {
            return 1.0;
        }

        let half_life = self.config.default_half_life_hours;

        // High-importance memories decay slower
        let importance_factor = 1.0
            + (self.config.importance_decay_modifier - 1.0)
                * ((importance - 0.5) * 2.0).max(0.0); // only above 0.5

        // Protective: avoid division by zero
        let s = memory_strength.max(0.01);

        let effective_half_life = s * half_life * importance_factor;

        // Ebbinghaus exponential decay
        let r = (-hours_since_created / effective_half_life).exp();

        // Apply floor
        r.max(self.config.recall_floor)
    }

    // ── Full retrieval score ────────────────────────────────────────

    /// Compute the combined retrieval score:
    ///
    ///   score = α × semantic + β × recall(t, S, imp) + γ × imp
    pub fn retrieval_score(
        &self,
        semantic_similarity: f32,
        hours_since_created: f32,
        memory_strength: f32,
        importance: f32,
    ) -> f32 {
        let recall = self.recall_score(hours_since_created, memory_strength, importance);

        self.config.alpha * semantic_similarity
            + self.config.beta * recall
            + self.config.gamma * importance
    }

    // ── Strength reinforcement ──────────────────────────────────────

    /// Bump memory strength on access. Returns new strength.
    pub fn bump_strength(&self, current_strength: f32) -> f32 {
        let new_strength = current_strength * self.config.strength_bump_multiplicative
            + self.config.strength_bump_additive;
        new_strength.min(self.config.max_strength)
    }

    // ── Importance auto-rating ──────────────────────────────────────

    /// Auto-rate importance of content using heuristics (no LLM needed).
    /// Returns a value in [0.0, 1.0].
    pub fn auto_rate_importance(&self, content: &str, role: &str) -> f32 {
        let mut score: f32 = 0.3; // base

        // User messages tend to be more important
        if role == "user" {
            score += 0.1;
        }

        // Decision keywords — strong signal
        let decision_keywords = [
            "决定", "以后都", "定下来", "就这样", "final",
            "important", "remember", "记住", "必须", "never",
            "always", "rule", "convention", "规则", "约定",
            "prefer", "偏好", "习惯", "don't",
        ];
        for kw in &decision_keywords {
            if content.to_lowercase().contains(&kw.to_lowercase()) {
                score += 0.08;
            }
        }

        // File paths — moderate signal
        let path_indicators = [".rs", ".ts", ".py", ".js", ".toml", ".md", "/", "\\", ".go"];
        for indic in &path_indicators {
            if content.contains(indic) {
                score += 0.03;
                break; // count once
            }
        }

        // Numbers/measurements — mild signal
        let has_numbers = content.chars().any(|c| c.is_ascii_digit());
        if has_numbers {
            score += 0.02;
        }

        // Longer content tends to be more substantive
        let len = content.chars().count();
        if len > 500 {
            score += 0.05;
        } else if len > 200 {
            score += 0.03;
        } else if len < 20 {
            score -= 0.05;
        }

        // Named entities: capitalized words or Chinese names
        let upper_count = content
            .chars()
            .filter(|c| c.is_uppercase())
            .count();
        if upper_count > 5 {
            score += 0.03;
        }

        score.clamp(0.0, 1.0)
    }

    // ── Category half-life ──────────────────────────────────────────

    /// Get half-life for a specific memory category.
    pub fn category_half_life(&self, category: MemoryCategory) -> f32 {
        match category {
            MemoryCategory::Conversation => self.config.default_half_life_hours,
            MemoryCategory::Decision => self.config.default_half_life_hours * 2.0,
            MemoryCategory::Code => self.config.default_half_life_hours * 1.5,
            MemoryCategory::Preference => self.config.default_half_life_hours * 3.0,
            MemoryCategory::Trivia => self.config.default_half_life_hours * 0.5,
        }
    }
}

// ── Memory Category ──────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MemoryCategory {
    /// General conversation turns.
    Conversation,
    /// Decisions, commitments, rules.
    Decision,
    /// Code snippets, file references, programming context.
    Code,
    /// User preferences, habits, conventions.
    Preference,
    /// Casual chat, unimportant filler.
    Trivia,
}

impl MemoryCategory {
    /// Classify content into a category based on heuristics.
    pub fn classify(content: &str, role: &str) -> Self {
        let lower = content.to_lowercase();

        // Decision signals
        let decision_signals = [
            "决定", "以后", "规则", "约定", "convention", "rule", "never",
            "always", "必须", "must", "should", "final", "就这样",
        ];
        for signal in &decision_signals {
            if lower.contains(signal) {
                return Self::Decision;
            }
        }

        // Preference signals
        let pref_signals = ["偏好", "prefer", "喜欢", "习惯", "usually", "一般", "常用"];
        for signal in &pref_signals {
            if lower.contains(signal) {
                return Self::Preference;
            }
        }

        // Code signals
        let code_signals = [
            ".rs", ".py", ".ts", ".js", "fn ", "def ", "class ", "import ", "use ",
            "struct ", "impl ", "async fn", "pub fn",
        ];
        for signal in &code_signals {
            if lower.contains(signal) {
                return Self::Code;
            }
        }

        // Trivia signals
        if role == "tool" && content.len() < 200 {
            let trivia_signals = ["ok", "done", "success", "result: 0", "exit code"];
            for signal in &trivia_signals {
                if lower.contains(signal) {
                    return Self::Trivia;
                }
            }
        }

        Self::Conversation
    }
}

// ── Scored Record ────────────────────────────────────────────────────

/// A memory record with its retrieval score and breakdown.
#[derive(Debug, Clone)]
pub struct ScoredRecord {
    pub id: String,
    pub content: String,
    pub total_score: f32,
    pub semantic_score: f32,
    pub recall_score: f32,
    pub importance: f32,
    pub memory_strength: f32,
    pub hours_since_created: f32,
    pub category: MemoryCategory,
}

impl ScoredRecord {
    pub fn breakdown(&self) -> String {
        format!(
            "total={:.3} sem={:.3} recall={:.3} imp={:.2} strength={:.2} age={:.0}h cat={:?}",
            self.total_score,
            self.semantic_score,
            self.recall_score,
            self.importance,
            self.memory_strength,
            self.hours_since_created,
            self.category,
        )
    }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recall_score_fresh() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        // Just created (0 hours) → recall = 1.0
        let r = scorer.recall_score(0.0, 1.0, 0.5);
        assert!((r - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_recall_score_at_half_life() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        // At exactly half life, recall ≈ e^(-1) ≈ 0.368
        let r = scorer.recall_score(168.0, 1.0, 0.5);
        assert!((r - 0.368).abs() < 0.05);
    }

    #[test]
    fn test_recall_score_high_strength_slower_decay() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        // strength=3.0 → effective HL = 3×168 = 504h
        let r_normal = scorer.recall_score(168.0, 1.0, 0.5); // ~0.37
        let r_strong = scorer.recall_score(168.0, 3.0, 0.5); // ~0.72
        assert!(r_strong > r_normal * 1.5);
    }

    #[test]
    fn test_recall_score_high_importance_slower_decay() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let r_low = scorer.recall_score(168.0, 1.0, 0.3);
        let r_high = scorer.recall_score(168.0, 1.0, 0.9);
        assert!(r_high > r_low);
    }

    #[test]
    fn test_recall_score_floor() {
        let mut config = SalienceConfig::default();
        config.recall_floor = 0.05;
        let scorer = SalienceScorer::new(config);
        // Very old, low strength → should hit floor
        let r = scorer.recall_score(10_000.0, 0.5, 0.1);
        assert!((r - 0.05).abs() < 0.01);
    }

    #[test]
    fn test_retrieval_score_combines_all() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let score = scorer.retrieval_score(0.9, 10.0, 1.5, 0.8);
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_bump_strength_grows() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let s1 = scorer.bump_strength(1.0);
        assert!(s1 > 1.0);
        let s2 = scorer.bump_strength(s1);
        assert!(s2 > s1);
    }

    #[test]
    fn test_bump_strength_capped() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let s = scorer.bump_strength(10.0);
        assert!((s - scorer.config.max_strength).abs() < 0.01);
    }

    #[test]
    fn test_auto_rate_importance_decision() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let score = scorer.auto_rate_importance("我决定以后都用 Rust 写后端了", "user");
        assert!(score > 0.5); // decision keywords should boost
    }

    #[test]
    fn test_auto_rate_importance_trivia() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let score = scorer.auto_rate_importance("ok", "tool");
        assert!(score < 0.4); // short tool response → low importance
    }

    #[test]
    fn test_auto_rate_importance_with_paths() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        let score = scorer.auto_rate_importance("修改了 src/main.rs 的基本配置", "user");
        assert!(score > 0.35); // path + user role
    }

    #[test]
    fn test_classify_decision() {
        let cat = MemoryCategory::classify("以后都用 PostgreSQL，不用 MySQL 了", "user");
        assert_eq!(cat, MemoryCategory::Decision);
    }

    #[test]
    fn test_classify_preference() {
        let cat = MemoryCategory::classify("我偏好用 Rust 而不是 Python", "user");
        assert_eq!(cat, MemoryCategory::Preference);
    }

    #[test]
    fn test_classify_code() {
        let cat = MemoryCategory::classify("pub fn main() -> Result<()> {", "assistant");
        assert_eq!(cat, MemoryCategory::Code);
    }

    #[test]
    fn test_classify_trivia() {
        let cat = MemoryCategory::classify("ok", "tool");
        assert_eq!(cat, MemoryCategory::Trivia);
    }

    #[test]
    fn test_classify_conversation_default() {
        let cat = MemoryCategory::classify("今天天气不错", "user");
        assert_eq!(cat, MemoryCategory::Conversation);
    }

    #[test]
    fn test_category_half_lives() {
        let scorer = SalienceScorer::new(SalienceConfig::default());
        assert!(scorer.category_half_life(MemoryCategory::Preference) > scorer.category_half_life(MemoryCategory::Conversation));
        assert!(scorer.category_half_life(MemoryCategory::Trivia) < scorer.category_half_life(MemoryCategory::Conversation));
    }

    #[test]
    fn test_scored_record_breakdown() {
        let record = ScoredRecord {
            id: "test".to_string(),
            content: "test content".to_string(),
            total_score: 0.85,
            semantic_score: 0.6,
            recall_score: 0.15,
            importance: 0.7,
            memory_strength: 2.0,
            hours_since_created: 5.0,
            category: MemoryCategory::Decision,
        };
        let bd = record.breakdown();
        assert!(bd.contains("0.850")); // total
        assert!(bd.contains("Decision"));
    }
}
