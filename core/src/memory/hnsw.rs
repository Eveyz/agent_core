//! HNSW approximate nearest neighbor index (powered by instant-distance).
//!
//! Uses inner-product distance on normalized embeddings for approximate
//! cosine-similarity nearest-neighbor search.
//!
//! The HnswMap is immutable after construction (no insert/delete). The index
//! is rebuilt from SQLite on restart (Brain initialization). New memories
//! added at runtime are found by BM25; HNSW catches up on next restart.
//!
//! For production use with frequent writes, consider switching to `sqlite-vec`
//! or a mutable HNSW implementation.

use anyhow::Result;
use instant_distance::{Builder, HnswMap, Search};
use std::sync::RwLock;

use super::embedding::cosine_similarity;

/// A normalized embedding vector for HNSW.
#[derive(Clone)]
struct NormalizedEmbedding(Vec<f32>);

impl instant_distance::Point for NormalizedEmbedding {
    fn distance(&self, other: &Self) -> f32 {
        // Inner-product distance on normalized vectors:
        //   dist = 1.0 - dot(A, B)
        // This preserves cosine ordering for nearest-neighbor search.
        let dot: f32 = self.0.iter().zip(&other.0).map(|(a, b)| a * b).sum();
        1.0 - dot.max(-1.0).min(1.0)
    }
}

/// In-memory HNSW vector search index.
///
/// Immutable after construction. Rebuild from SQLite on restart.
pub struct HNSWIndex {
    map: RwLock<HnswMap<NormalizedEmbedding, String>>,
    /// Fallback records for brute-force search (records added after index build).
    /// Cleared on rebuild.
    fallback: RwLock<Vec<(String, Vec<f32>)>>,
}

impl HNSWIndex {
    /// Build an HNSW index from a list of (id, embedding) pairs.
    /// Embeddings should already be normalized.
    pub fn from_records(records: Vec<(String, Vec<f32>)>) -> Result<Self> {
        let points: Vec<NormalizedEmbedding> = records
            .iter()
            .map(|(_, emb)| NormalizedEmbedding(emb.clone()))
            .collect();
        let values: Vec<String> = records.iter().map(|(id, _)| id.clone()).collect();

        let builder = Builder::default().ef_search(150);
        let map = builder.build(points, values);

        Ok(Self {
            map: RwLock::new(map),
            fallback: RwLock::new(Vec::new()),
        })
    }

    /// Create an empty HNSW index.
    pub fn new() -> Self {
        let builder = Builder::default().ef_search(150);
        let map = builder.build::<NormalizedEmbedding, String>(Vec::new(), Vec::new());

        Self {
            map: RwLock::new(map),
            fallback: RwLock::new(Vec::new()),
        }
    }

    /// Append a new record to the fallback list (since HnswMap is immutable).
    /// Records in the fallback are searched via brute-force cosine.
    pub fn add_fallback(&self, id: String, embedding: Vec<f32>) {
        let mut fb = self.fallback.write().expect("HNSW fallback lock poisoned");
        fb.push((id, embedding));
    }

    /// Remove the oldest entry with the given id from the fallback list.
    pub fn remove_fallback(&self, id: &str) {
        let mut fb = self.fallback.write().expect("HNSW fallback lock poisoned");
        fb.retain(|(entry_id, _)| entry_id != id);
    }

    /// Number of entries in the fallback list.
    pub fn fallback_count(&self) -> usize {
        self.fallback.read().expect("HNSW fallback lock poisoned").len()
    }

    /// Search for the top_k nearest neighbors to the query embedding.
    /// Merges HNSW results with brute-force fallback results.
    pub fn search(&self, query: &[f32], top_k: usize) -> Vec<String> {
        let map = self.map.read().expect("HNSW lock poisoned");
        let query_point = NormalizedEmbedding(query.to_vec());

        let mut ids: Vec<String> = Vec::new();

        // Search HNSW index
        if !map.iter().next().is_none() {
            let mut search = Search::default();
            let results = map.search(&query_point, &mut search);
            ids.extend(results.map(|item| item.value.clone()));
        }

        // Brute-force search fallback records
        let fb = self.fallback.read().expect("HNSW fallback lock poisoned");
        if !fb.is_empty() {
            let mut scored: Vec<(f32, String)> = fb
                .iter()
                .map(|(id, emb)| (cosine_similarity(query, emb), id.clone()))
                .collect();
            scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
            let extra: Vec<String> = scored.into_iter().map(|(_, id)| id).collect();
            ids.extend(extra);
        }

        ids.truncate(top_k);
        ids
    }
}

/// Normalize a raw embedding vector to unit length for HNSW.
pub fn normalize_embedding(embedding: &[f32]) -> Vec<f32> {
    let norm: f32 = embedding.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > 0.0 {
        embedding.iter().map(|x| x / norm).collect()
    } else {
        embedding.to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_search() {
        let index = HNSWIndex::new();
        let results = index.search(&[1.0f32; 4], 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_returns_nearest() {
        let records = vec![
            ("a".into(), vec![1.0, 0.0, 0.0, 0.0]),
            ("b".into(), vec![0.0, 1.0, 0.0, 0.0]),
            ("c".into(), vec![0.0, 0.0, 1.0, 0.0]),
        ];
        let index = HNSWIndex::from_records(records).unwrap();

        let query = vec![1.0, 0.0, 0.0, 0.0];
        let results = index.search(&query, 3);
        assert_eq!(results[0], "a");
    }

    #[test]
    fn test_fallback_search() {
        let records = vec![("a".into(), vec![1.0, 0.0, 0.0, 0.0])];
        let index = HNSWIndex::from_records(records).unwrap();

        // Add new record via fallback
        index.add_fallback("b".into(), vec![0.0, 1.0, 0.0, 0.0]);

        let query = vec![0.0, 1.0, 0.0, 0.0];
        let results = index.search(&query, 2);
        assert!(results.contains(&"b".to_string()));
    }

    #[test]
    fn test_normalize_embedding() {
        let raw = vec![3.0, 4.0]; // norm = 5
        let normalized = normalize_embedding(&raw);
        assert!((normalized[0] - 0.6).abs() < 0.001);
        assert!((normalized[1] - 0.8).abs() < 0.001);
    }

    #[test]
    fn test_normalize_zero_vector() {
        let raw = vec![0.0; 10];
        let normalized = normalize_embedding(&raw);
        assert_eq!(normalized, raw);
    }
}
