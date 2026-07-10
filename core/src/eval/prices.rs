//! Token price tables for cost estimation.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PriceTable {
    /// Dollars per million tokens.
    #[serde(default)]
    pub models: HashMap<String, ModelPrice>,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct ModelPrice {
    #[serde(default)]
    pub input_per_mtok: f64,
    #[serde(default)]
    pub output_per_mtok: f64,
    #[serde(default)]
    pub cache_hit_per_mtok: f64,
}

pub fn load_price_table(path: &Path) -> Result<PriceTable> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read price table {}", path.display()))?;
    Ok(toml::from_str(&text)?)
}

pub fn estimate_cost_usd(
    table: &PriceTable,
    model_id: &str,
    tokens_in: u64,
    tokens_out: u64,
    cache_hit: u64,
) -> f64 {
    let Some(price) = table.models.get(model_id).or_else(|| {
        // try suffix match
        table
            .models
            .iter()
            .find(|(k, _)| model_id.ends_with(k.as_str()) || k.ends_with(model_id))
            .map(|(_, v)| v)
    }) else {
        return 0.0;
    };

    let non_cache_in = tokens_in.saturating_sub(cache_hit);
    let input_cost = (non_cache_in as f64 / 1_000_000.0) * price.input_per_mtok;
    let cache_cost = (cache_hit as f64 / 1_000_000.0)
        * if price.cache_hit_per_mtok > 0.0 {
            price.cache_hit_per_mtok
        } else {
            price.input_per_mtok
        };
    let output_cost = (tokens_out as f64 / 1_000_000.0) * price.output_per_mtok;
    input_cost + cache_cost + output_cost
}
