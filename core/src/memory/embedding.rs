use anyhow::Result;
use fastembed::{InitOptions, TextEmbedding};
use parking_lot::Mutex;

pub struct EmbeddingModel {
    model: Mutex<TextEmbedding>,
}

impl EmbeddingModel {
    pub fn new(model_name: &str) -> Result<Self> {
        let model_enum = match model_name {
            "BAAI/bge-small-en-v1.5" => fastembed::EmbeddingModel::BGESmallENV15,
            "BAAI/bge-base-en-v1.5" => fastembed::EmbeddingModel::BGEBaseENV15,
            "sentence-transformers/all-MiniLM-L6-v2" => fastembed::EmbeddingModel::AllMiniLML6V2,
            _ => fastembed::EmbeddingModel::BGESmallENV15,
        };
        let init_options = InitOptions::new(model_enum).with_show_download_progress(true);
        let model = TextEmbedding::try_new(init_options)?;
        Ok(Self {
            model: Mutex::new(model),
        })
    }

    pub fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let mut model = self.model.lock();
        let embeddings = model.embed(texts, None)?;
        Ok(embeddings)
    }

    pub fn embed_single(&self, text: &str) -> Result<Vec<f32>> {
        let embeddings = self.embed(&[text.to_string()])?;
        Ok(embeddings.into_iter().next().unwrap_or_default())
    }

    pub fn dimension(&self) -> usize {
        384
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a * norm_b)
}

pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    embedding.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub fn bytes_to_embedding(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|chunk| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}
