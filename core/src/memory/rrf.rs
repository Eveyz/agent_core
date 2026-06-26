//! Reciprocal Rank Fusion — merges multiple search result lists into a
//! single ranked list using the formula:
//!
//!   S(id) = Σ 1 / (k + rank_i(id))
//!
//! where k=60 is a smoothing constant and rank is 1-based.
//!
//! RRF preserves ranking signals from both BM25 and HNSW without requiring
//! score normalization, then feeds the fused ranking into the salience scorer.

use std::collections::HashMap;

/// Fuse multiple ranked result lists with RRF.
///
/// `lists` — each inner Vec is a list of ids in descending relevance order (best first).
/// `k` — smoothing constant, typically 60.
///
/// Returns a Vec of (id, score) sorted descending by RRF score.
pub fn fuse(lists: &[Vec<String>], k: usize) -> Vec<(String, f32)> {
    let mut scores: HashMap<String, f32> = HashMap::new();

    for list in lists {
        for (rank, id) in list.iter().enumerate() {
            let rrf = 1.0 / (k as f32 + rank as f32 + 1.0);
            *scores.entry(id.clone()).or_insert(0.0) += rrf;
        }
    }

    let mut sorted: Vec<_> = scores.into_iter().collect();
    sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    sorted
}

/// Same as fuse(), but normalizes scores into [0, 1] for downstream use.
///
/// Normalization formula: S_rrf(id) = k / (k + rrf_rank(id))
/// where rrf_rank is the 1..n fusion ranking.
pub fn fuse_normalized(lists: &[Vec<String>], k: usize) -> Vec<(String, f32)> {
    let fused = fuse(lists, k);

    fused
        .into_iter()
        .enumerate()
        .map(|(rank, (id, _))| {
            // rrf_rank is 1-based in the original formula
            let s_rrf = k as f32 / (k as f32 + rank as f32 + 1.0);
            (id, s_rrf)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fuse_single_list() {
        let lists = vec![vec!["a".into(), "b".into(), "c".into()]];
        let result = fuse(&lists, 60);
        assert_eq!(result[0].0, "a");
        assert_eq!(result[1].0, "b");
        assert_eq!(result[2].0, "c");
    }

    #[test]
    fn test_fuse_consensus_boost() {
        let lists = vec![
            vec!["a".into(), "b".into(), "c".into()],
            vec!["b".into(), "a".into(), "c".into()],
        ];
        let result = fuse(&lists, 60);
        // "a" and "b" appear in top-2 of both lists, should rank higher than "c"
        assert!(result[0].0 == "a" || result[0].0 == "b");
        assert!(result[1].0 == "a" || result[1].0 == "b");
        assert_eq!(result[2].0, "c");
    }

    #[test]
    fn test_fuse_normalized_bounds() {
        let lists = vec![vec!["a".into(), "b".into()]];
        let result = fuse_normalized(&lists, 60);
        for (_, score) in &result {
            assert!(*score > 0.0 && *score <= 1.0);
        }
        // First item should be close to 60/61 ≈ 0.984
        assert!(result[0].1 > 0.95);
    }

    #[test]
    fn test_fuse_empty() {
        assert!(fuse(&[], 60).is_empty());
    }
}
