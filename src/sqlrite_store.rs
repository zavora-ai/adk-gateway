//! SQLRite-backed retrieval store for the RAG pipeline.
//!
//! Uses `SqlRiteHandle` (Send + Sync, cloneable) for concurrent access
//! without a global Mutex. Delegates chunking to SQLRite's built-in
//! `ingest_document_text`, and uses `TextChunkInput` for text-first
//! ingestion when embeddings arrive later.

use crate::config::RagConfig;
use sqlrite::{DocumentIngestOptions, SearchRequest, SqlRite, SqlRiteHandle};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

/// Thin wrapper around [`SqlRiteHandle`] (file-backed) or [`SqlRite`]
/// (in-memory) that adapts SQLRite to the gateway's RAG pipeline interface.
///
/// For file-backed databases we use `SqlRiteHandle` (Send + Sync, cloneable,
/// connection-per-operation). For in-memory databases `SqlRiteHandle` doesn't
/// apply, so we fall back to `Mutex<SqlRite>`.
#[derive(Debug)]
pub struct SqlRiteStore {
    backend: StoreBackend,
}

#[derive(Debug)]
enum StoreBackend {
    Handle(SqlRiteHandle),
    InMemory(Mutex<SqlRite>),
}

/// A search hit returned by [`SqlRiteStore::search`].
#[derive(Debug, Clone)]
pub struct SqlRiteResult {
    pub chunk_id: String,
    pub doc_id: String,
    pub content: String,
    pub hybrid_score: f32,
}

/// Diagnostics snapshot from the underlying SQLRite database.
#[derive(Debug, Clone)]
pub struct StoreDiagnostics {
    pub document_count: usize,
    pub chunk_count: usize,
    pub integrity_ok: bool,
}

impl SqlRiteStore {
    /// Open an in-memory SQLRite database (tests and ephemeral pipelines).
    pub fn open_in_memory(_config: &RagConfig) -> anyhow::Result<Self> {
        let db = SqlRite::open_in_memory()?;
        Ok(Self {
            backend: StoreBackend::InMemory(Mutex::new(db)),
        })
    }

    /// Open a file-backed SQLRite database (uses SqlRiteHandle for concurrency).
    pub fn open(path: impl AsRef<Path>, _config: &RagConfig) -> anyhow::Result<Self> {
        let db = SqlRiteHandle::open(path)?;
        Ok(Self {
            backend: StoreBackend::Handle(db),
        })
    }

    /// Ingest a file using SQLRite's built-in document chunking.
    pub fn ingest_file(&self, path: &Path) -> anyhow::Result<usize> {
        let content = std::fs::read_to_string(path)?;
        let doc_id = path.display().to_string();
        self.ingest_document(&doc_id, &content)
    }

    /// Ingest a document string using SQLRite's built-in chunking.
    /// Replaces any previous chunks for the same `doc_id`.
    pub fn ingest_document(&self, doc_id: &str, text: &str) -> anyhow::Result<usize> {
        // Remove stale chunks from a previous ingestion of this doc
        let _ = self.delete_document(doc_id);

        let opts = DocumentIngestOptions::default();
        match &self.backend {
            StoreBackend::Handle(h) => {
                let report = h.ingest_document_text(doc_id, text, opts)?;
                Ok(report.chunk_count)
            }
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let report = db.ingest_document_text(doc_id, text, opts)?;
                Ok(report.chunk_count)
            }
        }
    }

    /// Text-only search.
    pub fn search(&self, query: &str, top_k: usize) -> anyhow::Result<Vec<SqlRiteResult>> {
        let req = SearchRequest::text_only(query, top_k);
        let results = match &self.backend {
            StoreBackend::Handle(h) => h.search(req)?,
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                db.search(req)?
            }
        };
        Ok(results.into_iter().map(into_result).collect())
    }

    /// Hybrid search combining text query with a vector embedding.
    pub fn hybrid_search(
        &self,
        query: &str,
        embedding: Vec<f32>,
        top_k: usize,
        alpha: f32,
    ) -> anyhow::Result<Vec<SqlRiteResult>> {
        let mut req = SearchRequest::hybrid(query, embedding, top_k);
        req.alpha = alpha;
        let results = match &self.backend {
            StoreBackend::Handle(h) => h.search(req)?,
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                db.search(req)?
            }
        };
        Ok(results.into_iter().map(into_result).collect())
    }

    /// Filtered search with metadata key/value pairs.
    pub fn filtered_search(
        &self,
        query: &str,
        top_k: usize,
        filters: HashMap<String, String>,
    ) -> anyhow::Result<Vec<SqlRiteResult>> {
        let mut req = SearchRequest::text_only(query, top_k);
        req.metadata_filters = filters;
        let results = match &self.backend {
            StoreBackend::Handle(h) => h.search(req)?,
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                db.search(req)?
            }
        };
        Ok(results.into_iter().map(into_result).collect())
    }

    /// Total chunks stored.
    pub fn chunk_count(&self) -> anyhow::Result<usize> {
        match &self.backend {
            StoreBackend::Handle(h) => Ok(h.chunk_count()?),
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(db.chunk_count()?)
            }
        }
    }

    /// Total distinct documents.
    pub fn document_count(&self) -> anyhow::Result<usize> {
        match &self.backend {
            StoreBackend::Handle(h) => Ok(h.document_count()?),
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(db.document_count()?)
            }
        }
    }

    /// Database integrity check.
    pub fn integrity_ok(&self) -> anyhow::Result<bool> {
        match &self.backend {
            StoreBackend::Handle(h) => Ok(h.diagnostics()?.integrity_check_ok),
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(db.integrity_check_ok()?)
            }
        }
    }

    /// Single-call diagnostics snapshot.
    pub fn diagnostics(&self) -> anyhow::Result<StoreDiagnostics> {
        match &self.backend {
            StoreBackend::Handle(h) => {
                let d = h.diagnostics()?;
                Ok(StoreDiagnostics {
                    document_count: d.document_count,
                    chunk_count: d.chunk_count,
                    integrity_ok: d.integrity_check_ok,
                })
            }
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                let d = db.diagnostics()?;
                Ok(StoreDiagnostics {
                    document_count: d.document_count,
                    chunk_count: d.chunk_count,
                    integrity_ok: d.integrity_check_ok,
                })
            }
        }
    }

    /// Delete all chunks for a document (useful before re-ingestion).
    pub fn delete_document(&self, doc_id: &str) -> anyhow::Result<usize> {
        match &self.backend {
            StoreBackend::Handle(h) => Ok(h.delete_by_doc_id(doc_id)?),
            StoreBackend::InMemory(m) => {
                let db = m.lock().map_err(|e| anyhow::anyhow!("{e}"))?;
                Ok(db.delete_by_doc_id(doc_id)?)
            }
        }
    }
}

fn into_result(r: sqlrite::SearchResult) -> SqlRiteResult {
    SqlRiteResult {
        chunk_id: r.chunk_id,
        doc_id: r.doc_id,
        content: r.content,
        hybrid_score: r.hybrid_score,
    }
}

// ── Unit tests ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChunkingStrategy, EmbeddingConfig, RagConfig, VectorStoreBackend};

    fn test_config() -> RagConfig {
        RagConfig {
            vector_store: VectorStoreBackend::SqlRite,
            connection_string: None,
            embedding: EmbeddingConfig {
                provider: "openai".into(),
                model: None,
            },
            chunking: ChunkingStrategy::FixedSize,
            chunk_size: Some(100),
            chunk_overlap: Some(20),
            watch_dirs: vec![],
            ingest_webhook: None,
        }
    }

    #[test]
    fn open_in_memory_and_check_integrity() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        assert!(store.integrity_ok().unwrap());
        assert_eq!(store.chunk_count().unwrap(), 0);
        assert_eq!(store.document_count().unwrap(), 0);
    }

    #[test]
    fn ingest_and_search() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        let n = store
            .ingest_document(
                "doc-a",
                "Rust and SQLite are ideal for local-first AI agents.",
            )
            .unwrap();
        assert!(n > 0);
        assert_eq!(store.document_count().unwrap(), 1);

        let results = store.search("local AI agents", 3).unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn ingest_file_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.txt");
        std::fs::write(&path, "Embedded retrieval with SQLRite is fast and simple.").unwrap();

        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        let n = store.ingest_file(&path).unwrap();
        assert!(n > 0);
    }

    #[test]
    fn hybrid_search_returns_results() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        store
            .ingest_document("doc-b", "Vector search and keyword search combined.")
            .unwrap();

        let results = store
            .hybrid_search("vector keyword", vec![0.9, 0.1, 0.0], 3, 0.65)
            .unwrap();
        assert!(!results.is_empty());
    }

    #[test]
    fn delete_document_removes_chunks() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        store
            .ingest_document("doc-del", "some content to delete")
            .unwrap();
        assert!(store.chunk_count().unwrap() > 0);

        let removed = store.delete_document("doc-del").unwrap();
        assert!(removed > 0);
        assert_eq!(store.chunk_count().unwrap(), 0);
    }

    #[test]
    fn diagnostics_returns_counts() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        store
            .ingest_document("doc-diag", "diagnostics test content")
            .unwrap();

        let d = store.diagnostics().unwrap();
        assert!(d.chunk_count > 0);
        assert_eq!(d.document_count, 1);
        assert!(d.integrity_ok);
    }

    #[test]
    fn empty_search_returns_empty() {
        let store = SqlRiteStore::open_in_memory(&test_config()).unwrap();
        let results = store.search("anything", 5).unwrap();
        assert!(results.is_empty());
    }
}
