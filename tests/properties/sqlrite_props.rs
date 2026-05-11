//! Property-based tests for the SQLRite store integration.

use adk_gateway::config::{ChunkingStrategy, EmbeddingConfig, RagConfig, VectorStoreBackend};
use adk_gateway::sqlrite_store::SqlRiteStore;
use proptest::prelude::*;
use std::collections::HashMap;

fn sqlrite_config() -> RagConfig {
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

proptest! {
    /// Ingesting any non-empty ASCII text should produce at least one chunk.
    #[test]
    fn ingest_always_produces_chunks(text in "[a-zA-Z0-9 ]{1,500}") {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        let n = store.ingest_document("prop-doc", &text).unwrap();
        prop_assert!(n > 0, "expected at least 1 chunk, got {n}");
        prop_assert_eq!(store.chunk_count().unwrap(), n);
    }

    /// chunk_count should equal the sum of all ingested chunks.
    #[test]
    fn chunk_count_is_cumulative(
        a in "[a-z ]{10,200}",
        b in "[a-z ]{10,200}",
    ) {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        let n1 = store.ingest_document("doc-a", &a).unwrap();
        let n2 = store.ingest_document("doc-b", &b).unwrap();
        prop_assert_eq!(store.chunk_count().unwrap(), n1 + n2);
    }

    /// Search should never return more results than top_k.
    #[test]
    fn search_respects_top_k(top_k in 1_usize..20) {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        for i in 0..5 {
            store.ingest_document(
                &format!("doc-{i}"),
                &format!("chunk number {i} about retrieval and search engines"),
            ).unwrap();
        }
        let results = store.search("retrieval search", top_k).unwrap();
        prop_assert!(results.len() <= top_k);
    }

    /// Integrity check should always pass on a freshly opened database.
    #[test]
    fn fresh_db_integrity_ok(_seed in 0_u32..100) {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        prop_assert!(store.integrity_ok().unwrap());
    }

    /// Filtered search with a non-matching filter should return empty.
    #[test]
    fn filtered_search_no_match(query in "[a-z]{3,20}") {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        store.ingest_document("doc-x", "some content about AI agents").unwrap();
        let mut filters = HashMap::new();
        filters.insert("tenant".to_string(), "nonexistent-tenant-xyz".to_string());
        let results = store.filtered_search(&query, 5, filters).unwrap();
        prop_assert!(results.is_empty());
    }

    /// Hybrid search should never return more than top_k.
    #[test]
    fn hybrid_search_respects_top_k(top_k in 1_usize..10) {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        store.ingest_document("doc-h", "local-first retrieval with Rust").unwrap();
        let results = store.hybrid_search(
            "retrieval",
            vec![0.5, 0.3, 0.2],
            top_k,
            0.65,
        ).unwrap();
        prop_assert!(results.len() <= top_k);
    }

    /// diagnostics() should return consistent counts.
    #[test]
    fn diagnostics_consistent(_seed in 0_u32..10) {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        store.ingest_document("doc-d", "diagnostics property test").unwrap();
        let d = store.diagnostics().unwrap();
        prop_assert_eq!(d.document_count, 1);
        prop_assert!(d.chunk_count > 0);
        prop_assert!(d.integrity_ok);
    }

    /// delete_document should remove all chunks for that doc.
    #[test]
    fn delete_removes_document(text in "[a-z ]{10,100}") {
        let store = SqlRiteStore::open_in_memory(&sqlrite_config()).unwrap();
        store.ingest_document("doc-rm", &text).unwrap();
        prop_assert!(store.document_count().unwrap() > 0);
        store.delete_document("doc-rm").unwrap();
        prop_assert_eq!(store.document_count().unwrap(), 0);
    }

    /// VectorStoreBackend::SqlRite should round-trip through serde.
    #[test]
    fn sqlrite_backend_serde_roundtrip(_seed in 0_u32..10) {
        let backend = VectorStoreBackend::SqlRite;
        let json = serde_json::to_string(&backend).unwrap();
        let parsed: VectorStoreBackend = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(
            serde_json::to_string(&parsed).unwrap(),
            json
        );
    }
}
