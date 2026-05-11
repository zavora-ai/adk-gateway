//! RAG (Retrieval-Augmented Generation) pipeline and tool.
//!
//! Provides `RagPipelineBuilder` for constructing a RAG pipeline from config,
//! and `RagTool` for agent-invocable knowledge base search.
//!
//! The in-memory and SQLRite backends are both powered by `SqlRiteStore`,
//! giving every local pipeline real hybrid retrieval out of the box.
//!
//! Design: RagPipeline + RagTool [R25.1–R25.9, R25.11]

use crate::config::{ChunkingStrategy, RagConfig, VectorStoreBackend};
use crate::sqlrite_store::SqlRiteStore;
use serde::{Deserialize, Serialize};
use std::path::Path;

// ── Search result ──────────────────────────────────────────────────

/// A single search result from the RAG pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk_id: String,
    pub text: String,
    pub source: String,
    pub score: f64,
}

// ── RagPipeline ────────────────────────────────────────────────────

/// The assembled RAG pipeline: chunk → embed → store → search.
///
/// When the backend is `InMemory` or `SqlRite`, the store is a
/// `SqlRiteStore` opened in-memory, giving full hybrid retrieval
/// without any external service.
#[derive(Debug)]
pub struct RagPipeline {
    #[allow(dead_code)]
    vector_store: VectorStoreBackend,
    #[allow(dead_code)]
    embedding_provider: String,
    #[allow(dead_code)]
    embedding_model: Option<String>,
    #[allow(dead_code)]
    chunking: ChunkingStrategy,
    chunk_size: usize,
    chunk_overlap: usize,
    /// SQLRite-backed store (in-memory or file-backed).
    store: SqlRiteStore,
}

impl RagPipeline {
    /// Ingest a file or directory, chunking and storing the content.
    pub fn ingest(&self, path: &Path) -> anyhow::Result<usize> {
        if !path.exists() {
            anyhow::bail!("path does not exist: {}", path.display());
        }

        let mut total = 0;
        if path.is_dir() {
            for entry in std::fs::read_dir(path)? {
                let entry = entry?;
                if entry.file_type()?.is_file() {
                    total += self.store.ingest_file(&entry.path())?;
                }
            }
        } else {
            total += self.store.ingest_file(path)?;
        }
        Ok(total)
    }

    /// Search the pipeline for chunks relevant to the query.
    pub fn search(&self, query: &str, top_k: usize) -> Vec<SearchResult> {
        match self.store.search(query, top_k) {
            Ok(results) => results
                .into_iter()
                .map(|r| SearchResult {
                    chunk_id: r.chunk_id,
                    text: r.content,
                    source: r.doc_id,
                    score: r.hybrid_score as f64,
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Hybrid search combining text query with a vector embedding.
    pub fn hybrid_search(
        &self,
        query: &str,
        embedding: Vec<f32>,
        top_k: usize,
        alpha: f32,
    ) -> Vec<SearchResult> {
        match self.store.hybrid_search(query, embedding, top_k, alpha) {
            Ok(results) => results
                .into_iter()
                .map(|r| SearchResult {
                    chunk_id: r.chunk_id,
                    text: r.content,
                    source: r.doc_id,
                    score: r.hybrid_score as f64,
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Search with metadata filters (e.g. tenant isolation).
    pub fn filtered_search(
        &self,
        query: &str,
        top_k: usize,
        filters: std::collections::HashMap<String, String>,
    ) -> Vec<SearchResult> {
        match self.store.filtered_search(query, top_k, filters) {
            Ok(results) => results
                .into_iter()
                .map(|r| SearchResult {
                    chunk_id: r.chunk_id,
                    text: r.content,
                    source: r.doc_id,
                    score: r.hybrid_score as f64,
                })
                .collect(),
            Err(_) => vec![],
        }
    }

    /// Check database integrity (delegates to SQLRite's integrity_check).
    pub fn integrity_ok(&self) -> bool {
        self.store.integrity_ok().unwrap_or(false)
    }

    /// Single-call diagnostics snapshot.
    pub fn diagnostics(&self) -> crate::sqlrite_store::StoreDiagnostics {
        self.store
            .diagnostics()
            .unwrap_or(crate::sqlrite_store::StoreDiagnostics {
                document_count: 0,
                chunk_count: 0,
                integrity_ok: false,
            })
    }

    /// Reload the pipeline with new config (hot-reload support, R25.11).
    #[allow(dead_code)]
    pub fn reload(&mut self, config: &RagConfig) {
        self.vector_store = config.vector_store.clone();
        self.embedding_provider = config.embedding.provider.clone();
        self.embedding_model = config.embedding.model.clone();
        self.chunking = config.chunking.clone();
        self.chunk_size = config.chunk_size.unwrap_or(512);
        self.chunk_overlap = config.chunk_overlap.unwrap_or(50);
        // Re-open the store with updated config. On failure, keep the old one.
        if let Ok(new_store) = SqlRiteStore::open_in_memory(config) {
            self.store = new_store;
        }
        tracing::info!("RAG pipeline reloaded with new config");
    }

    /// Get the current vector store backend type.
    #[allow(dead_code)]
    pub fn vector_store(&self) -> &VectorStoreBackend {
        &self.vector_store
    }

    /// Get the number of indexed documents.
    pub fn document_count(&self) -> usize {
        self.store.document_count().unwrap_or(0)
    }

    /// Get the total number of chunks across all documents.
    pub fn chunk_count(&self) -> usize {
        self.store.chunk_count().unwrap_or(0)
    }
}

// ── RagPipelineBuilder ─────────────────────────────────────────────

/// Builds a `RagPipeline` from `RagConfig`.
pub struct RagPipelineBuilder;

impl RagPipelineBuilder {
    /// Build a RAG pipeline from the given configuration.
    ///
    /// For `InMemory` and `SqlRite` backends the store is opened in-memory.
    /// For `SqlRite` with a `connection_string`, a file-backed store is used.
    pub fn build(config: &RagConfig) -> anyhow::Result<RagPipeline> {
        // Validate embedding provider
        match config.embedding.provider.as_str() {
            "gemini" | "openai" => {}
            other => anyhow::bail!("unknown embedding provider: {other}"),
        }

        tracing::info!(
            vector_store = ?config.vector_store,
            embedding = %config.embedding.provider,
            chunking = ?config.chunking,
            "building RAG pipeline"
        );

        let store = match (&config.vector_store, &config.connection_string) {
            (VectorStoreBackend::SqlRite, Some(path)) => SqlRiteStore::open(path, config)?,
            // InMemory and SqlRite-without-path both use in-memory SQLRite
            (VectorStoreBackend::InMemory, _) | (VectorStoreBackend::SqlRite, None) => {
                SqlRiteStore::open_in_memory(config)?
            }
            _ => {
                // Other backends (Qdrant, LanceDb, etc.) fall back to
                // in-memory SQLRite for now; swap in real clients later.
                SqlRiteStore::open_in_memory(config)?
            }
        };

        Ok(RagPipeline {
            vector_store: config.vector_store.clone(),
            embedding_provider: config.embedding.provider.clone(),
            embedding_model: config.embedding.model.clone(),
            chunking: config.chunking.clone(),
            chunk_size: config.chunk_size.unwrap_or(512),
            chunk_overlap: config.chunk_overlap.unwrap_or(50),
            store,
        })
    }
}

// ── RagTool ────────────────────────────────────────────────────────

/// A tool that agents can invoke to search the RAG knowledge base.
#[derive(Debug)]
#[allow(dead_code)] // Constructed when RAG is configured; wraps pipeline for agent tool invocation
pub struct RagTool {
    pipeline: std::sync::Arc<RagPipeline>,
    top_k: usize,
}

impl RagTool {
    #[allow(dead_code)] // Used in tests; constructs RagTool for agent registration
    pub fn new(pipeline: std::sync::Arc<RagPipeline>, top_k: usize) -> Self {
        Self { pipeline, top_k }
    }

    /// Execute a text search query against the RAG pipeline.
    #[allow(dead_code)] // Used in tests; invoked by agent tool execution
    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>, String> {
        if query.trim().is_empty() {
            return Err("search query cannot be empty".to_string());
        }
        Ok(self.pipeline.search(query, self.top_k))
    }

    /// Execute a hybrid search combining text and vector embedding.
    pub fn hybrid_search(
        &self,
        query: &str,
        embedding: Vec<f32>,
        alpha: f32,
    ) -> Result<Vec<SearchResult>, String> {
        if query.trim().is_empty() {
            return Err("search query cannot be empty".to_string());
        }
        Ok(self
            .pipeline
            .hybrid_search(query, embedding, self.top_k, alpha))
    }

    /// Execute a filtered search with metadata key/value constraints.
    pub fn filtered_search(
        &self,
        query: &str,
        filters: std::collections::HashMap<String, String>,
    ) -> Result<Vec<SearchResult>, String> {
        if query.trim().is_empty() {
            return Err("search query cannot be empty".to_string());
        }
        Ok(self.pipeline.filtered_search(query, self.top_k, filters))
    }

    #[allow(dead_code)] // Used in tests; returns tool name for registration
    pub fn name(&self) -> &str {
        "rag_search"
    }

    #[allow(dead_code)] // Used in tests; returns tool description for registration
    pub fn description(&self) -> &str {
        "Search the knowledge base for relevant information"
    }
}

// ── Tests ──────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ChunkingStrategy, EmbeddingConfig, RagConfig, VectorStoreBackend};
    use std::io::Write;

    fn test_config() -> RagConfig {
        RagConfig {
            vector_store: VectorStoreBackend::InMemory,
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
    fn test_build_pipeline_valid() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        assert_eq!(pipeline.chunk_size, 100);
        assert_eq!(pipeline.chunk_overlap, 20);
    }

    #[test]
    fn test_build_pipeline_invalid_provider() {
        let mut config = test_config();
        config.embedding.provider = "unknown".into();
        assert!(RagPipelineBuilder::build(&config).is_err());
    }

    #[test]
    fn test_build_pipeline_sqlrite_backend() {
        let mut config = test_config();
        config.vector_store = VectorStoreBackend::SqlRite;
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        assert!(matches!(
            pipeline.vector_store(),
            VectorStoreBackend::SqlRite
        ));
    }

    #[test]
    fn test_ingest_and_search() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();

        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("doc.txt");
        let mut f = std::fs::File::create(&file_path).unwrap();
        writeln!(
            f,
            "Rust is a systems programming language focused on safety and performance"
        )
        .unwrap();

        let count = pipeline.ingest(&file_path).unwrap();
        assert!(count > 0);
        assert!(pipeline.chunk_count() > 0);

        let results = pipeline.search("Rust safety", 5);
        assert!(!results.is_empty());
        assert!(results[0].score > 0.0);
    }

    #[test]
    fn test_search_empty_query() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        let tool = RagTool::new(std::sync::Arc::new(pipeline), 5);
        assert!(tool.search("").is_err());
    }

    #[test]
    fn test_search_no_results() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        let results = pipeline.search("nonexistent query xyz", 5);
        assert!(results.is_empty());
    }

    #[test]
    fn test_ingest_nonexistent_path() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        assert!(pipeline.ingest(Path::new("/nonexistent/path")).is_err());
    }

    #[test]
    fn test_ingest_directory() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();

        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hello world").unwrap();
        std::fs::write(dir.path().join("b.txt"), "goodbye world").unwrap();

        let count = pipeline.ingest(dir.path()).unwrap();
        assert!(count >= 2);
        assert!(pipeline.chunk_count() >= 2);
    }

    #[test]
    fn test_reload() {
        let config = test_config();
        let mut pipeline = RagPipelineBuilder::build(&config).unwrap();
        assert_eq!(pipeline.chunk_size, 100);

        let mut new_config = test_config();
        new_config.chunk_size = Some(200);
        pipeline.reload(&new_config);
        assert_eq!(pipeline.chunk_size, 200);
    }

    #[test]
    fn test_rag_tool_name() {
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        let tool = RagTool::new(std::sync::Arc::new(pipeline), 5);
        assert_eq!(tool.name(), "rag_search");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_inmemory_uses_sqlrite() {
        // InMemory backend should be powered by SQLRite under the hood
        let config = test_config();
        let pipeline = RagPipelineBuilder::build(&config).unwrap();
        assert!(pipeline.store.integrity_ok().unwrap());
    }
}
