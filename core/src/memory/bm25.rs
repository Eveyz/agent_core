//! BM25 full-text search index (powered by tantivy).
//!
//! Builds an in-memory index from all recall records. Writes are synchronous
//! — tantivy's writer handles concurrent add/delete with internal locking.
//!
//! The index is rebuilt from SQLite on restart (Brain initialization).

use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;

use parking_lot::Mutex;
use tantivy::{
    Index, IndexWriter, TantivyDocument, collector::TopDocs, doc, query::QueryParser, schema::*,
};

/// In-memory BM25 search index wrapping tantivy.
pub struct BM25Index {
    index: Index,
    schema: Schema,
    content_field: Field,
    id_field: Field,
    writer: Arc<Mutex<IndexWriter>>,
}

impl BM25Index {
    /// Create an empty BM25 index in memory.
    pub fn new() -> Result<Self> {
        let mut schema_builder = Schema::builder();
        let id_field = schema_builder.add_text_field("id", STRING | STORED);
        let content_field = schema_builder.add_text_field("content", TEXT | STORED);
        let schema = schema_builder.build();

        let index = Index::create_in_ram(schema.clone());
        let writer = Arc::new(Mutex::new(
            index
                .writer(50_000_000)
                .context("failed to create tantivy writer")?,
        ));

        Ok(Self {
            index,
            schema,
            content_field,
            id_field,
            writer,
        })
    }

    /// Build index from existing records. Faster than inserting one by one.
    pub fn from_records(records: &[(String, String)]) -> Result<Self> {
        let this = Self::new()?;
        if records.is_empty() {
            return Ok(this);
        }

        let mut writer = this.writer.lock();
        for (id, content) in records {
            writer.add_document(
                doc!(this.id_field => id.clone(), this.content_field => content.clone()),
            )?;
        }
        writer.commit()?;
        drop(writer);
        Ok(this)
    }

    /// Add a single document to the index.
    pub fn insert(&self, id: &str, content: &str) -> Result<()> {
        let mut writer = self.writer.lock();
        writer.add_document(
            doc!(self.id_field => id.to_string(), self.content_field => content.to_string()),
        )?;
        writer.commit()?;
        Ok(())
    }

    /// Remove a document from the index by id.
    pub fn delete(&self, id: &str) -> Result<()> {
        let mut writer = self.writer.lock();
        let term = tantivy::Term::from_field_text(self.id_field, id);
        writer.delete_term(term);
        writer.commit()?;
        Ok(())
    }

    /// Search for the top_k documents matching the query.
    /// Returns a list of ids in descending relevance order.
    pub fn search(&self, query: &str, top_k: usize) -> Result<Vec<String>> {
        let reader = self
            .index
            .reader()
            .context("failed to get tantivy reader")?;
        let searcher = reader.searcher();
        let query_parser = QueryParser::for_index(&self.index, vec![self.content_field]);
        let query = query_parser
            .parse_query(query)
            .context("failed to parse tantivy query")?;

        let top_docs = searcher
            .search(&query, &TopDocs::with_limit(top_k))
            .context("tantivy search failed")?;

        let mut ids = Vec::with_capacity(top_docs.len());
        for (_score, doc_address) in top_docs {
            let doc: TantivyDocument = searcher.doc(doc_address)?;
            let id_field_val = doc
                .get_first(self.id_field)
                .and_then(|v| v.as_str())
                .unwrap_or("");
            ids.push(id_field_val.to_string());
        }

        Ok(ids)
    }

    /// Rebuild index from disk (SQLite) on startup.
    /// `root_dir` is for tantivy's mmap directory — unused since we use in-RAM index.
    pub fn rebuild(_dir: &Path, records: &[(String, String)]) -> Result<Self> {
        Self::from_records(records)
    }
}
